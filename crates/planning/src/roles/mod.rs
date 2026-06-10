mod proposer;
mod opponent;
mod judge;
mod researcher;
mod critic;
mod synthesizer;
mod reviewer;
mod supervisor;
mod planner;
mod executor;
mod writer;
mod evaluator;
mod observer;

pub use proposer::ProposerRole;
pub use opponent::OpponentRole;
pub use judge::JudgeRole;
pub use researcher::ResearcherRole;
pub use critic::CriticRole;
pub use synthesizer::SynthesizerRole;
pub use reviewer::ReviewerRole;
pub use supervisor::SupervisorRole;
pub use planner::PlannerRole;
pub use executor::ExecutorRole;
pub use writer::WriterRole;
pub use evaluator::EvaluatorRole;
pub use observer::ObserverRole;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

// EventStream and TodoAttention used by roles via filesystem helpers

// ── Shared output type ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleOutput {
    pub content: String,
    pub evidence: Vec<EvidenceItem>,
    pub confidence: f64,
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub output_files: Vec<String>,
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String { "success".into() }

impl RoleOutput {
    /// Build a failed output with the error preserved (Manus principle: never hide failures).
    pub fn failed(agent: &str, error: impl AsRef<str>) -> Self {
        Self {
            content: format!("[ERROR] {}", error.as_ref()),
            evidence: vec![],
            confidence: 0.0,
            metadata: {
                let mut m = HashMap::new();
                m.insert("error".into(), error.as_ref().into());
                m.insert("agent".into(), agent.into());
                m
            },
            output_files: vec![],
            status: "failed".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub claim: String,
    pub source: String,
    pub strength: f64,
    pub counter_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blackboard {
    pub work_dir: PathBuf,
    pub artifacts: HashMap<String, String>,
    pub budget: BudgetState,
    pub iteration: usize,
    pub decisions: Vec<DecisionRecord>,
    pub subscriptions: HashMap<String, Vec<String>>,
    pub write_permissions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetState {
    pub max_iterations: usize,
    pub max_tokens: usize,
    pub tokens_used: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub issuer: String,
    pub decision: String,
    pub reasoning: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Default for BudgetState {
    fn default() -> Self {
        Self { max_iterations: 50, max_tokens: 200_000, tokens_used: 0 }
    }
}

impl Default for Blackboard {
    fn default() -> Self {
        Self {
            work_dir: PathBuf::from("./miniagent_workspace"),
            artifacts: HashMap::new(),
            budget: BudgetState::default(),
            iteration: 0, decisions: Vec::new(),
            subscriptions: HashMap::new(),
            write_permissions: HashMap::new(),
        }
    }
}

impl Blackboard {
    pub fn new(work_dir: impl Into<PathBuf>) -> Self {
        let dir = work_dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self { work_dir: dir, ..Default::default() }
    }

    pub fn role_dir(&self, role: &str) -> PathBuf {
        let dir = self.work_dir.join(role);
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    /// Grant all read/write permissions to an agent (for roles that need full FS access).
    pub fn grant_full_access(&mut self, agent: &str) {
        self.write_permissions.insert(agent.to_string(), vec![]);
    }

    pub fn grant_write(&mut self, agent: &str, keys: Vec<&str>) {
        self.write_permissions.insert(agent.to_string(), keys.into_iter().map(|s| s.to_string()).collect());
    }

    pub fn can_write(&self, agent: &str, key: &str) -> bool {
        match self.write_permissions.get(agent) {
            Some(keys) if keys.is_empty() => true,
            Some(keys) => keys.contains(&key.to_string()),
            None => false,
        }
    }

    pub fn write_artifact(&mut self, agent: &str, key: impl Into<String>, value: impl Into<String>) -> Result<(), String> {
        let key = key.into();
        if !self.can_write(agent, &key) {
            return Err(format!("Agent '{agent}' lacks write permission for '{key}'"));
        }
        self.artifacts.insert(key.clone(), value.into());
        Ok(())
    }

    pub fn subscribe(&mut self, agent: &str, key: &str) {
        self.subscriptions.entry(key.to_string()).or_default().push(agent.to_string());
    }

    pub fn subscribers(&self, key: &str) -> Vec<&str> {
        self.subscriptions.get(key).map(|v| v.iter().map(|s| s.as_str()).collect()).unwrap_or_default()
    }

    pub fn has(&self, key: &str) -> bool {
        self.artifacts.get(key).is_some_and(|v| !v.is_empty())
    }

    pub fn is_new(&self, key: &str, prev_iteration: usize) -> bool {
        self.iteration > prev_iteration && self.has(key)
    }

    pub fn keys(&self) -> Vec<&str> {
        self.artifacts.keys().map(|s| s.as_str()).collect()
    }

    pub fn record_decision(&mut self, decision: DecisionRecord) {
        self.decisions.push(decision);
    }

    pub fn last_decision(&self) -> Option<&DecisionRecord> {
        self.decisions.last()
    }

    /// Record token usage from an LLM call.
    pub fn record_tokens(&mut self, tokens: usize) {
        self.budget.tokens_used += tokens;
    }

    /// Check if budget is exhausted.
    pub fn budget_exhausted(&self) -> bool {
        self.budget.tokens_used >= self.budget.max_tokens
    }
}

// ── File persistence helpers ───────────────────────────────────

pub fn persist_output(work_dir: &Path, role: &str, filename: &str, content: &str) {
    let dir = work_dir.join(role);
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(filename);
    if let Err(e) = std::fs::write(&path, content) {
        tracing::warn!("Failed to persist {}: {e}", path.display());
    }
}

pub fn load_checkpoint(work_dir: &Path, role: &str, filename: &str) -> Option<String> {
    let path = work_dir.join(role).join(filename);
    std::fs::read_to_string(&path).ok()
}

pub fn read_role_artifacts(work_dir: &Path, role: &str) -> HashMap<String, String> {
    let dir = work_dir.join(role);
    let mut artifacts = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json" || e == "md")
                && let Ok(content) = std::fs::read_to_string(&path) {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    artifacts.insert(name, content);
                }
        }
    }
    artifacts
}

pub fn load_todo(work_dir: &Path) -> String {
    std::fs::read_to_string(work_dir.join("todo.md")).unwrap_or_default()
}

pub fn save_todo(work_dir: &Path, content: &str) {
    persist_output(work_dir, "", "todo.md", content);
}

pub fn append_event(work_dir: &Path, event: &str) {
    let log_path = work_dir.join("events.log");
    let ts = chrono::Utc::now().to_rfc3339();
    let line = format!("[{ts}] {event}\n");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = f.write_all(line.as_bytes());
    }
}

// ── JSON parse helper ──────────────────────────────────────────

/// Parse LLM JSON output robustly. Returns an error message instead of
/// silently producing empty defaults.
pub fn parse_llm_json(text: &str) -> Result<serde_json::Value, String> {
    let json_str = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    if json_str.is_empty() {
        return Err("LLM returned empty response".into());
    }

    // Try direct parse first
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
        return Ok(v);
    }

    // Try to fix truncated JSON: close open strings and braces
    let mut fixed = json_str.to_string();

    // Count unclosed braces/brackets
    let mut open_curly = 0i32;
    let mut open_square = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    for ch in fixed.chars() {
        if escape_next { escape_next = false; continue; }
        if ch == '\\' { escape_next = true; continue; }
        if ch == '"' { in_string = !in_string; continue; }
        if in_string { continue; }
        match ch {
            '{' => open_curly += 1,
            '}' => open_curly -= 1,
            '[' => open_square += 1,
            ']' => open_square -= 1,
            _ => {}
        }
    }

    // Close truncated string
    if in_string {
        fixed.push('"');
    }

    // Close open brackets and braces
    for _ in 0..open_square.max(0) { fixed.push(']'); }
    for _ in 0..open_curly.max(0) { fixed.push('}'); }

    serde_json::from_str(&fixed).map_err(|e| {
        let snippet: String = json_str.chars().take(200).collect();
        format!("[ERROR] JSON parse error: {e}. Response starts with: {snippet}")
    })
}

// ── Base traits ────────────────────────────────────────────────

#[async_trait]
pub trait AgentRole: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// Execute the role's task. Takes mutable Blackboard so roles can
    /// record decisions, artifacts, and token usage.
    async fn execute(
        &self,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError>;
}

/// Extended trait: every role must be able to read/write files.
/// Prevents output loss during long-running tasks.
#[async_trait]
pub trait FileContext: AgentRole {
    fn workspace_name(&self) -> &str;

    /// List files produced by this role.
    fn list_artifacts(&self, work_dir: &Path) -> Vec<String> {
        let dir = work_dir.join(self.workspace_name());
        std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Persist output to disk. Called after every successful execute().
    fn persist(&self, work_dir: &Path, output: &RoleOutput) -> Result<(), std::io::Error> {
        let role = self.workspace_name();
        let dir = work_dir.join(role);
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(output)?;
        std::fs::write(dir.join("last_output.json"), &json)?;
        Ok(())
    }

    /// Restore the most recent output from disk (for recovery).
    fn restore(&self, work_dir: &Path) -> Option<RoleOutput> {
        let path = work_dir.join(self.workspace_name()).join("last_output.json");
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Read another role's output file.
    fn read_role_file(&self, work_dir: &Path, role: &str, filename: &str) -> Option<String> {
        load_checkpoint(work_dir, role, filename)
    }

    /// Write a file to this role's workspace.
    fn write_file(&self, work_dir: &Path, filename: &str, content: &str) -> Result<(), std::io::Error> {
        let dir = work_dir.join(self.workspace_name());
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(filename), content)
    }

    /// Read a file from this role's workspace.
    fn read_file(&self, work_dir: &Path, filename: &str) -> Option<String> {
        let path = work_dir.join(self.workspace_name()).join(filename);
        std::fs::read_to_string(&path).ok()
    }
}
