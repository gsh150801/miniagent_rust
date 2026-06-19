use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Conda/mamba/micromamba environment management tool.
/// Can create, remove, install, list, and activate environments.
/// New environments MUST be created with a 'mn_' prefix (e.g., 'mn_py310', 'mn_myproject').
/// CANNOT modify or delete environments without the 'mn_' prefix (they are system-owned).
pub struct CondaTool;

impl Default for CondaTool { fn default() -> Self { Self::new() } }
impl CondaTool {
    pub fn new() -> Self {
        Self
    }

    /// Detect which conda backend is available
    async fn detect_backend() -> &'static str {
        for cmd in &["micromamba", "mamba", "conda"] {
            if tokio::process::Command::new(cmd)
                .arg("--version")
                .output()
                .await
                .is_ok_and(|o| o.status.success())
            {
                return cmd;
            }
        }
        "conda" // fallback
    }
}

#[async_trait]
impl Tool for CondaTool {
    fn name(&self) -> &str { "conda" }
    fn description(&self) -> &str {
        "Manage conda/mamba/micromamba environments. Supports: create, install, remove, list, activate, deactivate, info. \
         New environments MUST be created with 'mn_' prefix (e.g., 'mn_py310'). \
         Environments without 'mn_' prefix are system-owned and cannot be modified."
    }
    fn class(&self) -> ToolClass { ToolClass::Mutating }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "install", "remove", "uninstall", "list", "activate", "deactivate", "info", "list_envs", "clean"],
                    "description": "Conda action to execute"
                },
                "env_name": {
                    "type": "string",
                    "description": "Environment name or full path. New envs should use a name (not a path)."
                },
                "packages": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Package names to install/remove"
                },
                "python_version": {
                    "type": "string",
                    "description": "Python version for new environments (e.g., '3.10')"
                },
                "channel": {
                    "type": "string",
                    "description": "Conda channel (e.g., 'conda-forge')"
                },
                "backend": {
                    "type": "string",
                    "enum": ["auto", "conda", "mamba", "micromamba"],
                    "description": "Which backend to use"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let action = input["action"].as_str()
            .ok_or_else(|| AgentError::tool("conda", "missing 'action'"))?;
        let env_name = input["env_name"].as_str().unwrap_or("");
        let packages: Vec<&str> = input["packages"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let python_ver = input["python_version"].as_str().unwrap_or("3.10");
        let channel = input["channel"].as_str();

        let backend = if let Some(b) = input["backend"].as_str() {
            match b { "conda" => "conda", "mamba" => "mamba", "micromamba" => "micromamba", _ => Self::detect_backend().await }
        } else {
            Self::detect_backend().await
        };

        // ---- Security guard: only 'mn_' prefixed environments can be modified ----
        if !env_name.is_empty() && matches!(action, "install" | "remove" | "uninstall" | "clean") {
            if !env_name.starts_with("mn_") {
                return Err(AgentError::tool("conda", format!(
                    "Environment '{}' does not have the 'mn_' prefix. \
                     Only miniagent-managed environments (with 'mn_' prefix) can be modified. \
                     To create a new environment, use: create with env_name='mn_yourname'", env_name
                )));
            }
        }

        // Force 'mn_' prefix when creating new environments
        if action == "create" {
            if !env_name.starts_with("mn_") {
                return Err(AgentError::tool("conda", format!(
                    "New conda environments must use the 'mn_' prefix. \
                     Use env_name='mn_{}' instead of '{}'.", env_name, env_name
                )));
            }
            if env_name.starts_with('/') || env_name.starts_with("~/") {
                return Err(AgentError::tool("conda", format!(
                    "Cannot create conda environment at '{}' — use a simple name (e.g., 'mn_myenv').", env_name
                )));
            }
        }

        let output = match action {
            "create" => conda_create(backend, env_name, python_ver, channel, cancel.clone()).await?,
            "install" => conda_install(backend, env_name, &packages, channel, cancel.clone()).await?,
            "remove" | "uninstall" => conda_remove(backend, env_name, &packages, cancel.clone()).await?,
            "list" => conda_list(backend, env_name, cancel.clone()).await?,
            "list_envs" => conda_list_envs(backend, cancel.clone()).await?,
            "activate" => conda_activate(env_name),
            "deactivate" => "Run 'conda deactivate' in your shell.".to_string(),
            "info" => conda_info(backend, cancel.clone()).await?,
            "clean" => conda_clean(backend, cancel.clone()).await?,
            _ => return Err(AgentError::tool("conda", format!("Unknown action: {action}"))),
        };

        Ok(ToolOutput { content: output, metadata: None })
    }
}

async fn run_conda(args: &[&str], backend: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    let result = tokio::select! {
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = tokio::process::Command::new(backend)
            .args(args)
            .output() => r,
    };
    let output = result.map_err(|e| AgentError::tool("conda", format!("Failed to run {backend}: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(AgentError::tool("conda", format!("{backend} error: {stderr}")));
    }
    let combined = if stderr.is_empty() { stdout } else { format!("{stdout}\n{stderr}") };
    Ok(combined.trim().to_string())
}

async fn conda_create(backend: &str, env_name: &str, python_ver: &str, channel: Option<&str>, cancel: CancellationToken) -> Result<String, AgentError> {
    let py_ver_flag = format!("python={python_ver}");
    let mut args = vec!["create", "-y", "-n", env_name, &py_ver_flag];
    if let Some(ch) = channel { args.push("-c"); args.push(ch); }
    run_conda(&args, backend, cancel).await
}

async fn conda_install(backend: &str, env_name: &str, packages: &[&str], channel: Option<&str>, cancel: CancellationToken) -> Result<String, AgentError> {
    if packages.is_empty() { return Err(AgentError::tool("conda", "install requires at least one package")) }
    let mut args = vec!["install", "-y", "-n", env_name];
    if let Some(ch) = channel { args.push("-c"); args.push(ch); }
    args.extend(packages);
    run_conda(&args, backend, cancel).await
}

async fn conda_remove(backend: &str, env_name: &str, packages: &[&str], cancel: CancellationToken) -> Result<String, AgentError> {
    if packages.is_empty() { return Err(AgentError::tool("conda", "remove requires at least one package")) }
    let mut args = vec!["remove", "-y", "-n", env_name];
    args.extend(packages);
    run_conda(&args, backend, cancel).await
}

async fn conda_list(backend: &str, env_name: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    let args = vec!["list", "-n", env_name];
    run_conda(&args, backend, cancel).await
}

async fn conda_list_envs(backend: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    run_conda(&["env", "list"], backend, cancel).await
}

fn conda_activate(env_name: &str) -> String {
    format!(
        "To activate the '{}' environment, run in your terminal:\n\n  conda activate {}\n\n\
         Or for mamba:\n  mamba activate {}\n\n\
         Or for micromamba:\n  micromamba activate {}",
        env_name, env_name, env_name, env_name
    )
}

async fn conda_info(backend: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    run_conda(&["info"], backend, cancel).await
}

async fn conda_clean(backend: &str, cancel: CancellationToken) -> Result<String, AgentError> {
    run_conda(&["clean", "-a", "-y"], backend, cancel).await
}
