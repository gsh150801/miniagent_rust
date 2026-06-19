use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::security::is_path_within_base;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Git operations tool: clone, status, add, commit, push, log, branch, diff.
/// Cannot operate outside the working directory (enforced by path guard).
pub struct GitTool;

impl Default for GitTool { fn default() -> Self { Self::new() } }
impl GitTool { pub fn new() -> Self { Self } }

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str { "git" }
    fn description(&self) -> &str {
        "Execute git operations. Supports: clone, status, add, commit, push, log, branch, diff, checkout, pull, remote. \
         Cannot operate on repositories outside the working directory."
    }
    fn class(&self) -> ToolClass { ToolClass::Mutating }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["clone", "status", "add", "commit", "push", "log", "branch", "diff", "checkout", "pull", "remote", "init", "reset"],
                    "description": "Git command to execute"
                },
                "repo_path": {
                    "type": "string",
                    "description": "Path to the local repository. Must be within working directory."
                },
                "args": {
                    "type": "object",
                    "description": "Additional arguments for the action",
                    "properties": {
                        "url": {"type": "string"},
                        "message": {"type": "string"},
                        "branch": {"type": "string"},
                        "files": {"type": "array", "items": {"type": "string"}},
                        "limit": {"type": "integer"},
                        "remote": {"type": "string"},
                    }
                }
            },
            "required": ["action", "repo_path"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let action = input["action"].as_str()
            .ok_or_else(|| AgentError::tool("git", "missing 'action'"))?;
        let repo_path = input["repo_path"].as_str()
            .ok_or_else(|| AgentError::tool("git", "missing 'repo_path'"))?;
        let args = &input["args"];

        let working_dir = std::path::Path::new(&ctx.working_dir);

        // For clone, repo_path is the destination directory for the clone
        // For all other actions, repo_path must be an existing repo within working dir
        if action != "clone" {
            let repo_dir = std::path::Path::new(repo_path);
            if !is_path_within_base(repo_dir, working_dir) {
                return Err(AgentError::tool("git", format!(
                    "Repository path '{}' is outside the working directory '{}'",
                    repo_path, ctx.working_dir
                )));
            }
        } else {
            // For clone, the destination must be within working dir
            let dest = std::path::Path::new(repo_path);
            if !is_path_within_base(dest, working_dir) {
                return Err(AgentError::tool("git", format!(
                    "Clone destination '{}' is outside the working directory '{}'",
                    repo_path, ctx.working_dir
                )));
            }
        }

        let output = match action {
            "clone" => git_clone(repo_path, args, cancel.clone()).await?,
            "status" => git_status(repo_path, cancel.clone()).await?,
            "add" => git_add(repo_path, args, cancel.clone()).await?,
            "commit" => git_commit(repo_path, args, cancel.clone()).await?,
            "push" => git_push(repo_path, args, cancel.clone()).await?,
            "pull" => git_pull(repo_path, args, cancel.clone()).await?,
            "log" => git_log(repo_path, args, cancel.clone()).await?,
            "branch" => git_branch(repo_path, args, cancel.clone()).await?,
            "diff" => git_diff(repo_path, args, cancel.clone()).await?,
            "checkout" => git_checkout(repo_path, args, cancel.clone()).await?,
            "remote" => git_remote(repo_path, args, cancel.clone()).await?,
            "init" => git_init(repo_path, args, cancel.clone()).await?,
            "reset" => git_reset(repo_path, args, cancel.clone()).await?,
            _ => return Err(AgentError::tool("git", format!("Unknown action: {action}"))),
        };

        Ok(ToolOutput { content: output, metadata: None })
    }
}

async fn run_git(args: &[&str], cwd: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    let result = tokio::select! {
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output() => r,
    };

    let output = result.map_err(|e| AgentError::tool("git", format!("Failed to execute git: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(AgentError::tool("git", format!("Git error: {stderr}")));
    }

    let combined = if stderr.is_empty() { stdout } else { format!("{stdout}\n{stderr}") };
    Ok(combined.trim().to_string())
}

async fn git_clone(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let url = args["url"].as_str()
        .ok_or_else(|| AgentError::tool("git", "clone requires 'url' in args"))?;
    let mut cmd = vec!["clone", url, repo_path];
    if let Some(branch) = args["branch"].as_str() {
        cmd.push("--branch"); cmd.push(branch);
    }
    // Use the parent of repo_path as cwd, since repo_path doesn't exist yet
    let parent = std::path::Path::new(repo_path).parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".into());
    run_git(&cmd, &parent, cancel).await
}

async fn git_status(repo_path: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    run_git(&["status", "--short"], repo_path, cancel).await
}

async fn git_add(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let files = args["files"].as_array().map(|a| {
        a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()
    }).unwrap_or_default();
    if files.is_empty() {
        run_git(&["add", "-A"], repo_path, cancel).await
    } else {
        let mut cmd = vec!["add"];
        cmd.extend(&files);
        run_git(&cmd, repo_path, cancel).await
    }
}

async fn git_commit(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let msg = args["message"].as_str()
        .ok_or_else(|| AgentError::tool("git", "commit requires 'message' in args"))?;
    run_git(&["commit", "-m", msg], repo_path, cancel).await
}

async fn git_push(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let remote = args["remote"].as_str().unwrap_or("origin");
    let branch = args["branch"].as_str().unwrap_or("main");
    run_git(&["push", remote, branch], repo_path, cancel).await
}

async fn git_pull(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let remote = args["remote"].as_str().unwrap_or("origin");
    let branch = args["branch"].as_str().unwrap_or("main");
    run_git(&["pull", remote, branch], repo_path, cancel).await
}

async fn git_log(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let limit = args["limit"].as_u64().unwrap_or(10).min(100);
    run_git(&["log", &format!("--max-count={limit}"), "--oneline", "--graph"], repo_path, cancel).await
}

async fn git_branch(repo_path: &str, _args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    run_git(&["branch", "-a"], repo_path, cancel).await
}

async fn git_diff(repo_path: &str, _args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    run_git(&["diff", "--stat"], repo_path, cancel).await
}

async fn git_checkout(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let branch = args["branch"].as_str()
        .ok_or_else(|| AgentError::tool("git", "checkout requires 'branch' in args"))?;
    run_git(&["checkout", branch], repo_path, cancel).await
}

async fn git_remote(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let remote_name = args["remote"].as_str().unwrap_or("-v");
    run_git(&["remote", remote_name], repo_path, cancel).await
}

async fn git_init(repo_path: &str, _args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    run_git(&["init"], repo_path, cancel).await
}

async fn git_reset(repo_path: &str, args: &serde_json::Value, cancel: CancellationToken) -> Result<String, AgentError> {
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("HEAD");
    let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("mixed");
    run_git(&["reset", &format!("--{mode}"), target], repo_path, cancel).await
}
