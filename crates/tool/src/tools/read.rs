use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct ReadTool;

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "read" }
    fn description(&self) -> &str {
        "Read a file from the filesystem. Returns the file contents with line numbers."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the file"},
                "offset": {"type": "integer", "description": "Line number to start reading from"},
                "limit": {"type": "integer", "description": "Maximum number of lines to read"}
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let path = input["path"].as_str()
            .ok_or_else(|| AgentError::tool("read", "missing 'path' parameter"))?;
        let offset = input["offset"].as_u64().unwrap_or(1) as usize;
        let limit = input["limit"].as_u64();

        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::tool("read", format!("Cannot read '{path}': {e}")))?;

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1).min(lines.len());
        let end = match limit {
            Some(n) => (start + n as usize).min(lines.len()),
            None => lines.len(),
        };

        let output: String = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", start + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolOutput {
            content: output,
            metadata: None,
        })
    }
}
