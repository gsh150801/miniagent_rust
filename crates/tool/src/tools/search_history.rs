use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Codex `history` 工具族（read_item / list_items / search_contents）的
/// 对应物：被裁剪出窗口的历史**完整归档**在
/// `<working_dir>/history_archive.jsonl`（每条带 item_id），本工具按需
/// 查回——历史详情永不丢失，只是不常驻上下文。
pub struct SearchHistoryTool;

impl Default for SearchHistoryTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchHistoryTool {
    pub fn new() -> Self {
        Self
    }
}

fn archive_path(ctx: &ToolContext) -> std::path::PathBuf {
    std::path::Path::new(&ctx.working_dir).join("history_archive.jsonl")
}

fn load_archive(ctx: &ToolContext) -> Vec<serde_json::Value> {
    std::fs::read_to_string(archive_path(ctx))
        .map(|raw| {
            raw.lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

const RESULT_CHAR_BUDGET: usize = 6_000;

#[async_trait]
impl Tool for SearchHistoryTool {
    fn name(&self) -> &str { "search_history" }
    fn description(&self) -> &str {
        "Search or read the archived conversation history (older items trimmed \
         out of the context window are preserved here, never destroyed). \
         action='search': case-insensitive keyword search over archived items, \
         returns matching item_ids with role + preview. action='read': return \
         the full content of one archived item by item_id. Use this to recover \
         exact technical details (file paths, command outputs, earlier findings) \
         referenced in your Task State Notes."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "read"],
                    "description": "search by keyword, or read one item fully"
                },
                "query": {
                    "type": "string",
                    "description": "search keyword(s) (action=search)"
                },
                "item_id": {
                    "type": "integer",
                    "description": "archived item id (action=read)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let action = input["action"].as_str()
            .ok_or_else(|| AgentError::tool("search_history", "missing 'action'"))?;
        let items = load_archive(ctx);
        if items.is_empty() {
            return Ok(ToolOutput {
                content: "history archive is empty (nothing trimmed yet)".into(),
                metadata: None,
            });
        }

        let mut out = String::new();
        match action {
            "search" => {
                let query = input["query"].as_str()
                    .ok_or_else(|| AgentError::tool("search_history", "missing 'query'"))?
                    .to_lowercase();
                let terms: Vec<&str> = query.split_whitespace().collect();
                let mut hits = 0usize;
                for item in &items {
                    let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let hay = content.to_lowercase();
                    if terms.iter().any(|t| hay.contains(t)) {
                        hits += 1;
                        if out.len() < RESULT_CHAR_BUDGET {
                            let preview: String = content.chars().take(200).collect();
                            out.push_str(&format!(
                                "[item_id {}] {} ({} chars): {}…\n---\n",
                                item.get("item_id").and_then(|v| v.as_u64()).unwrap_or(0),
                                item.get("role").and_then(|v| v.as_str()).unwrap_or("?"),
                                item.get("chars").and_then(|v| v.as_u64()).unwrap_or(0),
                                preview,
                            ));
                        }
                    }
                }
                if hits == 0 {
                    out = format!("no archived items match '{query}'");
                } else {
                    out = format!(
                        "{hits} match(es).{} Use action=read with item_id for full content.\n\n{}",
                        if out.len() >= RESULT_CHAR_BUDGET { " (preview budget reached)" } else { "" },
                        out,
                    );
                }
            }
            "read" => {
                let item_id = input["item_id"].as_u64()
                    .ok_or_else(|| AgentError::tool("search_history", "missing 'item_id'"))?;
                match items.iter().find(|i| i.get("item_id").and_then(|v| v.as_u64()) == Some(item_id)) {
                    Some(item) => {
                        let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        out = format!(
                            "[item_id {}] {}:\n{}{}",
                            item_id,
                            item.get("role").and_then(|v| v.as_str()).unwrap_or("?"),
                            content,
                            if content.len() >= 20_000 { "\n…[item truncated in archive]" } else { "" },
                        );
                    }
                    None => out = format!("no archived item with item_id {item_id}"),
                }
            }
            other => return Err(AgentError::tool("search_history", format!("unknown action '{other}'"))),
        }

        Ok(ToolOutput {
            content: out,
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn ctx_with_archive(dir: &std::path::Path, lines: &[serde_json::Value]) -> ToolContext {
        let path = dir.join("history_archive.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        ToolContext::new(dir.to_string_lossy().to_string(), "test")
    }

    #[tokio::test]
    async fn search_finds_and_read_returns_full_item() {
        let dir = std::env::temp_dir().join("miniagent_search_history_test");
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = ctx_with_archive(&dir, &[
            serde_json::json!({"item_id": 1, "role": "tool", "chars": 40, "content": "grep 基因表达 count = 30000"}),
            serde_json::json!({"item_id": 2, "role": "assistant", "chars": 30, "content": "小结：模板行完全同质"}),
        ]);
        let tool = SearchHistoryTool::new();

        let hit = tool.execute(json!({"action":"search","query":"基因表达"}), &ctx, Default::default())
            .await.unwrap();
        assert!(hit.content.contains("item_id 1"), "应命中含关键词的条目: {}", hit.content);

        let full = tool.execute(json!({"action":"read","item_id":2}), &ctx, Default::default())
            .await.unwrap();
        assert!(full.content.contains("模板行完全同质"), "{}", full.content);

        let miss = tool.execute(json!({"action":"search","query":"不存在的词"}), &ctx, Default::default())
            .await.unwrap();
        assert!(miss.content.contains("no archived items match"));
    }

    #[tokio::test]
    async fn empty_archive_is_reported() {
        let dir = std::env::temp_dir().join("miniagent_search_history_empty");
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = ToolContext::new(dir.to_string_lossy().to_string(), "t");
        let tool = SearchHistoryTool::new();
        let r = tool.execute(json!({"action":"search","query":"x"}), &ctx, Default::default())
            .await.unwrap();
        assert!(r.content.contains("archive is empty"));
    }
}
