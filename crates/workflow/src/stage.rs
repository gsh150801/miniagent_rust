use async_trait::async_trait;
use miniagent_core::config::TaskComplexity;
use miniagent_core::types::StageId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageContext {
    pub stage_id: StageId,
    pub input: serde_json::Value,
    pub previous_outputs: std::collections::HashMap<StageId, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageOutput {
    pub data: serde_json::Value,
    pub metadata: StageMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageMetadata {
    pub duration_ms: u64,
    pub items_processed: usize,
    pub success: bool,
    pub error: Option<String>,
}

#[async_trait]
pub trait StageHandler: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// Execute this stage. Receives context including inputs from upstream stages.
    async fn execute(&self, ctx: &StageContext) -> Result<StageOutput, StageError>;

    /// Whether this stage is idempotent (can be safely retried)
    fn idempotent(&self) -> bool { true }
}

pub struct Stage {
    pub id: StageId,
    pub name: String,
    pub handler: std::sync::Arc<dyn StageHandler>,
    pub depends_on: Vec<StageId>,
    pub provider: ProviderSelector,
    pub parallel: usize,
    pub idempotent: bool,
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("depends_on", &self.depends_on)
            .field("provider", &self.provider)
            .field("parallel", &self.parallel)
            .field("idempotent", &self.idempotent)
            .finish()
    }
}

impl Clone for Stage {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            handler: self.handler.clone(),
            depends_on: self.depends_on.clone(),
            provider: self.provider,
            parallel: self.parallel,
            idempotent: self.idempotent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderSelector {
    Flash,
    Pro,
    Auto,
}

impl Stage {
    pub fn new(name: impl Into<String>, handler: impl StageHandler + 'static) -> Self {
        let idempotent = handler.idempotent();
        Self {
            id: StageId::new(),
            name: name.into(),
            handler: std::sync::Arc::new(handler),
            depends_on: vec![],
            provider: ProviderSelector::Auto,
            parallel: 1,
            idempotent,
        }
    }

    pub fn depends_on(mut self, stage_ids: Vec<StageId>) -> Self {
        self.depends_on = stage_ids;
        self
    }

    pub fn with_provider(mut self, provider: ProviderSelector) -> Self {
        self.provider = provider;
        self
    }

    pub fn with_parallel(mut self, n: usize) -> Self {
        self.parallel = n;
        self
    }

    pub fn task_complexity(&self) -> TaskComplexity {
        match self.provider {
            ProviderSelector::Flash => TaskComplexity::Moderate,
            ProviderSelector::Pro => TaskComplexity::Complex,
            ProviderSelector::Auto => TaskComplexity::Moderate,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StageError {
    Failed(String),
    Retryable(String),
    Skipped(String),
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageError::Failed(msg) => write!(f, "failed: {msg}"),
            StageError::Retryable(msg) => write!(f, "retryable: {msg}"),
            StageError::Skipped(msg) => write!(f, "skipped: {msg}"),
        }
    }
}
