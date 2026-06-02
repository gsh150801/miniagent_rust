use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct WriteTool;

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str { "write" }
    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if needed. Overwrites existing files."
    }
    fn class(&self) -> ToolClass { ToolClass::Mutating }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the file"},
                "content": {"type": "string", "description": "Content to write"}
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let path = input["path"].as_str()
            .ok_or_else(|| AgentError::tool("write", "missing 'path'"))?;
        let content = input["content"].as_str()
            .ok_or_else(|| AgentError::tool("write", "missing 'content'"))?;

        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AgentError::tool("write", format!("mkdir '{parent:?}': {e}")))?;
        }

        std::fs::write(path, content)
            .map_err(|e| AgentError::tool("write", format!("write '{path}': {e}")))?;

        Ok(ToolOutput {
            content: format!("Wrote {} bytes to {path}", content.len()),
            metadata: None,
        })
    }
}
