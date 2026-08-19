use std::sync::Arc;
use async_trait::async_trait;
use miniagent_agent::Agent;
use miniagent_core::error::AgentError;
use miniagent_core::models::ModelRegistry;
use miniagent_core::settings::AppConfig;
use miniagent_tool::approval::AutoApprove;
use miniagent_tool::executor::ToolExecutor;
use miniagent_tool::tools;
use miniagent_provider::factory::{self, ProviderTier};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use crate::types::*;

/// Shared context for all pipeline stages.
///
/// Holds a single `Arc<Agent>` built once at pipeline start — all stages
/// reuse the same HTTP connection pool and ToolRegistry instead of
/// rebuilding them per task.
pub struct StageContext {
    pub state: PipelineState,
    pub messages: Vec<StageMessage>,
    pub config: Arc<AppConfig>,
    pub agent: Arc<Agent>,
    pub working_dir: String,
}

impl StageContext {
    /// Create context with default settings.
    pub fn new(task: impl Into<String>, config: Arc<AppConfig>) -> Self {
        let agent = Self::build_agent(&config);
        Self::with_agent(task, config, agent)
    }

    /// Create context with a pre-existing Agent (for testing or reuse).
    pub fn with_agent(task: impl Into<String>, config: Arc<AppConfig>, agent: Arc<Agent>) -> Self {
        Self {
            state: PipelineState::new(task),
            messages: Vec::new(),
            config,
            agent,
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into()),
        }
    }

    /// Build the shared Agent once — reused by all stages and all dispatched tasks.
    ///
    /// Provider construction goes through the runtime model registry +
    /// [`factory::build_provider`] (single source of truth, DeepSeek-harness
    /// lesson): the active `ModelProfile` — env built-ins or a custom
    /// `models.json` entry — decides flash/pro, including OpenAI/Anthropic
    /// compatible endpoints. The old per-provider if/else branches here
    /// silently fell back to DeepSeek (or panicked) under custom profiles.
    fn build_agent(config: &AppConfig) -> Arc<Agent> {
        let registry = ModelRegistry::load(config);
        let profile = registry.active();
        let flash = factory::build_provider(profile, ProviderTier::Flash)
            .unwrap_or_else(|e| panic!("loop pipeline: active model profile unusable: {e}"));
        let pro = factory::build_provider(profile, ProviderTier::Pro)
            .unwrap_or_else(|e| panic!("loop pipeline: active model profile unusable: {e}"));

        let tool_registry = tools::defaults();
        let executor = ToolExecutor::new(tool_registry, Box::new(AutoApprove));

        let config_arc = Arc::new(config.clone());
        Arc::new(
            Agent::new(flash, pro)
                .with_tools(executor)
                .with_config(config_arc),
        )
    }

    /// Anchor all tool execution to `dir` (the task's result directory).
    ///
    /// Dispatched agents run bash/write with this as their working directory,
    /// so relative-path artifacts land inside the task dir; dispatch-stage
    /// persistence and `outputs_still_exist` checks use the same base.
    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = dir.into();
        self
    }

    pub fn with_max_loops(mut self, n: usize) -> Self {
        self.state.max_loops = n;
        self
    }

    /// Collect stage messages: log them for observability and accumulate.
    ///
    /// The pipeline calls this after each stage executes, so the routing
    /// information (from_stage → to_stage) is visible in structured logs.
    pub fn collect_messages(&mut self, messages: Vec<StageMessage>) {
        for msg in &messages {
            let preview: String = msg.content.chars().take(200).collect();
            tracing::debug!(
                from = %msg.from_stage,
                to = %msg.to_stage,
                task_id = ?msg.task_id,
                "Stage message: {preview}",
            );
        }
        self.messages.extend(messages);
    }
}

/// Output from a pipeline stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageOutput {
    pub updated_state: PipelineState,
    pub new_messages: Vec<StageMessage>,
    pub summary: String,
}

/// All stages in the loop pipeline implement this trait
#[async_trait]
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &str;

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError>;
}
