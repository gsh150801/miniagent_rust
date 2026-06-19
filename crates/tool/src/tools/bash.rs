use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct BashTool;

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BashTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str {
        "Execute a shell command. Returns stdout and stderr. Command is killed on cancellation. Maximum 60 seconds timeout."
    }
    fn class(&self) -> ToolClass { ToolClass::Mutating }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "timeout_ms": {"type": "integer", "description": "Timeout in ms (default: 60000)"}
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let cmd = input["command"].as_str()
            .ok_or_else(|| AgentError::tool("bash", "missing 'command'"))?;
        let timeout_ms = input["timeout_ms"].as_u64().unwrap_or(60_000);

        let child = Command::new("bash")
            .arg("-c")
            .arg(cmd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AgentError::tool("bash", format!("spawn: {e}")))?;

        let pid = child.id().unwrap_or(0);

        let output = tokio::select! {
            _ = cancel.cancelled() => {
                if pid != 0 { kill_process(pid); }
                return Err(AgentError::Cancelled);
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
                if pid != 0 { kill_process(pid); }
                return Err(AgentError::tool("bash", format!("timed out after {timeout_ms}ms")));
            }
            result = child.wait_with_output() => {
                result
            }
        };

        let output = output
            .map_err(|e| AgentError::tool("bash", format!("wait: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut content = String::new();
        if !stdout.is_empty() {
            content.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !content.is_empty() { content.push('\n'); }
            content.push_str("STDERR:\n");
            content.push_str(&stderr);
        }
        if content.is_empty() {
            content.push_str("(no output)");
        }

        content.push_str(&format!("\n\nExit code: {}", output.status.code().unwrap_or(-1)));

        Ok(ToolOutput { content, metadata: None })
    }
}

fn kill_process(pid: u32) {
    let _ = std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .output();
}
