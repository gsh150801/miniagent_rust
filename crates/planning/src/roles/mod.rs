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
use miniagent_core::message::Message;
use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
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
    /// 共享的 Agent 实例（带完整工具执行循环）。注入后，角色可通过它跑
    /// [`miniagent_agent::Agent::run_with_loop`]，获得真实工具调用能力。
    ///
    /// `#[serde(skip)]`：Agent 不可序列化，序列化时跳过；反序列化后为 `None`
    /// （语义正确——从检查点恢复的 Blackboard 需重新注入 Agent）。
    #[serde(skip)]
    pub agent: Option<std::sync::Arc<miniagent_agent::Agent>>,
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
            agent: None,
        }
    }
}

impl Blackboard {
    pub fn new(work_dir: impl Into<PathBuf>) -> Self {
        let dir = work_dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self { work_dir: dir, ..Default::default() }
    }

    /// 注入共享 Agent（带完整工具执行循环）。注入后角色可通过 [`Self::agent`]
    /// 获取它，跑 `run_with_loop` 获得真实工具调用能力。
    pub fn with_agent(mut self, agent: std::sync::Arc<miniagent_agent::Agent>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// 获取共享 Agent（若已注入）。角色在 `call_llm_with_tools` 中检查此值：
    /// `Some` → 走工具循环；`None` → 退化为单次 complete（向后兼容）。
    pub fn agent(&self) -> Option<&std::sync::Arc<miniagent_agent::Agent>> {
        self.agent.as_ref()
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

    // ── 内存优先的产物读写（write-through 黑板层）─────────────────
    //
    // 角色间通信的推荐入口。与裸文件 IO（persist_output/load_checkpoint）相比：
    // - 写入同时更新内存 artifacts + 落盘（write-through），保留持久化/可观察性；
    // - 读取优先命中内存，miss 时回退文件并缓存，消除同一文件在单次迭代内的重复磁盘读；
    // - 写入错误向上传播（替代 persist_output 吞错误的隐患）。
    //
    // key 约定 "{role}/{filename}"，落盘路径 {work_dir}/{role}/{filename}，
    // 与既有文件布局完全一致，外部工具仍可直接查看产物文件。

    /// 写入产物：更新内存 + 落盘（write-through）。
    ///
    /// `key` 形如 `"researcher/findings.json"`，按首个 `/` 分割为 role 与 filename。
    /// 不含 `/` 的 key（如 `"todo.md"`）落到 `{work_dir}/todo.md`。
    /// 返回 IO 结果——**不吞错误**，调用方应处理（通常 `?` 向上传播或记录后继续）。
    pub fn put(&mut self, key: &str, content: &str) -> std::io::Result<()> {
        validate_key(key).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let (role, filename) = split_key(key);
        let dir = self.work_dir.join(&role);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(&filename), content)?;
        // 内存层更新（即使落盘成功也更新，保证后续 get 命中内存）
        self.artifacts.insert(key.to_string(), content.to_string());
        Ok(())
    }

    /// 读取产物：优先内存，miss 时从文件回退加载并缓存。
    ///
    /// 返回内容（含缓存填充），不存在则 `None`。注意：签名要求 `&mut self`
    /// 以便 miss 时把文件内容写回 `artifacts` 缓存。
    pub fn get(&mut self, key: &str) -> Option<String> {
        // 防御性校验：含 `..` 的 key 拒绝（即使内存里有也不返回）
        if validate_key(key).is_err() {
            tracing::warn!("blackboard::get rejected unsafe key: {key}");
            return None;
        }
        if let Some(v) = self.artifacts.get(key)
            && !v.is_empty() {
                return Some(v.clone());
            }
        // 内存未命中或为空：回退文件
        let (role, filename) = split_key(key);
        let path = self.work_dir.join(&role).join(&filename);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                // 缓存以便后续命中内存
                self.artifacts.insert(key.to_string(), content.clone());
                Some(content)
            }
            Err(_) => None,
        }
    }
}

/// 把黑板 key 拆成 (role, filename)。首个 `/` 之前为 role，之后为 filename；
/// 不含 `/` 时 role 为空（文件直接落在 work_dir 根）。
fn split_key(key: &str) -> (String, String) {
    match key.split_once('/') {
        Some((role, filename)) => (role.to_string(), filename.to_string()),
        None => (String::new(), key.to_string()),
    }
}

/// 校验黑板 key 不含路径遍历段（`..`），防止 `put`/`get` 逃逸出 `work_dir`。
/// 当前所有 key 都是硬编码字面量（如 `"critic/critique.json"`），不可达；
/// 但一旦未来有角色把 LLM 输出拼进 key，此校验立即生效。返回 Err 时附说明。
fn validate_key(key: &str) -> Result<(), String> {
    // 按路径分隔符分段，检查是否有 `..` 段
    let has_traversal = key.split('/')
        .any(|seg| seg == "..");
    if has_traversal {
        Err(format!("blackboard key '{key}' contains path traversal ('..') — rejected"))
    } else {
        Ok(())
    }
}

// ── File persistence helpers ───────────────────────────────────
//
// 注意：这些裸文件 IO 函数仅供未迁移到黑板层的调用方使用。新代码应优先使用
// [`Blackboard::put`] / [`Blackboard::get`]（内存优先 + write-through 持久化）。

/// 将产物写入 `{work_dir}/{role}/{filename}`。
///
/// **deprecated（未强制）**：仅记日志、吞错误。新代码请用 [`Blackboard::put`]，
/// 它在更新内存层的同时落盘到相同路径，并把 IO 错误返回给调用方。
pub fn persist_output(work_dir: &Path, role: &str, filename: &str, content: &str) {
    let dir = work_dir.join(role);
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(filename);
    if let Err(e) = std::fs::write(&path, content) {
        tracing::warn!("Failed to persist {}: {e}", path.display());
    }
}

/// 从 `{work_dir}/{role}/{filename}` 读取产物。
///
/// **deprecated（未强制）**：每次都走磁盘。新代码请用 [`Blackboard::get`]，
/// 它优先命中内存、miss 时回退文件并缓存。
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

/// 统一的 LLM 调用入口：有共享 Agent 时走完整工具循环（`run_with_loop`），
/// 否则退化为单次 `provider.complete`（向后兼容，测试/旧路径）。
///
/// 对外契约不变：`system + prompt → 最终文本 String`。
///
/// - `agent`：从 [`Blackboard::agent`] 取得。`Some` → 角色获得真实工具调用能力
///   （LLM 可发起 ToolUse，Agent 自动执行工具并回填，循环直到完成）。
/// - `provider`：退化路径的 LLM provider（agent 为 None 时用）。
/// - `allowed_tools`：空 slice = 全部工具（`None`，不过滤）；非空 = 只暴露这些工具名。
pub async fn call_llm_with_tools(
    agent: Option<&std::sync::Arc<miniagent_agent::Agent>>,
    provider: &dyn LlmProvider,
    allowed_tools: &[String],
    system: &str,
    prompt: &str,
    cancel: CancellationToken,
) -> Result<String, AgentError> {
    if let Some(agent) = agent {
        // 走完整工具循环（复用 agent crate 的 run_with_loop）
        let mut history = vec![Message::user(prompt)];
        let mut ctx = miniagent_agent::context::RunContext::new(system)
            .with_complexity(TaskComplexity::Moderate);
        // 非空 allowed_tools 才设过滤；空 slice 表示"全部工具"（保持 None）
        if !allowed_tools.is_empty() {
            ctx = ctx.with_allowed_tools(allowed_tools.to_vec());
        }
        let delta = agent.run_with_loop(&mut history, &ctx, cancel).await?;
        // 取最终文本（工具循环产出的消息序列）
        Ok(delta.new_messages.iter().map(|m| m.text_content()).collect::<Vec<_>>().join(""))
    } else {
        // 退化路径：单次 complete（向后兼容，无工具能力）
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![Message::user(prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.2),
                max_tokens: Some(4000),
                ..Default::default()
            },
        };
        let resp = provider.complete(&request, cancel).await?;
        Ok(resp.content.iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""))
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
    let had_truncated_string = in_string;
    if in_string {
        fixed.push('"');
    }

    // Close open brackets and braces
    for _ in 0..open_square.max(0) { fixed.push(']'); }
    for _ in 0..open_curly.max(0) { fixed.push('}'); }

    match serde_json::from_str::<serde_json::Value>(&fixed) {
        Ok(v) => {
            // 启发式修复成功——记录 warn 便于追溯（LLM 输出的 JSON 不完整/结构有误，
            // 被自动补全了截断的引号/方括号/花括号）。修复可能产出语义偏移的数据。
            tracing::warn!(
                target: "planning::parse_llm_json",
                unclosed_curly = open_curly.max(0),
                unclosed_square = open_square.max(0),
                truncated_string = had_truncated_string,
                "LLM JSON was malformed and auto-repaired (added closing delimiters)"
            );
            Ok(v)
        }
        Err(e) => {
            let snippet: String = json_str.chars().take(200).collect();
            Err(format!("[ERROR] JSON parse error: {e}. Response starts with: {snippet}"))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_blackboard(tag: &str) -> Blackboard {
        let dir = std::env::temp_dir().join(format!(
            "miniagent_bb_test_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Blackboard::new(&dir)
    }

    #[test]
    fn split_key_handles_role_and_root() {
        assert_eq!(split_key("researcher/findings.json"), ("researcher".into(), "findings.json".into()));
        assert_eq!(split_key("todo.md"), ("".into(), "todo.md".into()));
    }

    #[test]
    fn blackboard_put_then_get_roundtrip() {
        let mut bb = tmp_blackboard("roundtrip");
        bb.put("researcher/findings.json", r#"{"summary":"ok"}"#).unwrap();

        let got = bb.get("researcher/findings.json").unwrap();
        assert_eq!(got, r#"{"summary":"ok"}"#);
        // 内存层已缓存
        assert_eq!(bb.artifacts.get("researcher/findings.json").unwrap(), r#"{"summary":"ok"}"#);
    }

    #[test]
    fn blackboard_get_falls_back_to_file() {
        let bb = tmp_blackboard("fallback");
        // 直接写文件（绕过 put，模拟外部/旧数据写入），内存层应为空
        let path = bb.work_dir.join("planner").join("current_plan.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "plan-v1").unwrap();
        assert!(bb.artifacts.is_empty());

        let mut bb = bb; // get 需要 &mut self 以便回填缓存
        let got = bb.get("planner/current_plan.json").unwrap();
        assert_eq!(got, "plan-v1");
        // 回退读取后应缓存进内存
        assert_eq!(bb.artifacts.get("planner/current_plan.json").unwrap(), "plan-v1");
    }

    #[test]
    fn blackboard_get_returns_none_for_missing() {
        let mut bb = tmp_blackboard("missing");
        assert!(bb.get("nonexistent/file.json").is_none());
    }

    #[test]
    fn blackboard_put_writes_to_expected_file_path() {
        let bb = tmp_blackboard("writepath");
        let expected_path = bb.work_dir.join("critic").join("critique.json");

        let mut bb = bb;
        bb.put("critic/critique.json", "critique-body").unwrap();

        assert!(expected_path.exists(), "file should be persisted to {{work_dir}}/{{role}}/{{filename}}");
        assert_eq!(std::fs::read_to_string(&expected_path).unwrap(), "critique-body");
    }

    #[test]
    fn blackboard_put_propagates_io_error() {
        // 用一个无法创建子目录的 work_dir（将文件路径指向一个已存在的文件作为"目录"）
        // 触发 create_dir_all 失败 → put 应返回 Err（而非吞掉）。
        let bb = tmp_blackboard("ioerror");
        // 制造冲突：先创建一个普通文件，再用它作为 role 目录前缀
        let conflict = bb.work_dir.join("conflict_role");
        std::fs::write(&conflict, "i-am-a-file").unwrap();

        let mut bb = bb;
        let result = bb.put("conflict_role/file.json", "x");
        assert!(result.is_err(), "put must propagate IO errors instead of swallowing them");
    }

    #[test]
    fn validate_key_rejects_path_traversal() {
        assert!(validate_key("../etc/passwd").is_err());
        assert!(validate_key("a/../../b").is_err());
        assert!(validate_key("role/../../../etc/cron.d/x").is_err());
        // 合法 key 应通过
        assert!(validate_key("critic/critique.json").is_ok());
        assert!(validate_key("todo.md").is_ok());
        assert!(validate_key("researcher/findings.json").is_ok());
    }

    #[test]
    fn blackboard_put_rejects_traversal_key() {
        let mut bb = tmp_blackboard("traversal");
        let result = bb.put("../escape.txt", "malicious");
        assert!(result.is_err(), "put with '..' key must be rejected");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn blackboard_get_rejects_traversal_key() {
        let mut bb = tmp_blackboard("traversal_get");
        // 即使内存里有恶意 key 也不返回
        bb.artifacts.insert("../secret".into(), "leaked".into());
        assert!(bb.get("../secret").is_none(), "get with '..' key must return None");
    }

    #[tokio::test]
    async fn blackboard_agent_is_none_by_default_and_serde_skipped() {
        // 默认无 agent（tag 用不含 "agent" 的名字，避免 work_dir 值干扰断言）
        let bb = tmp_blackboard("bb_serde");
        assert!(bb.agent().is_none(), "agent should be None by default");

        // serialize → deserialize 后 agent 应为 None（#[serde(skip)] 验证）
        let json = serde_json::to_string(&bb).unwrap();
        assert!(!json.contains("\"agent\""), "serialized blackboard must not contain agent field key");
        let restored: Blackboard = serde_json::from_str(&json).unwrap();
        assert!(restored.agent().is_none(), "deserialized blackboard agent must be None");
    }

    #[tokio::test]
    async fn call_llm_with_tools_falls_back_to_provider_when_no_agent() {
        // agent=None → 走单次 complete 退化路径（用 MockProvider 验证返回文本）
        let provider = miniagent_provider::MockProvider::new("test");
        let result = call_llm_with_tools(
            None,                       // 无 Agent → 退化路径
            &provider,
            &[],                         // 全部工具（不过滤）
            "You are a test assistant.",
            "Say hello.",
            tokio_util::sync::CancellationToken::new(),
        ).await;
        assert!(result.is_ok(), "fallback path should succeed with MockProvider");
        let text = result.unwrap();
        // MockProvider 返回非空文本
        assert!(!text.is_empty(), "fallback should return provider's text response");
    }
}
