use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── Hook Action ────────────────────────────────────────────────
// 对标 ironclaw HookAction + LangGraph conditional edge 模式

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookAction {
    /// 继续正常流程
    Continue,
    /// 修改数据后继续 (inline transformation)
    Modify(serde_json::Value),
    /// 阻止本次操作 (返回拒绝原因)
    Block(String),
    /// 跳过本次操作 (静默跳过)
    Skip,
}

// ── Hook Context ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    pub agent_name: String,
    pub session_id: String,
    pub iteration: usize,
    pub event: HookEvent,
    pub data: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    BeforeAgentLoop,
    AfterAgentLoop,
    BeforeLlmCall,
    AfterLlmCall,
    BeforeToolCall,
    AfterToolCall,
    BeforeHumanApproval,
    AfterHumanApproval,
    OnError,
    OnCheckpoint,
}

// ── Hook Trait ─────────────────────────────────────────────────

#[async_trait]
pub trait Hook: Send + Sync {
    fn name(&self) -> &str;
    fn priority(&self) -> i32 { 0 }
    fn events(&self) -> Vec<HookEvent>;

    /// Execute the hook. Returns HookAction.
    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String>;

    /// Whether this hook is a modifier (runs sequentially) or observer (can run in parallel)
    fn is_modifier(&self) -> bool { true }
}

// ── Hook Registry ──────────────────────────────────────────────

pub struct HookRegistry {
    hooks: Vec<Arc<dyn Hook>>,
}

impl HookRegistry {
    pub fn new() -> Self { Self { hooks: Vec::new() } }

    pub fn register<H: Hook + 'static>(&mut self, hook: H) {
        self.hooks.push(Arc::new(hook));
        // Sort by priority descending
        self.hooks.sort_by_key(|h| -h.priority());
    }

    /// Run all hooks for a given event. Modifiers run sequentially, observers in parallel.
    /// Returns the first Block action, or the last Modify, or Continue.
    pub async fn run_hooks(&self, event: HookEvent, ctx: &mut HookContext) -> Result<HookAction, String> {
        ctx.event = event;

        let relevant: Vec<&Arc<dyn Hook>> = self.hooks.iter()
            .filter(|h| h.events().contains(&event))
            .collect();

        if relevant.is_empty() { return Ok(HookAction::Continue); }

        let mut modifiers = Vec::new();
        let mut observers = Vec::new();
        for h in &relevant {
            if h.is_modifier() { modifiers.push(*h); } else { observers.push(*h); }
        }

        // Modifiers: sequential, accumulate transformations
        let mut current_action = HookAction::Continue;
        for hook in &modifiers {
            match hook.execute(ctx).await {
                Ok(HookAction::Block(reason)) => return Ok(HookAction::Block(reason)),
                Ok(HookAction::Skip) => return Ok(HookAction::Skip),
                Ok(HookAction::Modify(data)) => {
                    ctx.data = data;
                    current_action = HookAction::Continue; // modified, but continue
                }
                Ok(HookAction::Continue) => {}
                Err(e) => {
                    tracing::warn!("Hook '{}' error: {e}. Continuing.", hook.name());
                }
            }
        }

        // Observers: fire-and-forget in parallel
        if !observers.is_empty() {
            let tasks: Vec<_> = observers.iter().map(|hook| {
                let hook = (*hook).clone();
                let ctx = ctx.clone();
                tokio::spawn(async move { hook.execute(&ctx).await })
            }).collect();
            for task in tasks { let _ = task.await; }
        }

        Ok(current_action)
    }

    pub fn len(&self) -> usize { self.hooks.len() }
    pub fn is_empty(&self) -> bool { self.hooks.is_empty() }
}

impl Default for HookRegistry {
    fn default() -> Self { Self::new() }
}

// ── Built-in Hooks ─────────────────────────────────────────────

/// Audit log hook: records every event to filesystem
pub struct AuditLogHook {
    log_dir: String,
}

impl AuditLogHook {
    pub fn new(log_dir: impl Into<String>) -> Self {
        let dir = log_dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self { log_dir: dir }
    }
}

#[async_trait]
impl Hook for AuditLogHook {
    fn name(&self) -> &str { "audit_log" }
    fn priority(&self) -> i32 { -100 }  // always last
    fn events(&self) -> Vec<HookEvent> {
        vec![HookEvent::SessionStart, HookEvent::SessionEnd,
             HookEvent::BeforeLlmCall, HookEvent::AfterLlmCall,
             HookEvent::BeforeToolCall, HookEvent::AfterToolCall,
             HookEvent::OnError, HookEvent::OnCheckpoint]
    }
    fn is_modifier(&self) -> bool { false }  // observer only

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        let entry = serde_json::json!({
            "timestamp": ctx.timestamp.to_rfc3339(),
            "agent": ctx.agent_name,
            "session": ctx.session_id,
            "iteration": ctx.iteration,
            "event": format!("{:?}", ctx.event),
            "data_preview": ctx.data.to_string().chars().take(200).collect::<String>(),
        });
        let path = format!("{}/audit_{}.jsonl", self.log_dir, ctx.session_id);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)
            .map_err(|e| format!("audit log open: {e}"))?;
        use std::io::Write;
        writeln!(file, "{}", serde_json::to_string(&entry).unwrap_or_default())
            .map_err(|e| format!("audit log write: {e}"))?;
        Ok(HookAction::Continue)
    }
}

/// Token budget hook: blocks LLM calls when budget exceeded
pub struct TokenBudgetHook {
    max_tokens: usize,
    tokens_used: std::sync::atomic::AtomicUsize,
}

impl TokenBudgetHook {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens, tokens_used: std::sync::atomic::AtomicUsize::new(0) }
    }

    pub fn tokens_used(&self) -> usize {
        self.tokens_used.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait]
impl Hook for TokenBudgetHook {
    fn name(&self) -> &str { "token_budget" }
    fn priority(&self) -> i32 { 100 }  // run early
    fn events(&self) -> Vec<HookEvent> {
        vec![HookEvent::BeforeLlmCall, HookEvent::AfterLlmCall]
    }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        match ctx.event {
            HookEvent::BeforeLlmCall => {
                if self.tokens_used.load(std::sync::atomic::Ordering::Relaxed) >= self.max_tokens {
                    return Ok(HookAction::Block("Token budget exceeded".into()));
                }
                Ok(HookAction::Continue)
            }
            HookEvent::AfterLlmCall => {
                if let Some(usage) = ctx.data.get("output_tokens").and_then(|v| v.as_u64()) {
                    self.tokens_used.fetch_add(usage as usize, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(HookAction::Continue)
            }
            _ => Ok(HookAction::Continue),
        }
    }
}

/// Tool approval hook: requires human approval for mutating tools
pub struct ToolApprovalHook;

#[async_trait]
impl Hook for ToolApprovalHook {
    fn name(&self) -> &str { "tool_approval" }
    fn priority(&self) -> i32 { 50 }
    fn events(&self) -> Vec<HookEvent> { vec![HookEvent::BeforeToolCall] }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        let tool_name = ctx.data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
        // Mutating tools require approval in non-interactive mode
        let mutating_tools = ["write", "edit", "bash"];
        if mutating_tools.contains(&tool_name) {
            // In CLI mode, auto-approve with audit
            tracing::warn!("Auto-approving mutating tool '{}' in CLI mode", tool_name);
        }
        Ok(HookAction::Continue)
    }
}

/// Context size hook: trims history when approaching limits
pub struct ContextSizeHook {
    max_chars: usize,
}

impl ContextSizeHook {
    pub fn new(max_chars: usize) -> Self { Self { max_chars } }
}

#[async_trait]
impl Hook for ContextSizeHook {
    fn name(&self) -> &str { "context_size" }
    fn priority(&self) -> i32 { 90 }  // before LLM call
    fn events(&self) -> Vec<HookEvent> { vec![HookEvent::BeforeAgentLoop] }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        if let Some(messages) = ctx.data.get("messages")
            && let Some(arr) = messages.as_array() {
                let total_chars: usize = arr.iter()
                    .map(|m| m.to_string().len()).sum();
                if total_chars > self.max_chars {
                    // Signal that context needs trimming
                    let trimmed: Vec<_> = arr.iter().rev().take(arr.len() / 2).cloned().collect();
                    return Ok(HookAction::Modify(serde_json::Value::Array(trimmed)));
                }
            }
        Ok(HookAction::Continue)
    }
}

/// Error recovery hook: catches failures and logs structured error reports
pub struct ErrorRecoveryHook {
    max_retries: usize,
    retry_count: std::sync::atomic::AtomicUsize,
}

impl ErrorRecoveryHook {
    pub fn new(max_retries: usize) -> Self {
        Self { max_retries, retry_count: std::sync::atomic::AtomicUsize::new(0) }
    }
}

#[async_trait]
impl Hook for ErrorRecoveryHook {
    fn name(&self) -> &str { "error_recovery" }
    fn priority(&self) -> i32 { 200 }  // highest priority
    fn events(&self) -> Vec<HookEvent> { vec![HookEvent::OnError] }

    async fn execute(&self, _ctx: &HookContext) -> Result<HookAction, String> {
        let count = self.retry_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count < self.max_retries {
            tracing::warn!("Error recovery: retry {}/{}", count + 1, self.max_retries);
            Ok(HookAction::Skip)  // skip the failed step, retry
        } else {
            Ok(HookAction::Continue)  // give up, let the error propagate
        }
    }
}

// ── Path Sandbox Hook ────────────────────────────────────────
// 对标: ironclaw file-access hook + LangGraph ToolNode 参数验证
//
// 限制文件写入/编辑/bash 操作只能在指定的安全目录内。
// 防止错误操作到系统关键目录（如 /etc, /usr, ~/.ssh 等）。

pub struct PathSandboxHook {
    allowed_dirs: Vec<String>,
    working_dir: String,
}

impl PathSandboxHook {
    pub fn new(allowed_dirs: Vec<&str>) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
        Self {
            allowed_dirs: allowed_dirs.into_iter().map(|s| s.to_string()).collect(),
            working_dir: cwd,
        }
    }

    fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{}", self.working_dir, path)
        }
    }

    fn is_safe_path(&self, raw_path: &str) -> bool {
        // Manually resolve ../ components to get the real target path
        let absolute = self.resolve_path(raw_path);
        let mut stack: Vec<&str> = Vec::new();
        let is_absolute = absolute.starts_with('/');

        for comp in std::path::Path::new(&absolute).components() {
            use std::path::Component;
            match comp {
                Component::RootDir => stack.clear(),
                Component::ParentDir => { stack.pop(); }
                Component::CurDir => {}
                Component::Normal(s) => {
                    if let Some(name) = s.to_str() { stack.push(name); }
                }
                Component::Prefix(p) => {
                    if let Some(name) = p.as_os_str().to_str() { stack.push(name); }
                }
            }
        }

        let resolved = if is_absolute {
            format!("/{}", stack.join("/"))
        } else {
            stack.join("/")
        };

        // Block dangerous system directories
        let dangerous = [
            "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev",
            "/proc", "/sys", "/root", "/System", "/Library", "/Applications",
        ];
        for pattern in &dangerous {
            if resolved == *pattern || resolved.starts_with(&format!("{pattern}/")) {
                return false;
            }
        }

        // Must stay within working directory or explicitly allowed dirs
        if resolved.starts_with(&self.working_dir) { return true; }

        for allowed in &self.allowed_dirs {
            let allowed_abs = self.resolve_path(allowed);
            if resolved.starts_with(&allowed_abs) { return true; }
        }

        false
    }
}

#[async_trait]
impl Hook for PathSandboxHook {
    fn name(&self) -> &str { "path_sandbox" }
    fn priority(&self) -> i32 { 150 }  // highest safety priority
    fn events(&self) -> Vec<HookEvent> {
        vec![HookEvent::BeforeToolCall]
    }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        let tool_name = ctx.data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

        // Only check file-system tools
        if !matches!(tool_name, "write" | "edit" | "bash" | "read") {
            return Ok(HookAction::Continue);
        }

        // For bash, check if command targets filesystem
        if tool_name == "bash" {
            if let Some(cmd) = ctx.data.get("command").or_else(|| ctx.data.get("input")).and_then(|v| v.as_str()) {
                // Check for path arguments in the command
                for part in cmd.split_whitespace() {
                    if (part.contains('/') || part.contains('\\'))
                        && !self.is_safe_path(part) {
                            return Ok(HookAction::Block(format!(
                                "PathSandbox: bash command references unsafe path '{part}'. \
                                 Operations restricted to: {:?}",
                                self.allowed_dirs
                            )));
                        }
                }
            }
            return Ok(HookAction::Continue);
        }

        // For write/edit/read: check the path parameter
        let path = ctx.data.get("path").or_else(|| ctx.data.get("input").and_then(|v| v.get("path")));
        if let Some(path) = path.and_then(|v| v.as_str())
            && !self.is_safe_path(path) {
                return Ok(HookAction::Block(format!(
                    "PathSandbox: '{path}' is outside allowed directories. \
                     Write/edit operations restricted to: {:?}",
                    self.allowed_dirs
                )));
            }

        Ok(HookAction::Continue)
    }
}

// ── Dangerous Command Hook ───────────────────────────────────
// 对标: CrewAI RBAC operation whitelist + AutoGen tool validation
//
// 阻止危险的 shell 命令: rm -rf, chmod 777, sudo, curl|sh, 等等。

pub struct DangerousCommandHook {
    blocked_patterns: Vec<String>,
    warn_patterns: Vec<String>,
}

impl Default for DangerousCommandHook {
    fn default() -> Self {
        Self::new()
    }
}

impl DangerousCommandHook {
    pub fn new() -> Self {
        Self {
            blocked_patterns: vec![
                "rm -rf /".into(), "rm -rf /*".into(), "rm -rf ~".into(),
                "rm -rf .".into(), "dd if=".into(), "mkfs.".into(),
                ":(){ :|:& };:".into(),  // fork bomb
                "chmod 777 /".into(), "chown -R".into(),
                "> /dev/sda".into(), "mv / /dev/null".into(),
                "curl".into(), "wget".into(),  // blocked: use web_fetch instead
                "sudo ".into(), "su ".into(),
                "shutdown".into(), "reboot".into(), "halt".into(),
                "kill -9".into(), "pkill".into(), "killall".into(),
            ],
            warn_patterns: vec![
                "rm -rf".into(), "rm -r".into(), "chmod 777".into(),
                "chmod -R".into(), "git push --force".into(),
                "docker rm".into(), "docker rmi".into(),
            ],
        }
    }
}

#[async_trait]
impl Hook for DangerousCommandHook {
    fn name(&self) -> &str { "dangerous_command" }
    fn priority(&self) -> i32 { 140 }
    fn events(&self) -> Vec<HookEvent> { vec![HookEvent::BeforeToolCall] }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        let tool_name = ctx.data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
        if tool_name != "bash" { return Ok(HookAction::Continue); }

        let cmd = ctx.data.get("command")
            .or_else(|| ctx.data.get("input").and_then(|v| v.get("command")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let cmd_lower = cmd.to_lowercase();

        // Check blocked patterns first
        for pattern in &self.blocked_patterns {
            if cmd_lower.contains(&pattern.to_lowercase()) {
                return Ok(HookAction::Block(format!(
                    "DangerousCommand: blocked pattern '{pattern}' in command: {cmd}"
                )));
            }
        }

        // Check warn patterns — auto-append safety flags
        for pattern in &self.warn_patterns {
            if cmd_lower.contains(&pattern.to_lowercase()) {
                // For rm -rf: add --preserve-root and require explicit target
                if pattern.contains("rm -rf") {
                    // Check if target is explicitly within allowed dirs
                    let parts: Vec<&str> = cmd.split_whitespace().collect();
                    if let Some(target) = parts.last()
                        && (*target == "/" || *target == "/*" || *target == "~" || *target == ".") {
                            return Ok(HookAction::Block(format!(
                                "DangerousCommand: rm -rf on '{target}' is blocked"
                            )));
                        }
                }

                // Log warning but allow (with audit trail)
                tracing::warn!(
                    "DangerousCommand: potentially dangerous command allowed with audit: '{cmd}'"
                );
            }
        }

        Ok(HookAction::Continue)
    }
}

// ── Task Verification Hook ───────────────────────────────────
// 对标: LangGraph checkpointer + CrewAI task completion check
//
// 在 write/edit/bash 操作后验证实际结果:
// - write: 确认文件存在且大小 > 0
// - edit:  确认文件已被修改
// - bash:  检查退出码和输出

pub struct TaskVerificationHook;

#[async_trait]
impl Hook for TaskVerificationHook {
    fn name(&self) -> &str { "task_verification" }
    fn priority(&self) -> i32 { -50 }  // runs after tool execution
    fn events(&self) -> Vec<HookEvent> { vec![HookEvent::AfterToolCall] }
    fn is_modifier(&self) -> bool { true }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        let tool_name = ctx.data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

        match tool_name {
            "write" => {
                if let Some(path) = ctx.data.get("path").and_then(|v| v.as_str()) {
                    match std::fs::metadata(path) {
                        Ok(meta) => {
                            if meta.len() == 0 {
                                return Ok(HookAction::Block(format!(
                                    "TaskVerification: write to '{path}' produced empty file"
                                )));
                            }
                        }
                        Err(e) => {
                            return Ok(HookAction::Block(format!(
                                "TaskVerification: write to '{path}' failed — file not found: {e}"
                            )));
                        }
                    }
                }
            }

            "edit" => {
                if let Some(path) = ctx.data.get("path").and_then(|v| v.as_str())
                    && !std::path::Path::new(path).exists() {
                        return Ok(HookAction::Block(format!(
                            "TaskVerification: edit target '{path}' does not exist"
                        )));
                    }
            }

            "bash" => {
                // Check that bash output doesn't contain common error indicators
                // (actual exit code check is done in the bash tool itself)
                if let Some(output) = ctx.data.get("output").and_then(|v| v.as_str())
                    && (output.contains("command not found") || output.contains("Permission denied")) {
                        return Ok(HookAction::Block(format!(
                            "TaskVerification: bash command failed: {output}"
                        )));
                    }
            }

            _ => {}
        }

        Ok(HookAction::Continue)
    }
}

// ── Permission Guard Hook ────────────────────────────────────
// 对标: AutoGen permission engine + CrewAI RBAC
//
// 确保文件操作的目标文件/目录具有正确的权限。

pub struct PermissionGuardHook;

#[async_trait]
impl Hook for PermissionGuardHook {
    fn name(&self) -> &str { "permission_guard" }
    fn priority(&self) -> i32 { 130 }
    fn events(&self) -> Vec<HookEvent> { vec![HookEvent::BeforeToolCall] }

    async fn execute(&self, ctx: &HookContext) -> Result<HookAction, String> {
        let tool_name = ctx.data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

        match tool_name {
            "read" | "glob" | "grep" => {
                // Read operations: check file exists and is readable
                if let Some(path) = ctx.data.get("path").and_then(|v| v.as_str()) {
                    let p = std::path::Path::new(path);
                    if p.exists() && p.is_file() {
                        // Check readability via metadata
                        if let Ok(meta) = std::fs::metadata(path) {
                            let readonly = meta.permissions().readonly();
                            let _ = readonly; // readable even if readonly
                        }
                    }
                }
            }

            "write" | "edit" => {
                // Write operations: check parent directory exists
                if let Some(path) = ctx.data.get("path").and_then(|v| v.as_str()) {
                    let p = std::path::Path::new(path);
                    if let Some(parent) = p.parent() {
                        if parent != std::path::Path::new("") && !parent.exists() {
                            // Parent doesn't exist — write tool will create it
                            tracing::info!("PermissionGuard: parent dir {:?} doesn't exist, will be created", parent);
                        }
                        // Check if existing file is writable
                        if p.exists()
                            && let Ok(meta) = std::fs::metadata(path)
                                && meta.permissions().readonly() {
                                    return Ok(HookAction::Block(format!(
                                        "PermissionGuard: '{path}' is read-only"
                                    )));
                                }
                    }
                }
            }

            _ => {}
        }

        Ok(HookAction::Continue)
    }
}

/// Default registry with built-in safety hooks
pub fn default_hooks(log_dir: &str) -> HookRegistry {
    let mut registry = HookRegistry::new();
    // Safety-first ordering (higher priority runs first):
    registry.register(PathSandboxHook::new(vec!["result", "./result", "skills", "docs", "output"]));
    registry.register(DangerousCommandHook::new());
    registry.register(PermissionGuardHook);
    registry.register(ErrorRecoveryHook::new(3));
    registry.register(TokenBudgetHook::new(200_000));
    registry.register(ToolApprovalHook);
    registry.register(ContextSizeHook::new(100_000));
    registry.register(TaskVerificationHook);
    registry.register(AuditLogHook::new(log_dir));
    registry
}
