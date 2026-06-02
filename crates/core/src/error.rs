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
}
