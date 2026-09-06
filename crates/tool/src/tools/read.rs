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

/// 单次读取的默认行数上限（对齐 Claude Code read 硬上限 2000 行）。
/// 更大的文件必须用 offset/limit 分页——防止一次整读把对话历史灌爆
/// （live：read 单次 646KB ×11 次 → MiniMax 2013 context window exceeds）。
const DEFAULT_LINE_LIMIT: u64 = 2000;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "read" }
    fn description(&self) -> &str {
        "Read a file from the filesystem. Returns the file contents with line numbers. \
         Output is capped per call: default 2000 lines (~32KB); page through larger files \
         with offset/limit instead of re-reading the whole file."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the file"},
                "offset": {"type": "integer", "description": "Line number to start reading from (1-based)"},
                "limit": {"type": "integer", "description": "Maximum number of lines to read (default 2000, max 2000)"}
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = input["path"].as_str()
            .ok_or_else(|| AgentError::tool("read", "missing 'path' parameter"))?;
        let offset = input["offset"].as_u64().unwrap_or(1) as usize;
        let limit = input["limit"].as_u64().unwrap_or(DEFAULT_LINE_LIMIT).min(DEFAULT_LINE_LIMIT);

        // 安全校验：路径必须在 working_dir 内
        let path = crate::security::resolve_safe_path(path_str, std::path::Path::new(&ctx.working_dir))
            .map_err(|e| AgentError::tool("read", e))?;

        let content = std::fs::read_to_string(&path)
            .map_err(|e| AgentError::tool("read", format!("Cannot read '{}': {e}", path.display())))?;

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1).min(lines.len());
        let end = (start + limit as usize).min(lines.len());

        let output: String = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", start + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

        // 分页提示：让模型知道还有后续内容、怎么取（而非静默截断）
        let content = if end < lines.len() {
            format!(
                "{output}\n\n[Showing lines {}-{} of {} total. Use offset={} for the next page.]",
                start + 1, end, lines.len(), end + 1,
            )
        } else {
            output
        };

        // 第二道防线：即使 2000 行也可能超预算（超长行/二进制样内容），
        // 统一封顶 + 卸载全文到文件
        let (capped, offloaded) = crate::output_cap::cap_tool_output("read", &content, &ctx.working_dir);
        if offloaded {
            tracing::info!(path = %path.display(), "read output capped and offloaded to file");
        }

        Ok(ToolOutput {
            content: capped,
            metadata: None,
        })
    }
}
