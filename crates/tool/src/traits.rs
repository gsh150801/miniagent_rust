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

#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: String,
    pub session_id: String,
    /// 用户交互处理器（参考 cc-python-claude ask_user 的 input_fn 依赖注入）。
    /// 不可序列化（Arc<dyn UserPrompt>），ToolContext 已不 derive Serialize。
    pub user_prompt: std::sync::Arc<dyn UserPrompt>,
}

impl ToolContext {
    /// 创建带 NoUserPrompt 的 ToolContext（向后兼容，非交互模式）。
    pub fn new(working_dir: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
            session_id: session_id.into(),
            user_prompt: std::sync::Arc::new(NoUserPrompt),
        }
    }

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

/// 用户交互 trait（参考 cc-python-claude ask_user_tool 的 input_fn 依赖注入）。
///
/// 允许工具向用户提问并等待回复。实现解耦输入来源：
/// - CLI 实现从 stdin 读取
/// - server 实现通过 WebSocket 等待前端回复（`ask_user` oneshot）
/// - 非交互实现返回 None（CI/管道模式）
#[async_trait::async_trait]
pub trait UserPrompt: Send + Sync {
    /// 向用户提问，返回用户的回答（None = 无法提问/用户未回答）
    async fn ask(&self, question: &str) -> Option<String>;
}

/// 非交互式用户提问实现（总是返回 None，用于 CI/管道模式）。
pub struct NoUserPrompt;

#[async_trait::async_trait]
impl UserPrompt for NoUserPrompt {
    async fn ask(&self, _question: &str) -> Option<String> {
        None
    }
}
