use serde::{Deserialize, Serialize};

use crate::event::ContentBlock;
use crate::types::MessageId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Message {
    pub fn new(role: MessageRole, content: Vec<ContentBlock>) -> Self {
        Self {
            id: MessageId::new(),
            role,
            content,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(
            MessageRole::User,
            vec![ContentBlock::Text {
                text: text.into(),
            }],
        )
    }

    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self::new(
            MessageRole::Assistant,
            vec![ContentBlock::Text {
                text: text.into(),
            }],
        )
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(
            MessageRole::System,
            vec![ContentBlock::Text {
                text: text.into(),
            }],
        )
    }

    pub fn tool(tool_call_id: impl Into<String>, result: impl Into<String>) -> Self {
        Self::new(
            MessageRole::Tool,
            vec![ContentBlock::Text {
                text: format!(
                    "[toolu_vrtx_{}] {}",
                    tool_call_id.into(),
                    result.into()
                ),
            }],
        )
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 从 Tool 消息文本中提取 tool_call_id（"[toolu_vrtx_{id}] ..." 格式）。
fn extract_tool_id(msg: &Message) -> Option<String> {
    msg.text_content()
        .strip_prefix("[toolu_vrtx_")
        .and_then(|s| s.split(']').next())
        .map(|s| s.to_string())
}

/// 修复 tool 配对违例——MiniMax 2013 "tool call result does not follow
/// tool call" 的机械修复，双向都管：
///
/// 1. **孤立/重复 Tool 消息 → 丢弃**。Tool 消息的 id 未被此前 assistant
///    请求过、或已被回答过（重复结果），会被 provider 拒绝。这是
///    validate_transcript 没有覆盖的反方向（live：b3337de9 任务 6 个
///    子任务死于此错误）。
/// 2. **未回答的 tool_use → 在其 assistant 消息后紧邻插入合成结果**。
///    validate_transcript 把合成结果追加到 history 末尾——若孤立
///    tool_use 在中间，合成结果落在后续消息之后，反而制造新的配对
///    违例。必须紧邻插入。
///
/// 返回修复的消息数。幂等：已配对的序列原样通过。
pub fn repair_tool_pairing(history: &mut Vec<Message>) -> usize {
    let mut fixed = 0usize;

    // Pass 1: 丢弃孤立/重复 Tool 消息（按顺序扫描，维护未答 id 集合）
    let mut pending: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cleaned: Vec<Message> = Vec::with_capacity(history.len());
    for msg in history.drain(..) {
        match msg.role {
            MessageRole::Tool => {
                match extract_tool_id(&msg) {
                    Some(id) if pending.remove(&id) => cleaned.push(msg),
                    // 孤立（无对应请求）、重复（已回答）、无法识别 id → 丢弃
                    _ => { fixed += 1; }
                }
            }
            MessageRole::Assistant => {
                for b in &msg.content {
                    if let ContentBlock::ToolUse { id, .. } = b {
                        pending.insert(format!("{}", id.0));
                    }
                }
                cleaned.push(msg);
            }
            _ => cleaned.push(msg),
        }
    }

    // Pass 2: 为未回答的 tool_use 紧邻插入合成结果（每缺失 id 一条）
    let mut out: Vec<Message> = Vec::with_capacity(cleaned.len());
    let mut i = 0usize;
    while i < cleaned.len() {
        let msg = cleaned[i].clone();
        let is_assistant = matches!(msg.role, MessageRole::Assistant);
        out.push(msg);
        if is_assistant {
            let requested: Vec<String> = cleaned[i].content.iter().filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(format!("{}", id.0)),
                _ => None,
            }).collect();
            if !requested.is_empty() {
                // 收集紧随其后的连续 Tool 消息已回答的 id
                let mut answered: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut j = i + 1;
                while j < cleaned.len() && matches!(cleaned[j].role, MessageRole::Tool) {
                    if let Some(id) = extract_tool_id(&cleaned[j]) {
                        answered.insert(id);
                    }
                    j += 1;
                }
                for id in &requested {
                    if !answered.contains(id) {
                        out.push(Message::tool(
                            id,
                            "[ERROR: tool execution was interrupted — this result is synthetic]",
                        ));
                        fixed += 1;
                    }
                }
            }
        }
        i += 1;
    }

    *history = out;
    fixed
}

/// 修复 history 中的孤立 tool_use（参考 cc-python-claude session/recovery.py validate_transcript）。
///
/// 当 Agent 在 tool_use 后、tool 执行前崩溃/中断时，history 末尾的 assistant 消息
/// 含 ToolUse block 但后续无对应的 Tool role 消息。直接送给 provider 会触发 API
/// 校验错误（tool_use 必须配对 tool_result）。
///
/// 本函数：
/// 1. 扫描 history 中每条 assistant 消息的 ToolUse blocks
/// 2. 检查后续是否有对应的 Tool role 消息（通过 tool_call_id 匹配）
/// 3. 对缺失的 tool_result 追加合成的 error 消息
///
/// 返回修改后的 history（原地修改 + 返回修补的消息数）。
pub fn validate_transcript(history: &mut Vec<Message>) -> usize {
    let mut fixed = 0usize;

    // 收集所有已回答的 tool_call_id（从 Tool role 消息的文本中提取）
    // Tool 消息格式: "[toolu_vrtx_{id}] {result}"
    let mut answered_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in history.iter() {
        if matches!(msg.role, MessageRole::Tool) {
            // 从文本中提取 tool_call_id
            let text = msg.text_content();
            if let Some(id_start) = text.find("[toolu_vrtx_") {
                let rest = &text[id_start + "[toolu_vrtx_".len()..];
                if let Some(id_end) = rest.find(']') {
                    answered_ids.insert(rest[..id_end].to_string());
                }
            }
        }
    }

    // 扫描 assistant 消息的 ToolUse，找未回答的
    let mut to_append: Vec<Message> = Vec::new();
    for msg in history.iter() {
        if !matches!(msg.role, MessageRole::Assistant) {
            continue;
        }
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, .. } = block {
                let id_str = format!("{}", id.0);
                if !answered_ids.contains(&id_str) {
                    // 孤立 tool_use：追加合成 error tool_result
                    to_append.push(Message::tool(
                        &id_str,
                        "[ERROR: tool execution was interrupted — this result is synthetic]",
                    ));
                    fixed += 1;
                }
            }
        }
    }

    if fixed > 0 {
        tracing::warn!(
            fixed_count = fixed,
            "validate_transcript: repaired {} orphaned tool_use blocks with synthetic error results",
            fixed,
        );
        // 注：tracing 在 core crate 中通过 workspace dep 可用，无需额外 import
        let _ = fixed; // suppress unused warning if tracing is filtered
        history.extend(to_append);
    }

    fixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ContentBlock;
    use crate::types::ToolCallId;

    #[test]
    fn test_validate_transcript_no_orphans() {
        // 正常 history：assistant tool_use → tool result
        let tool_id = uuid::Uuid::nil();
        let mut history = vec![
            Message::user("test"),
            Message::assistant(vec![ContentBlock::ToolUse {
                id: ToolCallId(tool_id),
                name: "read".into(),
                input: serde_json::json!({"path": "test.txt"}),
            }]),
            Message::tool(tool_id.to_string(), "file content"),
        ];
        let fixed = validate_transcript(&mut history);
        assert_eq!(fixed, 0, "no orphans → no fix needed");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_validate_transcript_repairs_orphan() {
        // 崩溃场景：assistant tool_use 后无 tool result
        let mut history = vec![
            Message::user("test"),
            Message::assistant(vec![ContentBlock::ToolUse {
                id: ToolCallId(uuid::Uuid::nil()),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            }]),
            // 缺少 tool result（崩溃在执行前）
        ];
        let fixed = validate_transcript(&mut history);
        assert_eq!(fixed, 1, "should detect 1 orphan");
        assert_eq!(history.len(), 3, "should append 1 synthetic result");
        assert!(matches!(history[2].role, MessageRole::Tool));
        assert!(history[2].text_content().contains("interrupted"));
    }

    #[test]
    fn test_validate_transcript_multiple_orphans() {
        let id1 = uuid::Uuid::new_v4();
        let id2 = uuid::Uuid::new_v4();
        let mut history = vec![
            Message::user("test"),
            Message::assistant(vec![
                ContentBlock::ToolUse {
                    id: ToolCallId(id1),
                    name: "read".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: ToolCallId(id2),
                    name: "write".into(),
                    input: serde_json::json!({}),
                },
            ]),
            // 只有 tool id1 被回答
            Message::tool(id1.to_string(), "ok"),
        ];
        let fixed = validate_transcript(&mut history);
        assert_eq!(fixed, 1, "tool 2 is orphaned");
        assert_eq!(history.len(), 4, "should append 1 synthetic result for tool 2");
    }

    #[test]
    fn test_validate_transcript_no_tool_use() {
        // 纯文本对话，无 tool_use
        let mut history = vec![
            Message::user("hello"),
            Message::assistant_text("hi"),
        ];
        let fixed = validate_transcript(&mut history);
        assert_eq!(fixed, 0);
    }
}
