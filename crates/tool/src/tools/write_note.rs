use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Codex 实验性上下文管理（0.153 `context_management.experimental_mode`）
/// 的 history notes 对应物：让模型在关键节点（决策敲定、假设被否决、
/// 里程碑完成、产物落盘）**主动**把任务状态写入结构化 notes，而不是等
/// 上下文爆掉后由事后摘要抢救——事后摘要发生在信息已丢失的时点，主动
/// notes 发生在信息最完整的时点。
///
/// notes 持久化到 `<working_dir>/notes.json`，由 Agent::run 每次请求前
/// 渲染进 system prompt（状态记忆层），并作为 notes 驱动 trim 的重建
/// 种子（new_context 式窗口重启）。
pub struct WriteNoteTool;

impl Default for WriteNoteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteNoteTool {
    pub fn new() -> Self {
        Self
    }
}

/// notes.json 的磁盘结构：{ sections: { goal: str, done: [..], todo: [..],
/// artifacts: [..], rejected: [..], key_facts: [..] }, updated_at }
pub fn notes_path(working_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(working_dir).join("notes.json")
}

pub fn load_notes(working_dir: &str) -> serde_json::Value {
    std::fs::read_to_string(notes_path(working_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({ "sections": {} }))
}

pub fn save_notes(working_dir: &str, notes: &serde_json::Value) -> Result<(), AgentError> {
    let path = notes_path(working_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AgentError::tool("write_note", format!("create_dir_all: {e}")))?;
    }
    let mut out = notes.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "updated_at".into(),
            json!(chrono::Utc::now().to_rfc3339()),
        );
    }
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap_or_default())
        .map_err(|e| AgentError::tool("write_note", format!("write: {e}")))
}

#[async_trait]
impl Tool for WriteNoteTool {
    fn name(&self) -> &str { "write_note" }
    fn description(&self) -> &str {
        "Record a structured task-state note that survives context trimming \
         (the state-memory layer). Sections: 'goal' (replaces the current \
         objective statement), 'done' / 'todo' (append one item), 'artifacts' \
         (append a file path + one-line summary), 'rejected' (append a \
         rejected hypothesis / approach + WHY it was rejected), 'key_facts' \
         (append a durable fact worth keeping). Write a note at decision \
         points and milestones — notes are re-injected into your context on \
         every turn even after history trimming."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "section": {
                    "type": "string",
                    "enum": ["goal", "done", "todo", "artifacts", "rejected", "key_facts"],
                    "description": "Which task-state section to write"
                },
                "content": {
                    "type": "string",
                    "description": "The note text. For 'goal' this replaces the objective; for others it is appended."
                }
            },
            "required": ["section", "content"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let section = input["section"].as_str()
            .ok_or_else(|| AgentError::tool("write_note", "missing 'section'"))?
            .to_string();
        let content = input["content"].as_str()
            .ok_or_else(|| AgentError::tool("write_note", "missing 'content'"))?
            .trim()
            .to_string();
        if content.is_empty() {
            return Err(AgentError::tool("write_note", "empty 'content'"));
        }
        const SECTIONS: &[&str] = &["goal", "done", "todo", "artifacts", "rejected", "key_facts"];
        if !SECTIONS.contains(&section.as_str()) {
            return Err(AgentError::tool("write_note", format!("unknown section '{section}'")));
        }

        let mut notes = load_notes(&ctx.working_dir);
        let sections = notes.get("sections").cloned().unwrap_or_else(|| json!({}));
        let mut sections = sections.as_object().cloned().unwrap_or_default();

        if section == "goal" {
            sections.insert(section.clone(), json!(content));
        } else {
            let list = sections.get(&section).cloned().unwrap_or_else(|| json!([]));
            let mut list = list.as_array().cloned().unwrap_or_default();
            list.push(json!(content));
            // 有界：每节最多 40 条，最老的先淘汰（notes 也不能无限膨胀）
            if list.len() > 40 {
                let drop = list.len() - 40;
                list.drain(..drop);
            }
            sections.insert(section.clone(), json!(list));
        }
        notes["sections"] = json!(sections);
        save_notes(&ctx.working_dir, &notes)?;

        Ok(ToolOutput {
            content: format!("note recorded in '{section}'"),
            metadata: None,
        })
    }
}

/// 渲染 notes 为 system prompt 注入块（状态记忆层）。空 notes 返回空串。
pub fn render_notes_block(working_dir: &str) -> String {
    let notes = load_notes(working_dir);
    let Some(sections) = notes.get("sections").and_then(|s| s.as_object()) else {
        return String::new();
    };
    if sections.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n## Task State Notes（跨上下文裁剪保留的状态记忆；这是唯一可靠的长期记忆，历史正文可能已被裁剪）\n",
    );
    if let Some(goal) = sections.get("goal").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        out.push_str(&format!("- 目标: {goal}\n"));
    }
    for (key, label) in [
        ("done", "已完成"),
        ("todo", "待办"),
        ("artifacts", "关键产物"),
        ("rejected", "已否决（勿重试）"),
        ("key_facts", "关键事实"),
    ] {
        if let Some(list) = sections.get(key).and_then(|v| v.as_array()) {
            for item in list.iter().filter_map(|v| v.as_str()) {
                out.push_str(&format!("- {label}: {item}\n"));
            }
        }
    }
    // 模型自发写的 notes.md 全文（同步进 notes_md 节）——按字符预算截断
    if let Some(md) = sections.get("notes_md").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        out.push_str("\n### 笔记文件（notes.md）内容\n");
        out.push_str(&md.chars().take(NOTES_MD_INJECT_CHARS).collect::<String>());
        let total = md.chars().count();
        if total > NOTES_MD_INJECT_CHARS {
            out.push_str(&format!("\n[…notes.md 共 {total} 字符，已截断；完整内容可用 read 工具读取]\n"));
        }
    }
    out
}

/// notes.md 同步与机械 checkpoint 注入时的字符预算（约 8K token）。
const NOTES_MD_INJECT_CHARS: usize = 16_000;

/// 把模型自发写入的 notes.md 同步进 notes.json 的 `notes_md` 节。
///
/// live 发现：模型在深度研究任务中 0 次调用 write_note 工具，但用
/// write 写了完整分节的 notes.md（## goal / ## key_facts / 每源一节）
/// ——意图接受度高，载体不重要。此函数把该自发行为接入状态记忆层：
/// Agent::run 每轮注入与 trim 重建都会读到。
pub fn sync_notes_markdown(working_dir: &str, markdown_content: &str) {
    if working_dir.is_empty() || markdown_content.trim().is_empty() {
        return;
    }
    let mut notes = load_notes(working_dir);
    let sections = notes.get("sections").cloned().unwrap_or_else(|| json!({}));
    let mut sections = sections.as_object().cloned().unwrap_or_default();
    let truncated: String = markdown_content.chars().take(NOTES_MD_INJECT_CHARS).collect();
    sections.insert("notes_md".into(), json!(truncated));
    notes["sections"] = json!(sections);
    let _ = save_notes(working_dir, &notes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ToolContext;

    #[tokio::test]
    async fn write_notes_md_syncs_into_notes_json() {
        let dir = std::env::temp_dir().join("miniagent_notes_sync_test");
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = ToolContext::new(dir.to_string_lossy().to_string(), "t");
        let tool = super::super::write::WriteTool::new();

        let md = "# Research Notes\n\n## goal\n- 主题: mRNA 疫苗调研\n## key_facts\n- KEYNOTE-942 RFS HR=0.61\n";
        let notes_file = dir.join("notes.md");
        let input = json!({
            "path": notes_file.to_string_lossy(),
            "content": md,
        });
        tool.execute(input, &ctx, CancellationToken::new()).await.unwrap();

        let notes = load_notes(&ctx.working_dir);
        let synced = notes["sections"]["notes_md"].as_str().unwrap();
        assert!(synced.contains("KEYNOTE-942"), "notes.md 内容应同步进 notes.json");
        // render_notes_block 应包含同步的 markdown
        let block = render_notes_block(&ctx.working_dir);
        assert!(block.contains("KEYNOTE-942"), "注入块应包含 notes_md 内容");
        assert!(block.contains("笔记文件"));
    }

    #[test]
    fn empty_markdown_is_ignored() {
        let dir = std::env::temp_dir().join("miniagent_notes_empty_test");
        std::fs::create_dir_all(&dir).unwrap();
        let existing = notes_path(&dir.to_string_lossy());
        let _ = std::fs::remove_file(&existing);

        sync_notes_markdown(&dir.to_string_lossy(), "   \n");
        assert!(!existing.exists(), "空白内容不应产生 notes.json");
    }

}
