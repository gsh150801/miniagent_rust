use async_trait::async_trait;
use crate::traits::ToolClass;

/// 权限决策（参考 cc-python-claude 的 PermissionDecision）。
#[derive(Debug)]
pub enum ApprovalDecision {
    /// 直接执行
    Allow,
    /// 需要询问用户是否允许（交互模式下弹窗/推送，非交互模式下拒绝）
    Ask(String),
    /// 直接拒绝
    Deny(String),
}

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn approve(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        class: ToolClass,
    ) -> ApprovalDecision;
}

/// Auto-approve all tool calls (full trust).
pub struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn approve(&self, _name: &str, _input: &serde_json::Value, _class: ToolClass) -> ApprovalDecision {
        ApprovalDecision::Allow
    }
}

/// Approve read-only tools, deny all mutating tools.
pub struct ReadOnlyAutoApprove;

#[async_trait]
impl ApprovalHandler for ReadOnlyAutoApprove {
    async fn approve(&self, _name: &str, _input: &serde_json::Value, class: ToolClass) -> ApprovalDecision {
        match class {
            ToolClass::ReadOnly => ApprovalDecision::Allow,
            ToolClass::Mutating => ApprovalDecision::Deny("Mutating tools require user approval".into()),
        }
    }
}

/// 只读工具白名单（参考 cc-python-claude 的 READ_ONLY_TOOLS）。
/// 这些工具不修改文件系统，在所有模式下都自动允许。
pub const READ_ONLY_TOOLS: &[&str] = &[
    "read", "glob", "grep", "web_search", "web_fetch",
    "pubmed_search", "patent_search", "clinical_trials_search",
];

/// 编辑工具集（参考 cc-python-claude 的 EDIT_TOOLS）。
/// 在 AcceptEdits 模式下自动允许。
pub const EDIT_TOOLS: &[&str] = &["write", "edit"];

/// 白名单权限模式（参考 cc-python-claude 的 PermissionMode + PermissionContext）。
///
/// 三级模式：
/// - `Bypass`：全放行（危险，仅受信环境）
/// - `AcceptEdits`：只读+编辑自动允许，bash/git/conda 等需 Ask
/// - `Default`：只只读自动允许，其余 Ask
/// - `NonInteractive`：同 Default 但 Ask 直接 Deny（fail-fast，用于后台/无人值守）
pub struct WhitelistApproval {
    pub mode: WhitelistMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhitelistMode {
    Bypass,
    AcceptEdits,
    Default,
    /// 非交互模式：遇 Ask 直接 Deny（fail-fast）
    NonInteractive,
    /// 计划模式：只允许只读工具，所有写操作 Deny（让用户先审查计划再执行）
    PlanOnly,
}

impl WhitelistApproval {
    pub fn new(mode: WhitelistMode) -> Self {
        Self { mode }
    }

    pub fn interactive() -> Self {
        Self::new(WhitelistMode::AcceptEdits)
    }

    pub fn non_interactive() -> Self {
        Self::new(WhitelistMode::NonInteractive)
    }
}

#[async_trait]
impl ApprovalHandler for WhitelistApproval {
    async fn approve(&self, tool_name: &str, _input: &serde_json::Value, class: ToolClass) -> ApprovalDecision {
        // Bypass：全放行
        if self.mode == WhitelistMode::Bypass {
            return ApprovalDecision::Allow;
        }

        // 只读工具白名单：所有模式都放行
        if READ_ONLY_TOOLS.contains(&tool_name) || class == ToolClass::ReadOnly {
            return ApprovalDecision::Allow;
        }

        // PlanOnly 模式：只允许只读，所有写操作 Deny
        if self.mode == WhitelistMode::PlanOnly {
            return ApprovalDecision::Deny(format!(
                "tool '{}' denied in plan-only mode — review the plan first, then switch to Default mode to execute",
                tool_name
            ));
        }

        // 编辑工具：AcceptEdits 模式放行
        if EDIT_TOOLS.contains(&tool_name) && self.mode == WhitelistMode::AcceptEdits {
            return ApprovalDecision::Allow;
        }

        // 其余工具（bash/git/conda 等）：Ask
        let ask_msg = format!("Tool '{}' requires user approval", tool_name);
        match self.mode {
            WhitelistMode::NonInteractive => {
                // 非交互模式：Ask → Deny（fail-fast）
                tracing::error!(tool = tool_name, "permission denied (non-interactive mode)");
                ApprovalDecision::Deny(format!("{} — denied in non-interactive mode", ask_msg))
            }
            _ => ApprovalDecision::Ask(ask_msg),
        }
    }
}

/// Security-enforcing approval handler.
///
/// Rules:
/// 1. All delete operations (write, edit, bash rm, fs operations with remove/delete)
///    require explicit user approval — they can only delete files within task_dir.
/// 2. Git tool cannot operate outside task_dir.
/// 3. Conda tool cannot modify system conda environments.
pub struct SecureApproval {
    pub task_dir: String,
}

impl SecureApproval {
    pub fn new(task_dir: impl Into<String>) -> Self {
        Self { task_dir: task_dir.into() }
    }
}

#[async_trait]
impl ApprovalHandler for SecureApproval {
    async fn approve(&self, tool_name: &str, input: &serde_json::Value, class: ToolClass) -> ApprovalDecision {
        // ---- Rule 1: File deletion requires explicit approval ----
        // Check for destructive operations:
        // - write tool with content that could be destructive
        // - bash tool with rm, rmdir, del, remove, delete, wipe, shred, mv
        // - edit tool
        if class == ToolClass::Mutating {
            if tool_name == "bash"
                && let Some(cmd) = input["command"].as_str() {
                    let cmd_lower = cmd.to_lowercase();
                    let destructive_keywords = [
                        " rm ", " rm -rf ", " rmdir ", " del ", " rd ", " remove-item ",
                        " wipe ", " shred ", " mv ", " truncate ", " dd ",
                    ];
                    let is_destructive = destructive_keywords.iter()
                        .any(|kw| cmd_lower.contains(kw));
                    if is_destructive {
                        return ApprovalDecision::Deny(
                            "File deletion/modification requires explicit user approval. \
                             Use 'write' or 'edit' tools for file operations instead, \
                             or get explicit approval before using rm/mv/del.".into()
                        );
                    }
                }

            if tool_name == "write" {
                // Writing empty content or very short content is suspicious
                if let Some(content) = input["content"].as_str()
                    && content.trim().is_empty() {
                        return ApprovalDecision::Deny(
                            "Writing empty content requires explicit approval.".into()
                        );
                    }
            }

            if tool_name == "edit" {
                // Editing files is potentially destructive
                // For edit, we check if it's replacing content with empty
                if let Some(old) = input["oldString"].as_str() {
                    let new = input["newString"].as_str().unwrap_or("");
                    if !old.is_empty() && new.is_empty() {
                        return ApprovalDecision::Deny(
                            "Deleting content via 'edit' (replacing with empty) requires explicit approval.".into()
                        );
                    }
                }
            }
        }

        // ---- Rule 2: Git tool cannot operate outside task_dir ----
        if tool_name == "git"
            && let Some(repo_path) = input["repo_path"].as_str() {
                let repo_dir = std::path::Path::new(repo_path);
                let task_dir = std::path::Path::new(&self.task_dir);
                // If both exist and repo is outside task_dir, deny
                if repo_dir.is_absolute() && task_dir.exists() {
                    let repo_canon = std::fs::canonicalize(repo_dir).ok();
                    let task_canon = std::fs::canonicalize(task_dir).ok();
                    if let (Some(repo_c), Some(task_c)) = (repo_canon, task_canon)
                        && !repo_c.starts_with(&task_c) {
                            return ApprovalDecision::Deny(format!(
                                "Git operations on '{}' are outside the task directory '{}'.",
                                repo_c.display(), task_c.display()
                            ));
                        }
                }
            }

        // ---- Rule 3: Conda tool cannot modify system environments ----
        if tool_name == "conda"
            && let Some(action) = input["action"].as_str() {
                let modifying_actions = ["install", "remove", "uninstall", "clean", "create"];
                if modifying_actions.contains(&action)
                    && let Some(env_name) = input["env_name"].as_str()
                        && crate::security::is_system_conda_path(env_name) {
                            return ApprovalDecision::Deny(format!(
                                "Modifying system conda environment '{}' is not allowed. \
                                 Create new environments within the working directory.",
                                env_name
                            ));
                        }
            }

        ApprovalDecision::Allow
    }
}

// ── 权限规则配置（参考 cc-python-claude permissions/rules.py）──────────────
//
// 用户通过 settings.json 配置 allow/deny 规则：
// {
//   "permissions": {
//     "allow": ["read", "glob", "grep", "bash:git*"],
//     "deny": ["bash:rm*"]
//   }
// }
//
// 规则语法：
// - "ToolName" — 精确匹配工具名
// - "bash:pattern" — 匹配 bash 工具且 command 符合 glob 模式
//
// 优先级：deny > allow > 无匹配（fallthrough 到模式检查）

/// 用户可配置的权限规则。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PermissionRules {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl PermissionRules {
    /// 从 JSON 文件加载权限规则。
    ///
    /// 防御式解析：文件不存在、JSON 格式错误都返回空规则（不影响正常使用）。
    pub fn load(path: &std::path::Path) -> Self {
        if !path.is_file() {
            return Self::default();
        }
        match std::fs::read_to_string(path) {
            Ok(content) => Self::from_json_str(&content),
            Err(_) => Self::default(),
        }
    }

    /// 从 JSON 字符串解析权限规则。
    pub fn from_json_str(json: &str) -> Self {
        match serde_json::from_str::<serde_json::Value>(json) {
            Ok(v) => {
                let perms = v.get("permissions").unwrap_or(&serde_json::Value::Null);
                let allow = perms.get("allow")
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let deny = perms.get("deny")
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                Self { allow, deny }
            }
            Err(_) => Self::default(),
        }
    }

    /// 检查规则是否匹配某个工具调用。
    ///
    /// 返回：Some(true) = allow, Some(false) = deny, None = 无匹配。
    /// 优先级：deny > allow。
    pub fn check(&self, tool_name: &str, input: &serde_json::Value) -> Option<bool> {
        // 先检查 deny（deny 优先）
        for rule in &self.deny {
            if matches_rule(rule, tool_name, input) {
                return Some(false);
            }
        }
        // 再检查 allow
        for rule in &self.allow {
            if matches_rule(rule, tool_name, input) {
                return Some(true);
            }
        }
        None
    }
}

/// 检查单条规则是否匹配（参考 cc-python-claude _matches_rule）。
///
/// "ToolName" — 精确匹配工具名
/// "bash:pattern" — 匹配工具名 "bash" 且 command 参数符合 glob 模式
fn matches_rule(rule: &str, tool_name: &str, input: &serde_json::Value) -> bool {
    if let Some((rule_tool, pattern)) = rule.split_once(':') {
        // 冒号分隔：工具名:命令模式
        if tool_name != rule_tool {
            return false;
        }
        // 从 input 提取 command 字段做 glob 匹配
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        return glob_matches(command, pattern);
    }
    // 简单规则：工具名精确匹配
    tool_name == rule
}

/// 简易 glob 匹配（支持 * 通配符）。
fn glob_matches(text: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return text.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return text.ends_with(suffix);
    }
    text == pattern
}

/// 基于规则的权限 handler（在 WhitelistApproval 之上叠加用户配置规则）。
///
/// 先检查用户规则（deny > allow），无匹配时 fallthrough 到内部 handler。
pub struct RuleBasedApproval {
    pub rules: PermissionRules,
    pub inner: Box<dyn ApprovalHandler>,
}

impl RuleBasedApproval {
    pub fn new(rules: PermissionRules, inner: Box<dyn ApprovalHandler>) -> Self {
        Self { rules, inner }
    }
}

#[async_trait]
impl ApprovalHandler for RuleBasedApproval {
    async fn approve(&self, tool_name: &str, input: &serde_json::Value, class: ToolClass) -> ApprovalDecision {
        // 先检查用户配置规则
        match self.rules.check(tool_name, input) {
            Some(true) => return ApprovalDecision::Allow,
            Some(false) => return ApprovalDecision::Deny(format!(
                "tool '{}' denied by permission rule", tool_name
            )),
            None => {} // 无匹配，fallthrough
        }
        // Fallthrough 到内部 handler
        self.inner.approve(tool_name, input, class).await
    }
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn test_permission_rules_load() {
        let json = r#"{"permissions":{"allow":["read","bash:git*"],"deny":["bash:rm*"]}}"#;
        let rules = PermissionRules::from_json_str(json);
        assert_eq!(rules.allow.len(), 2);
        assert_eq!(rules.deny.len(), 1);
    }

    #[test]
    fn test_permission_rules_check_exact() {
        let rules = PermissionRules {
            allow: vec!["read".into()],
            deny: vec![],
        };
        assert_eq!(rules.check("read", &serde_json::json!({})), Some(true));
        assert_eq!(rules.check("write", &serde_json::json!({})), None);
    }

    #[test]
    fn test_permission_rules_check_glob_deny() {
        let rules = PermissionRules {
            allow: vec!["bash:git*".into()],
            deny: vec!["bash:rm*".into()],
        };
        // git push → allow
        assert_eq!(rules.check("bash", &serde_json::json!({"command": "git push"})), Some(true));
        // rm -rf → deny
        assert_eq!(rules.check("bash", &serde_json::json!({"command": "rm -rf /tmp"})), Some(false));
        // ls → 无匹配
        assert_eq!(rules.check("bash", &serde_json::json!({"command": "ls"})), None);
    }

    #[test]
    fn test_permission_rules_deny_overrides_allow() {
        let rules = PermissionRules {
            allow: vec!["bash".into()],
            deny: vec!["bash".into()],
        };
        // 同时匹配 allow 和 deny → deny 优先
        assert_eq!(rules.check("bash", &serde_json::json!({})), Some(false));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("git push", "git*"));
        assert!(glob_matches("git commit", "git*"));
        assert!(!glob_matches("ls", "git*"));
        assert!(glob_matches("anything", "*"));
        assert!(glob_matches("hello.txt", "*.txt"));
    }
}
