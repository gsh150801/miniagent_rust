//! AskUser 工具：LLM 可调用的"向用户提问"工具（参考 cc-python-claude ask_user_tool.py）。
//!
//! 通过 ToolContext 的 UserPrompt trait 依赖注入解耦输入来源：
//! - CLI 实现从 stdin 读取
//! - server 实现通过 WebSocket 等待前端回复
//! - 非交互实现返回 None（CI/管道模式）
//!
//! 让模型在需要人工判断或缺少信息时主动暂停并提问，而非盲目猜测。

use async_trait::async_trait;
use miniagent_core::error::AgentError;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};
use tokio_util::sync::CancellationToken;

/// AskUser 工具：让 LLM 向用户提问并等待回复。
pub struct AskUserTool;

impl Default for AskUserTool { fn default() -> Self { Self } }
impl AskUserTool { pub fn new() -> Self { Self } }

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str { "ask_user" }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their response. Use this when you need clarification, \
         confirmation for risky actions, or information you cannot determine yourself."
    }

    fn class(&self) -> ToolClass { ToolClass::ReadOnly }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let question = input["question"].as_str()
            .ok_or_else(|| AgentError::tool("ask_user", "missing 'question'"))?;

        // 通过 ToolContext 的 UserPrompt 向用户提问
        match ctx.user_prompt.ask(question).await {
            Some(answer) => {
                Ok(ToolOutput {
                    content: answer,
                    metadata: None,
                })
            }
            None => {
                // 非交互模式（CI/管道）或用户未回答
                Ok(ToolOutput {
                    content: "(Cannot ask user in non-interactive mode or user did not answer)".to_string(),
                    metadata: None,
                })
            }
        }
    }
}
