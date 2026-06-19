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
        if matches!(action, "install" | "remove" | "uninstall" | "clean") {
            if !env_name.starts_with("mn_") {
                return HookAction::Block(format!(
                    "CondaSafety: only environments with 'mn_' prefix can be modified. \
                     '{env_name}' is a system-owned environment and is protected."
                ));
            }
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
        if let Ok(cwd) = std::env::current_dir() {
            if path.is_absolute() {
                if let Ok(canon) = path.canonicalize() {
                    if !canon.starts_with(&cwd) {
                        return HookAction::Block(format!(
                            "GitSafety: repo '{repo_path}' is outside working directory"
                        ));
                    }
                }
            }
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
