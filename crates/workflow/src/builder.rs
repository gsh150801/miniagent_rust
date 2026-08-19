use std::sync::Arc;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use miniagent_agent::Agent;
use miniagent_core::settings::AppConfig;
use miniagent_core::types::StageId;
use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};

use crate::engine::Workflow;
use crate::stage::{ProviderSelector, Stage};
use crate::stages::{AgentStage, AnalystStage, CriticStage, GenericLlmStage, OrchestratorStage, ResearcherStage, SynthesizerStage};

fn default_flash() -> String { "flash".into() }
fn default_max_iter() -> usize { 35 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSpec {
    #[serde(default = "default_task_type")]
    pub task_type: String,
    pub stages: Vec<StageSpec>,
    #[serde(default)]
    pub edges: Vec<[String; 2]>,
}

fn default_task_type() -> String { "single_agent".into() }

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
    config: Arc<AppConfig>,
    task_dir: Option<String>,
}

impl WorkflowBuilder {
    pub fn new(agent: Arc<Agent>, config: Arc<AppConfig>) -> Self {
        Self { agent, config, task_dir: None }
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

        let key = self.config.require_deepseek_key().map_err(|e| e.to_string())?;
        let max_iterations = self.config.max_iterations;
        let max_tokens = self.config.max_tokens;

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
                        .with_limits(max_iterations, max_tokens);
                    Stage::new(&stage_spec.name, handler).with_provider(provider)
                }
                "researcher" => {
                    let handler = ResearcherStage::new(self.agent.clone())
                        .with_limits(max_iterations, max_tokens);
                    Stage::new(&stage_spec.name, handler).with_provider(provider)
                }
                "analyst" => {
                    let handler = AnalystStage::new(self.agent.clone())
                        .with_limits(max_iterations, max_tokens);
                    Stage::new(&stage_spec.name, handler).with_provider(provider)
                }
                "critic" => {
                    let p: Box<dyn miniagent_provider::traits::LlmProvider> =
                        Box::new(DeepSeekFlash::new(key));
                    Stage::new(&stage_spec.name, CriticStage::new(p, "DeepSeek Flash")).with_provider(provider)
                }
                "synthesizer" => {
                    let p: Box<dyn miniagent_provider::traits::LlmProvider> =
                        Box::new(DeepSeekPro::new(key));
                    Stage::new(&stage_spec.name, SynthesizerStage::new(p, "DeepSeek Pro")).with_provider(provider)
                }
                "orchestrator" => {
                    let handler = OrchestratorStage::new(self.agent.clone())
                        .with_limits(stage_spec.max_iterations, max_tokens);
                    Stage::new(&stage_spec.name, handler).with_provider(provider)
                }
                _ => {
                    // "llm" or unknown → GenericLlmStage
                    let p: Box<dyn miniagent_provider::traits::LlmProvider> = match stage_spec.model_tier.as_str() {
                        "pro" => Box::new(DeepSeekPro::new(key)),
                        _ => Box::new(DeepSeekFlash::new(key)),
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

        // Collect sub_tasks per stage for orchestrator stages
        let stage_sub_tasks: HashMap<String, Vec<String>> = spec.stages.iter()
            .filter(|s| s.handler_type == "orchestrator")
            .map(|s| (s.name.clone(), s.sub_tasks.clone()))
            .collect();

        // Set input
        let task_dir = self.task_dir.clone().unwrap_or_else(crate::stages::default_workflow_dir);
        wf = wf.with_input(serde_json::json!({
            "prompt": prompt,
            "system": system,
            "complexity": "moderate",
            "provider": "flash",
            "task_dir": task_dir,
            "stage_sub_tasks": stage_sub_tasks,
        }));
        wf = wf.with_task_dir(task_dir);
        wf = wf.with_config(self.config.clone());

        Ok(wf)
    }
}
