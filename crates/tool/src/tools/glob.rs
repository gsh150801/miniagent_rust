use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct GlobTool;

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str { "glob" }
    fn description(&self) -> &str {
        "Find files matching a glob pattern. Returns sorted by modification time (newest first)."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Glob pattern, e.g. '**/*.rs' or 'src/*.ts'"}
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let pattern = input["pattern"].as_str()
            .ok_or_else(|| AgentError::tool("glob", "missing 'pattern'"))?;

        let entries = crate::glob_util::glob_files(pattern)
            .map_err(|e| AgentError::tool("glob", format!("Invalid pattern: {e}")))?;

        let mut files: Vec<_> = entries
            .into_iter()
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                let mtime = meta.modified().ok()?;
                let size = meta.len();
                Some((path, mtime, size))
            })
            .collect();

        files.sort_by_key(|b| std::cmp::Reverse(b.1));

        if files.is_empty() {
            return Ok(ToolOutput {
                content: format!("No files match pattern '{pattern}'"),
                metadata: None,
            });
        }

        let output = files
            .iter()
            .map(|(p, _mtime, size)| {
                format!("{:>8}  {}", size, p.display())
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolOutput {
            content: format!("{} files matching '{pattern}':\n\n{output}", files.len()),
            metadata: None,
        })
    }
}
