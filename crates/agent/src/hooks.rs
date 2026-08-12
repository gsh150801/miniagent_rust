use serde::{Deserialize, Serialize};

/// Events that hooks can observe or intercept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    BeforeAgentLoop,
    AfterAgentLoop,
    BeforeLlmCall,
    AfterLlmCall,
    BeforeToolCall,
    AfterToolCall,
    OnError,
    OnCheckpoint,
}

/// Action returned by a hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookAction {
    Continue,
    Block(String),
    Skip,
}

/// A single registered hook. Uses a boxed async closure internally.
pub struct RegisteredHook {
    pub name: String,
    pub events: Vec<HookEvent>,
    pub(crate) func: Box<dyn Fn(HookEvent, serde_json::Value) -> HookFuture + Send + Sync>,
}

type HookFuture = std::pin::Pin<Box<dyn std::future::Future<Output = HookAction> + Send>>;

impl RegisteredHook {
    pub fn new<F, Fut>(name: &str, events: Vec<HookEvent>, f: F) -> Self
    where
        F: Fn(HookEvent, serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = HookAction> + Send + 'static,
    {
        Self {
            name: name.to_string(),
            events,
            func: Box::new(move |evt, data| Box::pin(f(evt, data))),
        }
    }
}

/// Registry of hooks.
pub struct HookRegistry {
    hooks: Vec<RegisteredHook>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register<F, Fut>(&mut self, name: &str, events: Vec<HookEvent>, f: F) -> &mut Self
    where
        F: Fn(HookEvent, serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = HookAction> + Send + 'static,
    {
        self.hooks.push(RegisteredHook::new(name, events, f));
        self
    }

    pub async fn run_hooks(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        _session_id: String,
        _iteration: usize,
    ) -> HookAction {
        for hook in &self.hooks {
            if !hook.events.contains(&event) {
                continue;
            }
            let action = (hook.func)(event, data.clone()).await;
            match &action {
                HookAction::Block(reason) => {
                    tracing::warn!("Hook '{}' blocked: {reason}", hook.name);
                    return action;
                }
                HookAction::Skip => return action,
                HookAction::Continue => {}
            }
        }
        HookAction::Continue
    }

    pub fn len(&self) -> usize { self.hooks.len() }
    pub fn is_empty(&self) -> bool { self.hooks.is_empty() }
}

impl Default for HookRegistry {
    fn default() -> Self { Self::new() }
}

/// Create a default hook registry with all built-in safety hooks.
pub fn default_hooks() -> HookRegistry {
    let max_tokens = std::env::var("TOKEN_BUDGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3_000_000);
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());

    let mut registry = HookRegistry::new();

    // Token Budget Hook
    let tokens_used: std::sync::Arc<std::sync::atomic::AtomicUsize> =
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    registry.register("token_budget", vec![HookEvent::AfterLlmCall], {
        let max = max_tokens;
        let used = tokens_used.clone();
        move |_evt, data| {
            let used = used.clone();
            async move {
                if let Some(output_tokens) = data.get("output_tokens").and_then(|v| v.as_u64()) {
                    let prev = used.fetch_add(output_tokens as usize, std::sync::atomic::Ordering::Relaxed);
                    let total = prev + output_tokens as usize;
                    if total >= max {
                        return HookAction::Block(format!(
                            "Token budget exceeded: {}/{}K tokens used",
                            total / 1000, max / 1000
                        ));
                    }
                }
                HookAction::Continue
            }
        }
    });

    let allowed_dirs: Vec<String> = vec![
        cwd.clone(),
        format!("{}/result", cwd),
        format!("{}/skills", cwd),
        format!("{}/docs", cwd),
    ];

    // Path Sandbox Hook
    let ad = allowed_dirs.clone();
    registry.register("path_sandbox", vec![HookEvent::BeforeToolCall], move |_evt, data| {
        let ad = ad.clone();
        async move {
            path_sandbox_check(&ad, &data)
        }
    });

    // Dangerous Command Hook
    registry.register("dangerous_command", vec![HookEvent::BeforeToolCall], |_evt, data| async move {
        let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
        if tool_name != "bash" { return HookAction::Continue; }
        let input = data.get("input");
        let cmd = data.get("command")
            .or_else(|| input.and_then(|v| v.get("command")))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let lower = cmd.to_lowercase();
        for pattern in &["rm -rf /", "dd if=", "mkfs.", ":(){ :|:& };:", "chmod 777 /",
            "sudo ", "shutdown", "reboot", "halt", "kill -9"] {
            if lower.contains(pattern) {
                return HookAction::Block(format!("DangerousCommand: blocked '{pattern}'"));
            }
        }
        HookAction::Continue
    });

    // Conda Safety Hook
    registry.register("conda_safety", vec![HookEvent::BeforeToolCall], |_evt, data| async move {
        let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
        if tool_name != "conda" { return HookAction::Continue; }
        let input = data.get("input");
        let action = input.and_then(|v| v.get("action")).and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(action, "install" | "remove" | "uninstall" | "clean" | "create") {
            return HookAction::Continue;
        }
        let env_name = input.and_then(|v| v.get("env_name")).and_then(|v| v.as_str()).unwrap_or("");
        if env_name.is_empty() { return HookAction::Continue; }

        // For create: enforce mn_ prefix
        if action == "create" && !env_name.starts_with("mn_") {
            return HookAction::Block(format!(
                "CondaSafety: new environments must use 'mn_' prefix. Use 'mn_{env_name}' instead."
            ));
        }

        // For modify operations: only allow if env has mn_ prefix
        if matches!(action, "install" | "remove" | "uninstall" | "clean")
            && !env_name.starts_with("mn_") {
                return HookAction::Block(format!(
                    "CondaSafety: only environments with 'mn_' prefix can be modified. \
                     '{env_name}' is a system-owned environment and is protected."
                ));
            }

        HookAction::Continue
    });

    // Git Safety Hook
    registry.register("git_safety", vec![HookEvent::BeforeToolCall], |_evt, data| async move {
        let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
        if tool_name != "git" { return HookAction::Continue; }
        let input = data.get("input");
        let repo_path = input.and_then(|v| v.get("repo_path")).and_then(|v| v.as_str()).unwrap_or("");
        if repo_path.is_empty() { return HookAction::Continue; }
        let path = std::path::Path::new(repo_path);
        if let Ok(cwd) = std::env::current_dir()
            && path.is_absolute()
                && let Ok(canon) = path.canonicalize()
                    && !canon.starts_with(&cwd) {
                        return HookAction::Block(format!(
                            "GitSafety: repo '{repo_path}' is outside working directory"
                        ));
                    }
        HookAction::Continue
    });

    // Tool Validation Hook: validates LLM tool call name + required params against schema.
    let tool_schemas: std::sync::Arc<std::collections::HashMap<String, serde_json::Value>> =
        std::sync::Arc::new(build_tool_schemas());
    registry.register("tool_validation", vec![HookEvent::BeforeToolCall], {
        let schemas = tool_schemas.clone();
        move |_evt, data| {
            let schemas = schemas.clone();
            async move {
                let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
                let input = data.get("input");

                let schema = match schemas.get(tool_name) {
                    Some(s) => s,
                    None => {
                        let known: Vec<&str> = schemas.keys().map(|s| s.as_str()).collect();
                        return HookAction::Block(format!(
                            "ToolValidation: unknown tool '{tool_name}'. Available: {}", known.join(", ")
                        ));
                    }
                };

                if let Some(required) = schema["required"].as_array() {
                    for req in required {
                        if let Some(param_name) = req.as_str() {
                            let has = input.and_then(|v| v.get(param_name)).is_some()
                                || data.get(param_name).is_some();
                            if !has {
                                return HookAction::Block(format!(
                                    "ToolValidation: tool '{tool_name}' missing required param '{param_name}'"
                                ));
                            }
                        }
                    }
                }

                HookAction::Continue
            }
        }
    });

    registry
}

/// Build a hashmap of tool_name -> input_schema (JSON schema).
/// Mirrors the tools defined in miniagent_tool::tools::defaults().
fn build_tool_schemas() -> std::collections::HashMap<String, serde_json::Value> {
    let mut m = std::collections::HashMap::new();
    // Read: path (required), offset, limit
    m.insert("read".into(), serde_json::json!({
        "required": ["path"],
        "properties": { "path": {}, "offset": {}, "limit": {} }
    }));
    // Write: path (required), content (required)
    m.insert("write".into(), serde_json::json!({
        "required": ["path", "content"],
        "properties": { "path": {}, "content": {} }
    }));
    // Edit: path (required), oldString (required), newString (required)
    m.insert("edit".into(), serde_json::json!({
        "required": ["path", "oldString", "newString"],
        "properties": { "path": {}, "oldString": {}, "newString": {} }
    }));
    // Glob: pattern (required)
    m.insert("glob".into(), serde_json::json!({
        "required": ["pattern"],
        "properties": { "pattern": {} }
    }));
    // Grep: pattern (required)
    m.insert("grep".into(), serde_json::json!({
        "required": ["pattern"],
        "properties": { "pattern": {}, "path": {}, "include": {} }
    }));
    // Bash: command (required)
    m.insert("bash".into(), serde_json::json!({
        "required": ["command"],
        "properties": { "command": {}, "timeout_ms": {}, "description": {} }
    }));
    // WebSearch: query (required)
    m.insert("web_search".into(), serde_json::json!({
        "required": ["query"],
        "properties": { "query": {}, "num": {}, "backend": {} }
    }));
    // WebFetch: url (required)
    m.insert("web_fetch".into(), serde_json::json!({
        "required": ["url"],
        "properties": { "url": {}, "max_length": {} }
    }));
    // PubMed: query (required)
    m.insert("pubmed_search".into(), serde_json::json!({
        "required": ["query"],
        "properties": { "query": {}, "max_results": {}, "offset": {}, "min_year": {} }
    }));
    // PatentSearch: query (required)
    m.insert("patent_search".into(), serde_json::json!({
        "required": ["query"],
        "properties": { "query": {}, "max_results": {}, "backend": {}, "filing_year": {}, "status": {} }
    }));
    // ClinicalTrials: query (required)
    m.insert("clinical_trials_search".into(), serde_json::json!({
        "required": ["query"],
        "properties": { "query": {}, "max_results": {}, "status": {}, "phase": {}, "study_type": {}, "min_year": {} }
    }));
    // Git: action (required), repo_path (required)
    m.insert("git".into(), serde_json::json!({
        "required": ["action", "repo_path"],
        "properties": { "action": {}, "repo_path": {}, "args": {} }
    }));
    // Conda: action (required), env_name
    m.insert("conda".into(), serde_json::json!({
        "required": ["action"],
        "properties": { "action": {}, "env_name": {}, "packages": {}, "python_version": {}, "channel": {}, "backend": {} }
    }));
    m
}

fn path_sandbox_check(allowed_dirs: &[String], data: &serde_json::Value) -> HookAction {
    let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let input = data.get("input");

    match tool_name {
        "read" | "write" | "edit" | "glob" | "grep" => {
            let path = data.get("path")
                .or_else(|| input.and_then(|v| v.get("path")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.is_empty() || is_path_safe(path, allowed_dirs) {
                HookAction::Continue
            } else {
                HookAction::Block(format!("PathSandbox: '{path}' outside allowed directories"))
            }
        }
        "git" => {
            let repo_path = data.get("repo_path")
                .or_else(|| input.and_then(|v| v.get("repo_path")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if repo_path.is_empty() || is_path_safe(repo_path, allowed_dirs) {
                HookAction::Continue
            } else {
                HookAction::Block(format!("PathSandbox: git repo '{repo_path}' outside allowed dirs"))
            }
        }
        "bash" => {
            let cmd = data.get("command")
                .or_else(|| input.and_then(|v| v.get("command")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            for part in cmd.split_whitespace() {
                if (part.contains('/') || part.starts_with('.')) && !is_path_safe(part, allowed_dirs) {
                    return HookAction::Block(format!("PathSandbox: bash references unsafe path '{part}'"));
                }
            }
            HookAction::Continue
        }
        _ => HookAction::Continue,
    }
}

fn is_path_safe(raw_path: &str, allowed_dirs: &[String]) -> bool {
    use std::path::Component;
    let path = std::path::Path::new(raw_path);
    let resolved = if path.is_relative() {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => return false,
        }
    } else {
        path.to_path_buf()
    };
    let mut stack: Vec<String> = Vec::new();
    for comp in resolved.components() {
        match comp {
            Component::RootDir => stack.clear(),
            Component::ParentDir => { stack.pop(); }
            Component::CurDir => {}
            Component::Normal(s) => { stack.push(s.to_string_lossy().to_string()); }
            Component::Prefix(p) => { stack.push(p.as_os_str().to_string_lossy().to_string()); }
        }
    }
    let resolved_str = if resolved.has_root() {
        format!("/{}", stack.join("/"))
    } else {
        stack.join("/")
    };
    // Block dangerous system directories
    let dangerous = ["/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/root",
                      "/System", "/Library", "/Applications", "/var", "/opt", "/private"];
    for d in &dangerous {
        if resolved_str == *d || resolved_str.starts_with(&format!("{d}/")) {
            return false;
        }
    }
    for allowed in allowed_dirs {
        if resolved_str.starts_with(allowed) {
            return true;
        }
    }
    false
}

// ── 外部 shell 命令钩子（参考 cc-python-claude hook_runner.py）─────────
//
// 允许用户从配置文件加载外部 shell 命令作为钩子，无需改 Rust 代码。
// 配置格式（JSON）：
// {
//   "hooks": {
//     "BeforeToolCall": [
//       {"command": "echo 'tool: '$tool_name >> /tmp/audit.log", "tool_name": "bash"},
//       "make lint"
//     ],
//     "AfterToolCall": [
//       {"command": "echo 'done: '$tool_name >> /tmp/audit.log"}
//     ]
//   }
// }
//
// 执行协议：
// - 工具上下文以 JSON 经 stdin 传入子进程
// - 退出码 0 = 放行，2 = 阻止（stdout 作为原因），超时 10s 强杀
// - 钩子故障不影响工具执行（异常/超时 → Continue）

/// 外部 shell 钩子配置。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ExternalHookConfig {
    pub command: String,
    #[serde(default)]
    pub tool_name: Option<String>,
}

/// 从 JSON 配置加载外部钩子并注册到 HookRegistry。
///
/// 配置格式：`{"hooks": {"BeforeToolCall": [...], "AfterToolCall": [...]}}`
/// 每项可以是字符串（简写，对所有工具生效）或对象 `{command, tool_name}`。
pub fn load_external_hooks(registry: &mut HookRegistry, config_json: &str) {
    let config: serde_json::Value = match serde_json::from_str(config_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to parse external hooks config");
            return;
        }
    };

    let hooks = match config.get("hooks").and_then(|v| v.as_object()) {
        Some(h) => h,
        None => return,
    };

    for (event_name, hook_list) in hooks {
        let event = match event_name.as_str() {
            "BeforeToolCall" => HookEvent::BeforeToolCall,
            "AfterToolCall" => HookEvent::AfterToolCall,
            "BeforeLlmCall" => HookEvent::BeforeLlmCall,
            "AfterLlmCall" => HookEvent::AfterLlmCall,
            "BeforeAgentLoop" => HookEvent::BeforeAgentLoop,
            "AfterAgentLoop" => HookEvent::AfterAgentLoop,
            _ => continue,
        };

        let entries = match hook_list.as_array() {
            Some(a) => a,
            None => continue,
        };

        for entry in entries {
            let (command, tool_filter) = if let Some(s) = entry.as_str() {
                (s.to_string(), None)
            } else if let Some(obj) = entry.as_object() {
                let cmd = obj.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let tn = obj.get("tool_name").and_then(|v| v.as_str()).map(|s| s.to_string());
                (cmd, tn)
            } else {
                continue;
            };

            if command.is_empty() { continue; }

            let hook_name = format!("external_{}_{}", event_name, command.len());
            let events = vec![event];
            let cmd = command.clone();
            let filter = tool_filter.clone();

            registry.register(&hook_name, events, move |_evt, data| {
                let cmd = cmd.clone();
                let filter = filter.clone();
                let data = data.clone();
                async move {
                    // 工具名过滤
                    if let Some(ref f) = filter {
                        let tn = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
                        if tn != f { return HookAction::Continue; }
                    }

                    // 执行 shell 命令，stdin 传上下文 JSON
                    let ctx_json = serde_json::to_string(&data).unwrap_or_default();
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        tokio::process::Command::new("bash")
                            .arg("-c").arg(&cmd)
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .kill_on_drop(true)
                            .output_async_with_input(ctx_json.as_bytes()),
                    ).await;

                    match result {
                        Ok(Ok(output)) => {
                            if output.status.code() == Some(2) {
                                // 退出码 2 = 阻止
                                let reason = String::from_utf8_lossy(&output.stdout)
                                    .trim().to_string();
                                HookAction::Block(if reason.is_empty() {
                                    "Blocked by external hook".into()
                                } else {
                                    reason
                                })
                            } else {
                                HookAction::Continue
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, cmd = %cmd, "external hook failed — continuing");
                            HookAction::Continue
                        }
                        Err(_) => {
                            tracing::warn!(cmd = %cmd, "external hook timed out (10s) — continuing");
                            HookAction::Continue
                        }
                    }
                }
            });
        }
    }
}

/// Trait extension for async output with stdin input.
#[async_trait::async_trait]
trait CommandExt {
    async fn output_async_with_input(&mut self, input: &[u8]) -> std::io::Result<std::process::Output>;
}

#[async_trait::async_trait]
impl CommandExt for tokio::process::Command {
    async fn output_async_with_input(&mut self, input: &[u8]) -> std::io::Result<std::process::Output> {
        use tokio::io::AsyncWriteExt;
        let mut child = self.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input).await;
            let _ = stdin.shutdown().await;
        }
        child.wait_with_output().await
    }
}
