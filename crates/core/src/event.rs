use serde::{Deserialize, Serialize};

use crate::types::{MessageId, RunId, StepId, ToolCallId};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // lifecycle
    RunStarted {
        run_id: RunId,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    RunCompleted {
        run_id: RunId,
        stop_reason: StopReason,
        usage: Usage,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    RunFailed {
        run_id: RunId,
        error: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },

    // step level
    StepStarted {
        step_id: StepId,
        iteration: usize,
    },
    StepCompleted {
        step_id: StepId,
    },

    // messages
    UserMessage {
        message_id: MessageId,
        content: String,
    },
    AssistantMessage {
        message_id: MessageId,
        content: Vec<ContentBlock>,
    },

    // tool calls
    ToolCallRequested {
        call_id: ToolCallId,
        tool_name: String,
        input: serde_json::Value,
    },
    ToolCallCompleted {
        call_id: ToolCallId,
        tool_name: String,
        output: String,
        duration_ms: u64,
        is_error: bool,
    },

    // skills
    SkillInvoked {
        skill_name: String,
        trigger: String,
    },

    // subtasks (workflow orchestration)
    SubtaskStarted {
        agent: String,
        step: String,
    },
    SubtaskCompleted {
        agent: String,
        step: String,
        ok: bool,
    },

    // budget
    BudgetWarning {
        budget_type: String,
        consumed: f64,
        limit: f64,
    },

    // checkpoint
    CheckpointSaved {
        checkpoint_id: String,
        step_id: StepId,
    },

    // sub-agent (AgentTool)
    SubAgentCompleted {
        task_id: String,
        result: String,
        success: bool,
    },

    // data-analysis execution (validation plan → AnalysisRunner)
    // Captures the audit outcome of one data-analysis task: whether it
    // succeeded, whether it was a dry-run, and where its provenance record
    // lives on disk (queryable via /api/provenance/{task_id}).
    AnalysisRunCompleted {
        task_id: String,
        hypothesis_ref: Option<uuid::Uuid>,
        success: bool,
        dry_run: bool,
        provenance_path: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    Cancelled,
    BudgetExhausted,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub cache_creation_input_tokens: Option<usize>,
    pub cache_read_input_tokens: Option<usize>,
}

impl Usage {
    pub fn total_tokens(&self) -> usize {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: ToolCallId, name: String, input: serde_json::Value },
    Thinking { thinking: String, signature: Option<String> },
    RedactedThinking { data: String },
}
