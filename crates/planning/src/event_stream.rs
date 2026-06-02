use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Typed event for the agent event stream.
/// Every agent action is recorded as a typed event, enabling:
/// - Cross-agent awareness without in-memory state coupling
/// - Context compression (replace details with event summaries)
/// - Audit trail for debugging long-running tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub agent: String,
    pub kind: EventKind,
    pub details: String,
    pub file_refs: Vec<String>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventKind {
    TaskStarted,
    TaskCompleted,
    TaskFailed,
    ToolInvoked,
    ToolResult,
    OutputProduced,
    CheckpointSaved,
    ContextCompressed,
    PlanCreated,
    PlanUpdated,
    ReviewCompleted,
    EvaluationCompleted,
    IterationStarted,
    ErrorRecovery,
}

/// In-memory event stream with disk persistence.
/// Append-only for KV-cache friendliness.
#[derive(Debug, Clone)]
pub struct EventStream {
    events: Vec<AgentEvent>,
    log_path: PathBuf,
    max_in_memory: usize,
}

impl EventStream {
    pub fn new(work_dir: &Path) -> Self {
        let log_path = work_dir.join("events.jsonl");
        let mut stream = Self {
            events: Vec::new(),
            log_path,
            max_in_memory: 500,
        };
        stream.load_from_disk();
        stream
    }

    pub fn with_max_events(mut self, max: usize) -> Self {
        self.max_in_memory = max;
        self
    }

    /// Append an event. Writes to both memory and disk (append-only).
    pub fn push(&mut self, event: AgentEvent) {
        // Append to disk (JSONL format — one JSON per line)
        if let Ok(line) = serde_json::to_string(&event) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true).append(true).open(&self.log_path)
            {
                let _ = writeln!(f, "{line}");
            }
        }

        // Keep in memory (bounded)
        self.events.push(event);
        if self.events.len() > self.max_in_memory {
            self.events.drain(..self.events.len() - self.max_in_memory);
        }
    }

    /// Convenience: create and push a task-started event.
    pub fn task_started(&mut self, agent: &str, task: &str) {
        self.push(AgentEvent {
            timestamp: chrono::Utc::now(),
            agent: agent.into(),
            kind: EventKind::TaskStarted,
            details: task.into(),
            file_refs: vec![],
            success: true,
        });
    }

    /// Convenience: create and push a task-completed event.
    pub fn task_completed(&mut self, agent: &str, summary: &str, files: Vec<String>) {
        self.push(AgentEvent {
            timestamp: chrono::Utc::now(),
            agent: agent.into(),
            kind: EventKind::TaskCompleted,
            details: summary.into(),
            file_refs: files,
            success: true,
        });
    }

    /// Convenience: create and push a task-failed event.
    pub fn task_failed(&mut self, agent: &str, error: &str) {
        self.push(AgentEvent {
            timestamp: chrono::Utc::now(),
            agent: agent.into(),
            kind: EventKind::TaskFailed,
            details: error.into(),
            file_refs: vec![],
            success: false,
        });
    }

    /// Convenience: tool invocation event.
    pub fn tool_invoked(&mut self, agent: &str, tool: &str, input_summary: &str) {
        self.push(AgentEvent {
            timestamp: chrono::Utc::now(),
            agent: agent.into(),
            kind: EventKind::ToolInvoked,
            details: format!("{tool}: {input_summary}"),
            file_refs: vec![],
            success: true,
        });
    }

    /// Convenience: checkpoint saved event.
    pub fn checkpoint_saved(&mut self, agent: &str, path: &str) {
        self.push(AgentEvent {
            timestamp: chrono::Utc::now(),
            agent: agent.into(),
            kind: EventKind::CheckpointSaved,
            details: format!("checkpoint: {path}"),
            file_refs: vec![path.into()],
            success: true,
        });
    }

    /// Get recent events, optionally filtered by agent name.
    pub fn recent(&self, count: usize, agent_filter: Option<&str>) -> Vec<&AgentEvent> {
        let filtered: Vec<&AgentEvent> = self.events.iter()
            .filter(|e| agent_filter.is_none_or(|f| e.agent == f))
            .rev()
            .take(count)
            .collect();
        filtered.into_iter().rev().collect()
    }

    /// Get events relevant to a specific role.
    /// A role sees: its own events + events from agents it depends on.
    pub fn relevant_to(&self, role: &str, count: usize) -> Vec<&AgentEvent> {
        let dependents = role_dependencies(role);
        self.events.iter()
            .filter(|e| e.agent == role || dependents.contains(&e.agent.as_str()))
            .rev()
            .take(count)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Format recent events as a text block for inclusion in prompts.
    /// KV-cache friendly: stable format, append-only.
    pub fn format_recent(&self, count: usize, agent_filter: Option<&str>) -> String {
        let events = self.recent(count, agent_filter);
        if events.is_empty() {
            return "(no recent events)".into();
        }
        events.iter().map(|e| {
            let ts = e.timestamp.format("%H:%M:%S");
            let status = if e.success { "OK" } else { "FAIL" };
            format!("[{ts}] [{status}] {}: {}", e.agent, e.details)
        }).collect::<Vec<_>>().join("\n")
    }

    /// Count events by kind.
    pub fn count_by_kind(&self, kind: EventKind) -> usize {
        self.events.iter().filter(|e| e.kind == kind).count()
    }

    /// Count events for a specific agent.
    pub fn count_for_agent(&self, agent: &str) -> usize {
        self.events.iter().filter(|e| e.agent == agent).count()
    }

    /// Total event count (in memory).
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Iterate over all in-memory events.
    pub fn iter(&self) -> impl Iterator<Item = &AgentEvent> {
        self.events.iter()
    }

    /// Load events from disk (JSONL format).
    fn load_from_disk(&mut self) {
        if let Ok(content) = std::fs::read_to_string(&self.log_path) {
            for line in content.lines().rev().take(self.max_in_memory) {
                if let Ok(event) = serde_json::from_str::<AgentEvent>(line) {
                    self.events.push(event);
                }
            }
            // Reverse to get chronological order
            self.events.reverse();
        }
    }

    /// Compute total wall-clock duration from first to last event.
    pub fn total_duration(&self) -> Option<Duration> {
        let first = self.events.first()?;
        let last = self.events.last()?;
        last.timestamp.signed_duration_since(first.timestamp).to_std().ok()
    }
}

/// Define which agents a given role depends on for context.
fn role_dependencies(role: &str) -> Vec<&'static str> {
    match role {
        "supervisor" => vec!["planner", "evaluator"],
        "planner" => vec!["supervisor", "evaluator"],
        "researcher" => vec!["supervisor", "planner"],
        "critic" => vec!["researcher"],
        "synthesizer" => vec!["researcher", "critic"],
        "executor" => vec!["supervisor", "planner"],
        "writer" => vec!["researcher", "critic", "synthesizer", "reviewer"],
        "reviewer" => vec!["researcher", "critic", "synthesizer", "writer"],
        "evaluator" => vec!["reviewer", "writer", "synthesizer"],
        "observer" => vec![],  // observer sees everything
        "proposer" => vec!["researcher", "opponent", "judge"],
        "opponent" => vec!["proposer", "judge"],
        "judge" => vec!["proposer", "opponent"],
        _ => vec![],
    }
}
