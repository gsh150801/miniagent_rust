use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolClass {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub working_dir: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub metadata: Option<ToolMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub duration_ms: u64,
    pub is_error: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn class(&self) -> ToolClass;
    fn input_schema(&self) -> serde_json::Value;

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError>;
}
