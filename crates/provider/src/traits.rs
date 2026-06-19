use async_trait::async_trait;
use miniagent_core::config::InferenceConfig;
use miniagent_core::error::AgentError;
use miniagent_core::event::{ContentBlock, StopReason, Usage};
use miniagent_core::message::Message;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub config: InferenceConfig,
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: Vec<ContentBlock>,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError>;

    async fn stream(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError>;
}

pub struct StreamResponse {
    pub content_receiver: tokio::sync::mpsc::Receiver<Result<StreamChunk, AgentError>>,
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    TextDelta { text: String },
    ContentBlockStart { block: ContentBlock },
    ContentBlockDelta { index: usize, text: String },
    ContentBlockStop { index: usize },
    Usage(Usage),
    Stop(StopReason),
    Error(String),
}
