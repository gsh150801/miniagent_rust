use std::sync::Arc;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use miniagent_agent::Agent;
use miniagent_core::types::StageId;
use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};

use crate::engine::Workflow;
use crate::stage::{ProviderSelector, Stage};
use crate::stages::{AgentStage, CriticStage, GenericLlmStage, SynthesizerStage};

fn default_flash() -> String { "flash".into() }
fn default_max_iter() -> usize { 35 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSpec {
    pub task_type: String,
    pub stages: Vec<StageSpec>,
    pub edges: Vec<[String; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageSpec {
    pub name: String,
    pub handler_type: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_flash")]
    pub model_tier: String,
    #[serde(default = "default_max_iter")]
    pub max_iterations: usize,
    #[serde(default)]
    pub enable_skills: bool,
    /// Human-readable description of what this stage does
    #[serde(default)]
    pub description: String,
    /// Sub-tasks delegated to this stage's agent
    #[serde(default)]
    pub sub_tasks: Vec<String>,
}

pub struct WorkflowBuilder {
    agent: Arc<Agent>,
    api_key: String,
    max_iterations: usize,
    max_tokens: u32,
    task_dir: Option<String>,
}

impl WorkflowBuilder {
    pub fn new(agent: Arc<Agent>, api_key: impl Into<String>) -> Self {
        Self { agent, api_key: api_key.into(), max_iterations: 35, max_tokens: 10_000_000, task_dir: None }
    }

    pub fn with_limits(mut self, max_iterations: usize, max_tokens: u32) -> Self {
        self.max_iterations = max_iterations;
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_task_dir(mut self, dir: impl Into<String>) -> Self {
        self.task_dir = Some(dir.into());
        self
    }

    pub fn build(
        &self,
        spec: &WorkflowSpec,
        prompt: &str,
        system: &str,
    ) -> Result<Workflow, String> {
        // Validate: unique names
        let mut seen = HashMap::new();
        for s in &spec.stages {
            if seen.contains_key(&s.name) {
                return Err(format!("Duplicate stage name: '{}'", s.name));
            }
            seen.insert(s.name.clone(), s);
        }

        // Validate: edges reference existing names
        for [from, to] in &spec.edges {
            if !seen.contains_key(from) {
                return Err(format!("Edge references unknown stage: '{from}'"));
            }
            if !seen.contains_key(to) {
                return Err(format!("Edge references unknown stage: '{to}'"));
            }
        }

        // Build stages and track name -> StageId
        let mut name_to_id: HashMap<String, StageId> = HashMap::new();
        let mut stages: Vec<Stage> = Vec::new();
        let mut wf = Workflow::new(&spec.task_type);

        for stage_spec in &spec.stages {
            let provider = match stage_spec.model_tier.as_str() {
                "pro" => ProviderSelector::Pro,
                _ => ProviderSelector::Flash,
            };
            let stage = match stage_spec.handler_type.as_str() {
                "agent" => {
                    let handler = AgentStage::new(self.agent.clone())
                        .with_limits(self.max_iterations, self.max_tokens);
                    Stage::new(&stage_spec.name, handler).with_provider(provider)
                }
                "critic" => {
                    let p: Box<dyn miniagent_provider::traits::LlmProvider> =
                        Box::new(DeepSeekFlash::new(&self.api_key));
                    Stage::new(&stage_spec.name, CriticStage::new(p, "DeepSeek Flash")).with_provider(provider)
                }
                "synthesizer" => {
                    let p: Box<dyn miniagent_provider::traits::LlmProvider> =
                        Box::new(DeepSeekPro::new(&self.api_key));
                    Stage::new(&stage_spec.name, SynthesizerStage::new(p, "DeepSeek Pro")).with_provider(provider)
                }
                _ => {
                    // "llm" or unknown → GenericLlmStage
                    let p: Box<dyn miniagent_provider::traits::LlmProvider> = match stage_spec.model_tier.as_str() {
                        "pro" => Box::new(DeepSeekPro::new(&self.api_key)),
                        _ => Box::new(DeepSeekFlash::new(&self.api_key)),
                    };
                    let sys = if stage_spec.system_prompt.is_empty() {
                        "You are a helpful AI assistant.".into()
                    } else {
                        stage_spec.system_prompt.clone()
                    };
                    Stage::new(&stage_spec.name, GenericLlmStage::new(p, &stage_spec.name, &sys))
                        .with_provider(provider)
                }
            };
            name_to_id.insert(stage_spec.name.clone(), stage.id);
            stages.push(stage);
        }

        // Add stages
        for stage in stages {
            wf = wf.add_stage(stage);
        }

        // Add edges
        for [from, to] in &spec.edges {
            let from_id = name_to_id.get(from).ok_or_else(|| format!("Missing stage: {from}"))?;
            let to_id = name_to_id.get(to).ok_or_else(|| format!("Missing stage: {to}"))?;
            wf = wf.add_edge(*from_id, *to_id);
        }

        // Set input
        let task_dir = self.task_dir.clone().unwrap_or_else(|| "./result/.workflow".into());
        wf = wf.with_input(serde_json::json!({
            "prompt": prompt,
            "system": system,
            "complexity": "moderate",
            "provider": "flash",
            "task_dir": task_dir,
        }));

        Ok(wf)
    }
}

/// Built-in workflow presets matching common task patterns.
impl WorkflowSpec {
    pub fn single_agent() -> Self {
        Self {
            task_type: "single_agent".into(),
            stages: vec![StageSpec {
                name: "agent".into(),
                handler_type: "agent".into(),
                system_prompt: String::new(),
                tools: vec![],
                model_tier: "flash".into(),
                max_iterations: 50,
                enable_skills: true,
                description: String::new(),
                sub_tasks: vec![],
            }],
            edges: vec![],
        }
    }

    pub fn deep_research() -> Self {
        Self {
            task_type: "deep_research".into(),
            stages: vec![
                StageSpec {
                    name: "research".into(),
                    handler_type: "agent".into(),
                    system_prompt: String::new(),
                    tools: vec!["web_search".into(), "web_fetch".into(), "pubmed_search".into()],
                    model_tier: "flash".into(),
                    max_iterations: 50,
                    enable_skills: true,
                    description: String::new(),
                    sub_tasks: vec![],
                },
                StageSpec {
                    name: "critique".into(),
                    handler_type: "critic".into(),
                    system_prompt: String::new(),
                    tools: vec![],
                    model_tier: "flash".into(),
                    max_iterations: 1,
                    enable_skills: false,
                    description: String::new(),
                    sub_tasks: vec![],
                },
                StageSpec {
                    name: "synthesize".into(),
                    handler_type: "synthesizer".into(),
                    system_prompt: String::new(),
                    tools: vec![],
                    model_tier: "pro".into(),
                    max_iterations: 1,
                    enable_skills: false,
                    description: String::new(),
                    sub_tasks: vec![],
                },
            ],
            edges: vec![
                ["research".into(), "critique".into()],
                ["critique".into(), "synthesize".into()],
            ],
        }
    }

    pub fn writing() -> Self {
        Self {
            task_type: "writing".into(),
            stages: vec![
                StageSpec {
                    name: "research".into(),
                    handler_type: "agent".into(),
                    system_prompt: String::new(),
                    tools: vec!["web_search".into(), "web_fetch".into()],
                    model_tier: "flash".into(),
                    max_iterations: 30,
                    enable_skills: true,
                    description: String::new(),
                    sub_tasks: vec![],
                },
                StageSpec {
                    name: "writer".into(),
                    handler_type: "synthesizer".into(),
                    system_prompt: "You are an expert writer. Produce polished, well-structured prose based on the research findings. Use the same language as the original user request.".into(),
                    tools: vec![],
                    model_tier: "pro".into(),
                    max_iterations: 1,
                    enable_skills: false,
                    description: String::new(),
                    sub_tasks: vec![],
                },
            ],
            edges: vec![["research".into(), "writer".into()]],
        }
    }

}
