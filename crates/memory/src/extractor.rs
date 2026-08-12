//! LLM 记忆提取器（参考 cc-python-claude memory/extractor.py）。
//!
//! 每轮对话结束后，自动分析最近的对话消息，用 LLM 提取值得持久化的记忆：
//! - **user**：用户角色、偏好、专业水平、目标
//! - **feedback**：用户对工作方式的纠正或确认
//! - **project**：项目上下文、决策、截止日期
//! - **reference**：外部资源指针（Linear 项目、Slack 频道、仪表板等）
//!
//! 提取的记忆写入 episodic memory（SQLite），下次对话通过 assemble_context 检索注入。
//!
//! 关键设计（参考 cc-python-claude）：
//! - **不存什么**：代码模式、git 历史、debug 方案——这些可从代码/git 推导
//! - **选择性**：大多数轮次没有值得存的记忆（LLM 返回空列表）
//! - **最少消息阈值**：新消息不足 4 条时跳过（省 API 调用）

use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::event::ContentBlock;
use miniagent_core::config::InferenceConfig;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::types::{EpisodicRecord, StructuredSummary};

/// 触发记忆提取的最少新消息数（参考 cc-python-claude MIN_NEW_MESSAGES=4）。
pub const MIN_NEW_MESSAGES: usize = 4;

/// 提取的记忆条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMemory {
    /// 记忆类型：user / feedback / project / reference
    #[serde(rename = "type")]
    pub memory_type: String,
    /// 简短标题
    pub name: String,
    /// 记忆内容
    pub content: String,
}

/// 提取结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractionResult {
    memories: Vec<ExtractedMemory>,
}

/// 记忆提取系统提示词（参考 cc-python-claude EXTRACTION_SYSTEM_PROMPT）。
const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are a memory extraction agent. Analyze the conversation and determine if there is anything worth saving to persistent memory.

## What to save
- **user**: User's role, preferences, expertise level, goals
- **feedback**: Corrections or confirmations about how to approach work
- **project**: Ongoing work context, decisions, deadlines (convert relative dates to absolute)
- **reference**: Pointers to external resources

## What NOT to save
- Code patterns, architecture, file paths — derivable from project state
- Git history — use git log/git blame
- Debugging solutions — the fix is in the code
- Ephemeral task details or temporary state

## Output format
Respond with EXACTLY this JSON (no other text):
{"memories": [{"type": "user|feedback|project|reference", "name": "short_name", "content": "memory content"}]}

If nothing worth saving, respond with: {"memories": []}
Be very selective. Most conversations have nothing worth saving."#;

/// 从对话消息中提取记忆。
///
/// 用 LLM 分析最近的对话，提取值得持久化的记忆。
/// 返回提取的记忆列表（可能为空）。
pub async fn extract_memories(
    provider: &dyn LlmProvider,
    messages: &[Message],
    cancel: CancellationToken,
) -> Result<Vec<ExtractedMemory>, AgentError> {
    // 消息不足时跳过（省 API）
    if messages.len() < MIN_NEW_MESSAGES {
        return Ok(vec![]);
    }

    // 拼接对话文本（取最近 20 条消息）
    let recent: Vec<&Message> = messages.iter().rev().take(20).collect();
    let conversation: String = recent.iter().rev()
        .map(|m| {
            let role = match m.role {
                miniagent_core::message::MessageRole::User => "User",
                miniagent_core::message::MessageRole::Assistant => "Assistant",
                miniagent_core::message::MessageRole::Tool => "Tool",
                miniagent_core::message::MessageRole::System => "System",
            };
            format!("[{role}]: {}", m.text_content())
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let request = CompletionRequest {
        system: EXTRACTION_SYSTEM_PROMPT.to_string(),
        messages: vec![Message::user(&conversation)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.1),
            max_tokens: Some(1000),
            ..Default::default()
        },
    };

    let resp = provider.complete(&request, cancel).await?;
    let text: String = resp.content.iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    // 解析 JSON（容错：LLM 可能加 markdown 包裹）
    let cleaned = text.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match serde_json::from_str::<ExtractionResult>(cleaned) {
        Ok(result) => Ok(result.memories),
        Err(e) => {
            tracing::debug!(error = %e, "memory extraction parse failed (likely no memories) — ignoring");
            Ok(vec![]) // 解析失败 = 无记忆可提取
        }
    }
}

/// 将提取的记忆转为 EpisodicRecord 以便存入 episodic memory。
pub fn memory_to_record(mem: &ExtractedMemory) -> EpisodicRecord {
    let now = chrono::Utc::now();
    EpisodicRecord {
        id: uuid::Uuid::new_v4(),
        title: format!("[{}] {}", mem.memory_type, mem.name),
        content: StructuredSummary {
            raw_summary: mem.content.clone(),
            ..Default::default()
        },
        tags: vec![mem.memory_type.clone()],
        source: Some("auto_extraction".to_string()),
        importance: match mem.memory_type.as_str() {
            "feedback" => 0.9,  // 反馈最重要
            "user" => 0.8,
            "project" => 0.6,
            "reference" => 0.5,
            _ => 0.5,
        },
        created_at: now,
        last_accessed: now,
        access_count: 0,
        decay_rate: 0.01,
        retention_floor: 0.3,
        current_strength: 1.0,
    }
}

/// 完整的记忆提取+存储流程。
///
/// 从对话提取记忆 → 转为 EpisodicRecord → 存入 MemoryManager。
/// 返回存储的记忆数量。
pub async fn extract_and_store(
    provider: &dyn LlmProvider,
    messages: &[Message],
    manager: &crate::manager::MemoryManager,
    cancel: CancellationToken,
) -> Result<usize, AgentError> {
    let memories = extract_memories(provider, messages, cancel).await?;
    let count = memories.len();

    for mem in &memories {
        let record = memory_to_record(mem);
        if let Err(e) = manager.store(&record) {
            tracing::error!(error = %e, memory_type = %mem.memory_type, "failed to store extracted memory");
        }
    }

    if count > 0 {
        tracing::info!(count, "extracted and stored memories from conversation");
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_to_record_feedback() {
        let mem = ExtractedMemory {
            memory_type: "feedback".into(),
            name: "prefer_terse".into(),
            content: "User prefers terse responses".into(),
        };
        let record = memory_to_record(&mem);
        assert_eq!(record.importance, 0.9);
        assert!(record.tags.contains(&"feedback".to_string()));
        assert!(record.title.contains("feedback"));
    }

    #[test]
    fn test_memory_to_record_user() {
        let mem = ExtractedMemory {
            memory_type: "user".into(),
            name: "data_scientist".into(),
            content: "User is a data scientist".into(),
        };
        let record = memory_to_record(&mem);
        assert_eq!(record.importance, 0.8);
    }

    #[tokio::test]
    async fn test_extract_memories_skips_short_conversations() {
        // 少于 MIN_NEW_MESSAGES 条消息 → 跳过
        let provider = miniagent_provider::MockProvider::new(r#"{"memories":[]}"#);
        let messages = vec![Message::user("hi")]; // 只有 1 条
        let result = extract_memories(&provider, &messages, CancellationToken::new()).await.unwrap();
        assert!(result.is_empty());
    }
}
