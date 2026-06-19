use async_trait::async_trait;
use crate::traits::ToolClass;

#[derive(Debug)]
pub enum ApprovalDecision {
    Allow,
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
            if tool_name == "bash" {
                if let Some(cmd) = input["command"].as_str() {
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
            }

            if tool_name == "write" {
                // Writing empty content or very short content is suspicious
                if let Some(content) = input["content"].as_str() {
                    if content.trim().is_empty() {
                        return ApprovalDecision::Deny(
                            "Writing empty content requires explicit approval.".into()
                        );
                    }
                }
            }

            if tool_name == "edit" {
                // Editing files is potentially destructive
                // For edit, we check if it's replacing content with empty
                if let Some(old) = input["oldString"].as_str() {
                    let new = input["newString"].as_str().unwrap_or("");
                    if old.len() > 0 && new.is_empty() {
                        return ApprovalDecision::Deny(
                            "Deleting content via 'edit' (replacing with empty) requires explicit approval.".into()
                        );
                    }
                }
            }
        }

        // ---- Rule 2: Git tool cannot operate outside task_dir ----
        if tool_name == "git" {
            if let Some(repo_path) = input["repo_path"].as_str() {
                let repo_dir = std::path::Path::new(repo_path);
                let task_dir = std::path::Path::new(&self.task_dir);
                // If both exist and repo is outside task_dir, deny
                if repo_dir.is_absolute() && task_dir.exists() {
                    let repo_canon = std::fs::canonicalize(repo_dir).ok();
                    let task_canon = std::fs::canonicalize(task_dir).ok();
                    if let (Some(repo_c), Some(task_c)) = (repo_canon, task_canon) {
                        if !repo_c.starts_with(&task_c) {
                            return ApprovalDecision::Deny(format!(
                                "Git operations on '{}' are outside the task directory '{}'.",
                                repo_c.display(), task_c.display()
                            ));
                        }
                    }
                }
            }
        }

        // ---- Rule 3: Conda tool cannot modify system environments ----
        if tool_name == "conda" {
            if let Some(action) = input["action"].as_str() {
                let modifying_actions = ["install", "remove", "uninstall", "clean", "create"];
                if modifying_actions.contains(&action) {
                    if let Some(env_name) = input["env_name"].as_str() {
                        if crate::security::is_system_conda_path(env_name) {
                            return ApprovalDecision::Deny(format!(
                                "Modifying system conda environment '{}' is not allowed. \
                                 Create new environments within the working directory.",
                                env_name
                            ));
                        }
                    }
                }
            }
        }

        ApprovalDecision::Allow
    }
}
