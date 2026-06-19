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
