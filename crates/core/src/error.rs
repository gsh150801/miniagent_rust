#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("provider error: {0}")]
    Provider(String),

    #[error("tool execution error: {tool} — {message}")]
    Tool { tool: String, message: String },

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("policy denied: {0}")]
    PolicyDenied(String),

    #[error("budget exhausted: {budget_type}")]
    BudgetExhausted { budget_type: String },

    #[error("context overflow: input {input_tokens} exceeds limit {limit_tokens}")]
    ContextOverflow {
        input_tokens: usize,
        limit_tokens: usize,
    },

    #[error("cancelled")]
    Cancelled,

    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid state: {0}")]
    InvalidState(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AgentError {
    pub fn provider(msg: impl Into<String>) -> Self {
        Self::Provider(msg.into())
    }

    pub fn tool(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            message: message.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    pub fn invalid_config(msg: impl Into<String>) -> Self {
        Self::InvalidConfig(msg.into())
    }

    pub fn invalid_state(msg: impl Into<String>) -> Self {
        Self::InvalidState(msg.into())
    }
}

/// IO 错误（如黑板产物读写失败）映射到 `Checkpoint` 变体，
/// 使 `Blackboard::put` 的 `?` 能在返回 `Result<_, AgentError>` 的调用链中自然传播。
impl From<std::io::Error> for AgentError {
    fn from(e: std::io::Error) -> Self {
        Self::Checkpoint(format!("io error: {e}"))
    }
}
