use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::security::resolve_safe_path;

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
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = input["path"].as_str()
            .ok_or_else(|| AgentError::tool("write", "missing 'path'"))?;
        let content = input["content"].as_str()
            .ok_or_else(|| AgentError::tool("write", "missing 'content'"))?;

        // 安全校验：路径必须在 working_dir 内，防止 LLM 写任意路径（如 ~/.ssh/authorized_keys）
        let path = resolve_safe_path(path_str, std::path::Path::new(&ctx.working_dir))
            .map_err(|e| AgentError::tool("write", e))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AgentError::tool("write", format!("mkdir '{parent:?}': {e}")))?;
        }

        std::fs::write(&path, content)
            .map_err(|e| AgentError::tool("write", format!("write '{}': {e}", path.display())))?;

        Ok(ToolOutput {
            content: format!("Wrote {} bytes to {}", content.len(), path.display()),
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ToolContext;

    fn ctx_with_workdir(dir: &str) -> ToolContext {
        ToolContext { working_dir: dir.to_string(), session_id: "test".to_string() }
    }

    #[tokio::test]
    async fn write_rejects_path_outside_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workdir(&dir.path().to_string_lossy());
        let tool = WriteTool::new();

        // 尝试写到 workdir 之外（绝对路径）
        let outside = tempfile::tempdir().unwrap();
        let input = json!({
            "path": outside.path().join("evil.txt").to_string_lossy(),
            "content": "malicious"
        });
        let result = tool.execute(input, &ctx, CancellationToken::new()).await;
        assert!(result.is_err(), "write must reject path outside working_dir");
        // 确认文件确实没被创建
        assert!(!outside.path().join("evil.txt").exists());
    }

    #[tokio::test]
    async fn write_allows_path_within_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workdir(&dir.path().to_string_lossy());
        let tool = WriteTool::new();

        let input = json!({
            "path": dir.path().join("ok.txt").to_string_lossy(),
            "content": "safe content"
        });
        let result = tool.execute(input, &ctx, CancellationToken::new()).await;
        assert!(result.is_ok(), "write within workdir should succeed");
        assert!(dir.path().join("ok.txt").exists());
    }

    #[tokio::test]
    async fn write_rejects_traversal_path() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workdir(&dir.path().to_string_lossy());
        let tool = WriteTool::new();

        let input = json!({
            "path": dir.path().join("../../../etc/evil.txt").to_string_lossy(),
            "content": "traversal"
        });
        let result = tool.execute(input, &ctx, CancellationToken::new()).await;
        assert!(result.is_err(), "write must reject path traversal");
    }
}
