use async_trait::async_trait;
use chrono::{DateTime, Utc};
use miniagent_core::error::AgentError;
use miniagent_provider::traits::LlmProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use crate::roles::{
    AgentRole, Blackboard, FileContext, RoleOutput,
    persist_output, append_event, parse_llm_json,
    ProposerRole, OpponentRole, CriticRole,
};
use crate::state_graph::ModelTier;
use crate::tournament::TournamentArena;

/// Template for dynamic agent creation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentTemplate {
    Proposer,
    Opponent,
    DomainSpecialist,
    MethodReviewer,
    EvolutionMutator,
}

/// Lifecycle status of a dynamic agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Active,
    Idle,
    Retired,
}

/// Specification for creating a dynamic agent at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    pub template: AgentTemplate,
    pub system_prompt_suffix: String,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub write_permissions: Vec<String>,
    pub max_iterations: usize,
    pub model_tier: ModelTier,
    pub metadata: HashMap<String, String>,
}

impl Default for AgentSpec {
    fn default() -> Self {
        Self {
            template: AgentTemplate::Proposer,
            system_prompt_suffix: String::new(),
            tools: vec![],
            skills: vec![],
            write_permissions: vec![],
            max_iterations: 10,
            model_tier: ModelTier::Flash,
            metadata: HashMap::new(),
        }
    }
}

/// A runtime-created agent with lifecycle management.
pub struct DynamicAgent {
    pub id: String,
    pub spec: AgentSpec,
    pub role: Box<dyn AgentRole>,
    pub created_at: DateTime<Utc>,
    pub status: AgentStatus,
    pub reward_signal: f64,
}

impl std::fmt::Debug for DynamicAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicAgent")
            .field("id", &self.id)
            .field("template", &self.spec.template)
            .field("status", &self.status)
            .field("reward_signal", &self.reward_signal)
            .finish_non_exhaustive()
    }
}

impl DynamicAgent {
    pub fn new(id: impl Into<String>, spec: AgentSpec, role: Box<dyn AgentRole>) -> Self {
        Self {
            id: id.into(),
            spec,
            role,
            created_at: Utc::now(),
            status: AgentStatus::Active,
            reward_signal: 0.0,
        }
    }

    pub fn retire(&mut self) {
        self.status = AgentStatus::Retired;
    }

    pub fn add_reward(&mut self, delta: f64) {
        self.reward_signal += delta;
    }
}

/// Registry of active dynamic agents and their templates.
#[derive(Debug)]
pub struct AgentRegistry {
    pub agents: HashMap<String, DynamicAgent>,
    pub default_templates: HashMap<AgentTemplate, AgentSpec>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        let mut default_templates = HashMap::new();
        default_templates.insert(AgentTemplate::Proposer, AgentSpec {
            template: AgentTemplate::Proposer,
            tools: vec!["pubmed_search".into(), "web_search".into(), "web_fetch".into()],
            model_tier: ModelTier::Pro,
            ..Default::default()
        });
        default_templates.insert(AgentTemplate::Opponent, AgentSpec {
            template: AgentTemplate::Opponent,
            tools: vec!["pubmed_search".into(), "web_search".into()],
            model_tier: ModelTier::Pro,
            ..Default::default()
        });
        default_templates.insert(AgentTemplate::DomainSpecialist, AgentSpec {
            template: AgentTemplate::DomainSpecialist,
            tools: vec!["pubmed_search".into(), "web_fetch".into()],
            model_tier: ModelTier::Pro,
            ..Default::default()
        });
        default_templates.insert(AgentTemplate::MethodReviewer, AgentSpec {
            template: AgentTemplate::MethodReviewer,
            tools: vec!["web_search".into()],
            model_tier: ModelTier::Flash,
            ..Default::default()
        });
        default_templates.insert(AgentTemplate::EvolutionMutator, AgentSpec {
            template: AgentTemplate::EvolutionMutator,
            tools: vec!["pubmed_search".into(), "web_search".into()],
            model_tier: ModelTier::Pro,
            ..Default::default()
        });

        Self {
            agents: HashMap::new(),
            default_templates,
        }
    }
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a dynamic agent from a template spec and LLM provider.
    pub fn create_agent(
        &mut self,
        id: impl Into<String>,
        spec: &AgentSpec,
        provider: Box<dyn LlmProvider>,
    ) -> &DynamicAgent {
        let id = id.into();
        let role: Box<dyn AgentRole> = match spec.template {
            AgentTemplate::Proposer => Box::new(ProposerRole::new(provider)),
            AgentTemplate::Opponent => Box::new(OpponentRole::new(provider)),
            AgentTemplate::DomainSpecialist => Box::new(CriticRole::new(provider)),
            AgentTemplate::MethodReviewer => Box::new(CriticRole::new(provider)),
            AgentTemplate::EvolutionMutator => Box::new(ProposerRole::new(provider)),
        };

        let agent = DynamicAgent::new(&id, spec.clone(), role);
        self.agents.insert(id.clone(), agent);
        self.agents.get(&id).unwrap()
    }

    /// Retire all agents of a given template type.
    pub fn retire_by_template(&mut self, template: &AgentTemplate) {
        for agent in self.agents.values_mut() {
            if &agent.spec.template == template {
                agent.retire();
            }
        }
    }

    /// Remove all retired agents.
    pub fn cleanup_retired(&mut self) {
        self.agents.retain(|_, a| a.status != AgentStatus::Retired);
    }

    /// Get active agents of a specific template.
    pub fn active_of_type(&self, template: &AgentTemplate) -> Vec<&DynamicAgent> {
        self.agents.values()
            .filter(|a| a.status == AgentStatus::Active && &a.spec.template == template)
            .collect()
    }

    /// Execute a task on a specific agent.
    pub async fn execute_agent(
        &mut self,
        agent_id: &str,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let agent = self.agents.get_mut(agent_id)
            .ok_or_else(|| AgentError::Internal(format!("Agent '{agent_id}' not found")))?;

        if agent.status != AgentStatus::Active {
            return Err(AgentError::Internal(format!("Agent '{agent_id}' is not active")));
        }

        let result = agent.role.execute(task, blackboard, cancel).await;

        // Propagate reward: if hypothesis rating improved, reward the proposer
        if let Ok(ref output) = result
            && output.status == "success" {
                agent.add_reward(0.1);
            }

        result
    }
}

/// Scheduler role: manages dynamic agent creation based on tournament state.
/// Reads tournament arena state, decides which agents to create/destroy.
pub struct SchedulerRole {
    provider: Box<dyn LlmProvider>,
    registry: AgentRegistry,
}

impl SchedulerRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self {
            provider,
            registry: AgentRegistry::new(),
        }
    }

    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut AgentRegistry {
        &mut self.registry
    }
}

#[async_trait]
impl AgentRole for SchedulerRole {
    fn name(&self) -> &str { "scheduler" }
    fn description(&self) -> &str {
        "Dynamic agent scheduler. Creates and manages specialized agents based on \
         tournament state. Assigns tools, skills, and permissions."
    }

    async fn execute(
        &self,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        append_event(&blackboard.work_dir, "scheduler: analyzing tournament state");

        // Read tournament state from blackboard
        let arena_state = blackboard.artifacts.get("tournament_arena")
            .and_then(|s| serde_json::from_str::<TournamentArena>(s).ok());

        let mut agents_to_create: Vec<(String, AgentTemplate, String)> = vec![];

        if let Some(arena) = &arena_state {
            // For each hypothesis needing revision, create an EvolutionMutator
            for h_id in arena.hypotheses_needing_revision() {
                let agent_id = format!("mutator_{h_id}");
                agents_to_create.push((agent_id, AgentTemplate::EvolutionMutator, h_id));
            }

            // Ensure we have enough proposers and opponents for debates
            let pairs = arena.round_robin_pairs();
            for (i, (a_id, b_id)) in pairs.iter().enumerate() {
                let prop_agent = format!("proposer_{a_id}");
                let opp_agent = format!("opponent_{b_id}_{i}");
                agents_to_create.push((prop_agent, AgentTemplate::Proposer, a_id.clone()));
                agents_to_create.push((opp_agent, AgentTemplate::Opponent, b_id.clone()));
            }
        } else {
            // No tournament state — parse task for agent creation instructions
            let prompt = format!(
                "Analyze this task and determine what agents to create. \
                 Output JSON: {{\"agents\": [{{\"template\": \"Proposer|Opponent|DomainSpecialist|MethodReviewer|EvolutionMutator\", \
                 \"id\": \"agent_id\", \"domain_hint\": \"optional domain context\"}}]}}\n\nTask: {task}"
            );

            let request = miniagent_provider::traits::CompletionRequest {
                system: "You are an agent scheduler. Output only valid JSON.".into(),
                messages: vec![miniagent_core::message::Message::user(&prompt)],
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.1),
                    max_tokens: Some(1000),
                    ..Default::default()
                },
            };

            let resp = self.provider.complete(&request, cancel).await?;
            let text: String = resp.content.iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();

            if let Ok(parsed) = parse_llm_json(&text)
                && let Some(arr) = parsed["agents"].as_array() {
                    for agent_def in arr {
                        let template_str = agent_def["template"].as_str().unwrap_or("Proposer");
                        let template = match template_str {
                            "Opponent" => AgentTemplate::Opponent,
                            "DomainSpecialist" => AgentTemplate::DomainSpecialist,
                            "MethodReviewer" => AgentTemplate::MethodReviewer,
                            "EvolutionMutator" => AgentTemplate::EvolutionMutator,
                            _ => AgentTemplate::Proposer,
                        };
                        let id = agent_def["id"].as_str().unwrap_or("unknown").to_string();
                        let hint = agent_def["domain_hint"].as_str().unwrap_or("").to_string();
                        agents_to_create.push((id, template, hint));
                    }
                }
        }

        // Persist scheduling decision
        let decision = serde_json::to_string_pretty(&serde_json::json!({
            "agents_to_create": agents_to_create.iter().map(|(id, t, hint)| {
                serde_json::json!({"id": id, "template": format!("{t:?}"), "domain_hint": hint})
            }).collect::<Vec<_>>(),
        })).unwrap_or_default();

        persist_output(&blackboard.work_dir, "scheduler", "decision.json", &decision);
        append_event(&blackboard.work_dir, &format!("scheduler: planned {} agents", agents_to_create.len()));

        Ok(RoleOutput {
            content: format!("Scheduled {} agents for creation", agents_to_create.len()),
            evidence: vec![],
            confidence: 0.9,
            metadata: agents_to_create.into_iter().map(|(id, t, hint)| {
                (id, format!("{t:?}:{hint}"))
            }).collect(),
            output_files: vec!["scheduler/decision.json".into()],
            status: "success".into(),
        })
    }
}

#[async_trait]
impl FileContext for SchedulerRole {
    fn workspace_name(&self) -> &str { "scheduler" }
}
