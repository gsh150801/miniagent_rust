use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct EditTool;

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EditTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str { "edit" }
    fn description(&self) -> &str {
        "Replace exact string occurrences in a file. Returns count of replacements made."
    }
    fn class(&self) -> ToolClass { ToolClass::Mutating }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the file"},
                "old_string": {"type": "string", "description": "Text to replace"},
                "new_string": {"type": "string", "description": "Replacement text"},
                "replace_all": {"type": "boolean", "description": "Replace all occurrences (default: false)"}
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let path = input["path"].as_str()
            .ok_or_else(|| AgentError::tool("edit", "missing 'path'"))?;
        let old = input["old_string"].as_str()
            .ok_or_else(|| AgentError::tool("edit", "missing 'old_string'"))?;
        let new = input["new_string"].as_str()
            .ok_or_else(|| AgentError::tool("edit", "missing 'new_string'"))?;
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::tool("edit", format!("read '{path}': {e}")))?;

        if old.is_empty() {
            return Err(AgentError::tool("edit", "old_string must not be empty"));
        }

        let count = if replace_all {
            content.matches(old).count()
        } else {
            if content.matches(old).count() > 1 && !replace_all {
                return Err(AgentError::tool(
                    "edit",
                    format!("old_string is not unique in file (found {} matches). Use replace_all or provide more context.", content.matches(old).count()),
                ));
            }
            1
        };

        let new_content = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };

        std::fs::write(path, &new_content)
            .map_err(|e| AgentError::tool("edit", format!("write '{path}': {e}")))?;

        Ok(ToolOutput {
            content: format!("Made {count} replacement(s) in {path}"),
            metadata: None,
        })
    }
}
