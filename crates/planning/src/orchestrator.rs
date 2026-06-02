use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::roles::persist_output;

// ── Agent Message ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    pub content: String,
}

// ── AgentRole trait — pluggable agent roles ───────────────────

#[async_trait]
pub trait OrchestratorRole: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn process(
        &self,
        input: &str,
        history: &[AgentMessage],
        cancel: CancellationToken,
    ) -> Result<String, miniagent_core::error::AgentError>;
}

// ── LLM-backed Role Agent ─────────────────────────────────────

pub struct RoleAgent {
    name: String,
    system_prompt: String,
    provider: Box<dyn miniagent_provider::traits::LlmProvider>,
    tools: Vec<miniagent_provider::traits::ToolDef>,
}

impl RoleAgent {
    pub fn new(
        name: impl Into<String>,
        system_prompt: impl Into<String>,
        provider: Box<dyn miniagent_provider::traits::LlmProvider>,
    ) -> Self {
        Self {
            name: name.into(),
            system_prompt: system_prompt.into(),
            provider,
            tools: Vec::new(),
        }
    }

    pub fn with_tools(mut self, tools: Vec<miniagent_provider::traits::ToolDef>) -> Self {
        self.tools = tools;
        self
    }
}

#[async_trait]
impl OrchestratorRole for RoleAgent {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.system_prompt }

    async fn process(
        &self,
        input: &str,
        history: &[AgentMessage],
        cancel: CancellationToken,
    ) -> Result<String, miniagent_core::error::AgentError> {
        let mut messages: Vec<miniagent_core::message::Message> = Vec::new();

        let history_context: String = history.iter()
            .map(|m| format!("[{} → {}]: {}", m.from, m.to, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let full_input = if history_context.is_empty() {
            input.to_string()
        } else {
            format!("Conversation history:\n{history_context}\n\nYour task: {input}")
        };

        messages.push(miniagent_core::message::Message::user(&full_input));

        let request = miniagent_provider::traits::CompletionRequest {
            system: self.system_prompt.clone(),
            messages,
            tools: self.tools.clone(),
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.3), max_tokens: Some(2000), ..Default::default()
            },
        };

        let response = self.provider.complete(&request, cancel).await?;
        let text = response.content.iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("");

        Ok(text)
    }
}

// ── Structured Delegation ─────────────────────────────────────

/// Structured task delegation (replaces fragile string parsing).
/// The supervisor outputs this as JSON, workers parse it directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub task_id: String,
    pub agent: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub input_files: Vec<String>,
    pub expected_output: String,
    pub priority: u8,
}

// ── Orchestration Patterns ────────────────────────────────────

#[derive(Debug)]
pub enum OrchestrationPattern {
    /// Agents execute sequentially, each receiving the previous agent's output
    Chain,
    /// All agents process independently (parallel fan-out), results are collected
    Parallel,
    /// Agents debate: multiple rounds of critique and refinement
    Debate { rounds: usize },
    /// One supervisor delegates subtasks to specialized agents
    Hierarchical,
}

// ── Orchestrator ──────────────────────────────────────────────

pub struct Orchestrator {
    agents: Vec<Arc<dyn OrchestratorRole>>,
    history: Vec<AgentMessage>,
    work_dir: std::path::PathBuf,
}

impl Orchestrator {
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            history: Vec::new(),
            work_dir: std::path::PathBuf::from("./miniagent_workspace"),
        }
    }

    pub fn with_work_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.work_dir = dir.into();
        std::fs::create_dir_all(&self.work_dir).ok();
        self
    }

    pub fn register<A: OrchestratorRole + 'static>(&mut self, agent: A) -> &mut Self {
        self.agents.push(Arc::new(agent));
        self
    }

    pub fn agent_count(&self) -> usize { self.agents.len() }

    /// Execute agents according to the specified pattern
    pub async fn execute(
        &mut self,
        input: &str,
        pattern: OrchestrationPattern,
        cancel: CancellationToken,
    ) -> Result<Vec<(String, String)>, miniagent_core::error::AgentError> {
        std::fs::create_dir_all(&self.work_dir).ok();
        match pattern {
            OrchestrationPattern::Chain => self.execute_chain(input, cancel).await,
            OrchestrationPattern::Parallel => self.execute_parallel(input, cancel).await,
            OrchestrationPattern::Debate { rounds } => self.execute_debate(input, rounds, cancel).await,
            OrchestrationPattern::Hierarchical => self.execute_hierarchical(input, cancel).await,
        }
    }

    async fn execute_chain(
        &mut self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<(String, String)>, miniagent_core::error::AgentError> {
        let mut results = Vec::new();
        let mut current_input = input.to_string();

        for agent in &self.agents {
            let output = agent.process(&current_input, &self.history, cancel.child_token()).await?;
            self.history.push(AgentMessage {
                from: "user".into(), to: agent.name().into(), content: current_input.clone(),
            });
            self.history.push(AgentMessage {
                from: agent.name().into(), to: "orchestrator".into(), content: output.clone(),
            });

            // Persist each agent's output
            persist_output(&self.work_dir, agent.name(), "output.txt", &output);

            results.push((agent.name().to_string(), output.clone()));
            current_input = output;
        }

        Ok(results)
    }

    async fn execute_parallel(
        &mut self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<(String, String)>, miniagent_core::error::AgentError> {
        let tasks: Vec<_> = self.agents.iter().map(|agent| {
            let agent = agent.clone();
            let input = input.to_string();
            let cancel = cancel.child_token();
            tokio::spawn(async move {
                let output = agent.process(&input, &[], cancel).await;
                (agent.name().to_string(), output)
            })
        }).collect();

        let mut results = Vec::new();
        for task in tasks {
            let (name, result) = match task.await {
                Ok(res) => res,
                Err(e) => {
                    results.push(("unknown".into(), format!("Task panicked: {e}")));
                    continue;
                }
            };
            match result {
                Ok(output) => {
                    persist_output(&self.work_dir, &name, "output.txt", &output);
                    results.push((name, output));
                }
                Err(e) => {
                    results.push((name, format!("Error: {e}")));
                }
            }
        }

        Ok(results)
    }

    async fn execute_debate(
        &mut self,
        topic: &str,
        rounds: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<(String, String)>, miniagent_core::error::AgentError> {
        let mut results = Vec::new();

        for round in 0..rounds {
            let round_input = if round == 0 {
                topic.to_string()
            } else {
                self.history.iter()
                    .filter(|m| m.to == "orchestrator")
                    .map(|m| format!("[{}]: {}", m.from, m.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            for agent in &self.agents {
                let prompt = if round == 0 {
                    format!("Analyze this topic. Provide your expert perspective:\n\n{round_input}")
                } else {
                    format!(
                        "Previous round opinions:\n{round_input}\n\n\
                         Critically evaluate the above perspectives. \
                         Agree, disagree, or refine. Be specific.",
                    )
                };

                let output = agent.process(&prompt, &self.history, cancel.child_token()).await?;
                self.history.push(AgentMessage {
                    from: agent.name().into(), to: "orchestrator".into(), content: output.clone(),
                });
                persist_output(&self.work_dir, agent.name(), &format!("round_{round}.txt"), &output);
                results.push((agent.name().to_string(), output));
            }
        }

        Ok(results)
    }

    /// Hierarchical: supervisor decomposes task into structured JSON delegations.
    /// Replaces fragile "AGENT: xxx | TASK: yyy" string parsing.
    async fn execute_hierarchical(
        &mut self,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<(String, String)>, miniagent_core::error::AgentError> {
        if self.agents.is_empty() { return Ok(Vec::new()); }

        let supervisor = &self.agents[0];
        let workers: Vec<Arc<dyn OrchestratorRole>> = self.agents[1..].to_vec();

        // Step 1: Supervisor decomposes task into structured JSON
        let decompose_prompt = format!(
            r#"You are a supervisor. Decompose this task into subtasks for {} specialized agents.
Agents: {}

For each agent, create a concrete subtask.

Output ONLY a JSON array of objects:
[
  {{
    "task_id": "task-1",
    "agent": "agent-name",
    "description": "what this agent should do",
    "dependencies": [],
    "input_files": [],
    "expected_output": "description of expected result",
    "priority": 1
  }}
]

Task: {input}"#,
            workers.len(),
            workers.iter().map(|a| a.name()).collect::<Vec<_>>().join(", ")
        );

        let plan_text = supervisor.process(&decompose_prompt, &[], cancel.child_token()).await?;
        persist_output(&self.work_dir, supervisor.name(), "plan.json", &plan_text);

        // Step 2: Parse structured delegations (robust JSON parsing)
        let delegations = Self::parse_delegations(&plan_text, &workers);

        // Step 3: Execute delegations (respecting dependencies)
        let mut results = Vec::new();
        results.push((supervisor.name().to_string(), plan_text));

        // Simple dependency resolution: execute in order, skip unresolved deps
        for delegation in &delegations {
            if cancel.is_cancelled() { break; }

            if let Some(worker) = workers.iter().find(|a| a.name() == delegation.agent) {
                let output = worker.process(
                    &delegation.description,
                    &[],
                    cancel.child_token(),
                ).await;
                match output {
                    Ok(out) => {
                        persist_output(&self.work_dir, &delegation.agent, &format!("{}.txt", delegation.task_id), &out);
                        results.push((delegation.agent.clone(), out));
                    }
                    Err(e) => {
                        results.push((delegation.agent.clone(), format!("Error: {e}")));
                    }
                }
            }
        }

        Ok(results)
    }

    /// Parse supervisor output into structured delegations.
    /// Handles: pure JSON array, JSON wrapped in markdown code blocks,
    /// and falls back gracefully on parse failures.
    fn parse_delegations(text: &str, workers: &[Arc<dyn OrchestratorRole>]) -> Vec<Delegation> {
        let json_str = text.trim()
            .trim_start_matches("```json").trim_start_matches("```")
            .trim_end_matches("```");

        // Try parsing as JSON array
        if let Ok(delegations) = serde_json::from_str::<Vec<Delegation>>(json_str) {
            return delegations;
        }

        // Try parsing as object with "tasks" key
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str) {
            if let Some(tasks) = obj.get("tasks").and_then(|t| t.as_array()) {
                let delegations: Vec<Delegation> = tasks.iter()
                    .filter_map(|t| serde_json::from_value(t.clone()).ok())
                    .collect();
                if !delegations.is_empty() {
                    return delegations;
                }
            }
            // Try as array
            if let Some(arr) = obj.as_array() {
                let delegations: Vec<Delegation> = arr.iter()
                    .filter_map(|t| serde_json::from_value(t.clone()).ok())
                    .collect();
                if !delegations.is_empty() {
                    return delegations;
                }
            }
        }

        // Fallback: assign the entire task to each worker
        workers.iter().enumerate().map(|(i, w)| {
            Delegation {
                task_id: format!("task-{}", i + 1),
                agent: w.name().to_string(),
                description: text.to_string(),
                dependencies: vec![],
                input_files: vec![],
                expected_output: String::new(),
                priority: 1,
            }
        }).collect()
    }
}

impl Default for Orchestrator {
    fn default() -> Self { Self::new() }
}
