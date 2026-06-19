use std::sync::Arc;
use async_trait::async_trait;
use miniagent_agent::Agent;
use miniagent_core::error::AgentError;
use miniagent_core::settings::AppConfig;
use miniagent_evolution::MemoryRetriever;
use miniagent_evolution::SelectionEngine;
use miniagent_tool::approval::AutoApprove;
use miniagent_tool::executor::ToolExecutor;
use miniagent_tool::tools;
use miniagent_provider::stepfun::StepFunFlash;
use miniagent_provider::traits::LlmProvider;
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
    /// Optional memory retriever for MLEvolve-inspired experience retrieval.
    /// When None (default), a NoOpRetriever is used and no retrieval occurs.
    pub memory_retriever: Option<Arc<dyn MemoryRetriever>>,
    /// Persistent SelectionEngine for cross-loop Elo rating retention.
    /// Lazily initialized when Phase 2 (tournament selection) is first enabled.
    /// Wrapped in Mutex because execute() takes &StageContext (immutable).
    pub selection_engine: std::sync::Mutex<Option<SelectionEngine>>,
}

impl StageContext {
    /// Create context with default settings (no memory retriever).
    pub fn new(task: impl Into<String>, config: Arc<AppConfig>) -> Self {
        let agent = Self::build_agent(&config);
        Self::with_agent(task, config, agent)
    }

    /// Create context with a pre-existing Agent (for testing or reuse).
    /// Use `with_memory_retriever()` afterwards to enable MLEvolve memory.
    pub fn with_agent(task: impl Into<String>, config: Arc<AppConfig>, agent: Arc<Agent>) -> Self {
        Self {
            state: PipelineState::new(task),
            messages: Vec::new(),
            config,
            agent,
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into()),
            memory_retriever: None,
            selection_engine: std::sync::Mutex::new(None),
        }
    }

    /// Attach a memory retriever to enable MLEvolve-inspired experience retrieval.
    pub fn with_memory_retriever(mut self, retriever: Arc<dyn MemoryRetriever>) -> Self {
        self.memory_retriever = Some(retriever);
        self
    }

    /// Build the shared Agent once — reused by all stages and all dispatched tasks.
    fn build_agent(config: &AppConfig) -> Arc<Agent> {
        let key = config
            .require_stepfun_key()
            .expect("STEPFUN_API_KEY required for loop pipeline");

        let flash: Box<dyn LlmProvider> = Box::new(StepFunFlash::new(key));
        // StepFun uses a single model for all roles; clone as "pro" for repair/judge stages
        let pro: Box<dyn LlmProvider> = Box::new(StepFunFlash::new(key).with_base_url(config.stepfun_base_url.clone()));
        let tool_registry = tools::defaults();
        let executor = ToolExecutor::new(tool_registry, Box::new(AutoApprove));

        let config_arc = Arc::new(config.clone());
        Arc::new(
            Agent::new(flash, pro)
                .with_tools(executor)
                .with_config(config_arc),
        )
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
