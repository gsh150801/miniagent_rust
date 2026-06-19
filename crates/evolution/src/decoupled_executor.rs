use std::sync::Arc;

// ── Escalation Context ─────────────────────────────────────────

/// Context passed when a tactic execution escalates to strategy layer.
#[derive(Debug, Clone)]
pub struct EscalationContext {
    pub task_id: String,
    pub task_description: String,
    pub expected_output: String,
    pub failure_history: Vec<String>,
    pub consecutive_failures: usize,
}

// ── Tactic Result ──────────────────────────────────────────────

/// Result of a single tactic execution attempt.
#[derive(Debug, Clone)]
pub struct TacticResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    /// All error messages from this attempt (for escalation context).
    pub error_messages: Vec<String>,
    pub tokens_used: usize,
}
