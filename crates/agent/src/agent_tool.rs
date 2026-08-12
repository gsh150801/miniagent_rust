//! AgentTool：LLM 可调用的"派生子智能体"工具（参考 cc-python-claude agent_tool.py）。
//!
//! 放在 agent crate 中（而非 tool crate）以避免循环依赖：
//! agent 依赖 tool（Agent 持有 ToolExecutor），所以 AgentTool 也必须在 agent 侧。
//!
//! 后台异步模式：spawn 子 agent 后立即返回 task_id，结果通过 broadcast 回传。

use std::sync::Arc;
use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::event::AgentEvent;
use miniagent_core::message::Message;
use miniagent_core::config::TaskComplexity;
use miniagent_tool::traits::{Tool, ToolClass, ToolContext, ToolOutput};
use tokio_util::sync::CancellationToken;

use crate::context::RunContext;
use crate::Agent;

/// 子 agent 最大并发数。
const MAX_CONCURRENT_SUBAGENTS: usize = 3;

/// AgentTool：派生子智能体。
pub struct AgentTool {
    agent: Arc<Agent>,
    completion_tx: tokio::sync::broadcast::Sender<AgentEvent>,
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl AgentTool {
    /// 创建 AgentTool。
    ///
    /// 返回 (AgentTool, Receiver) —— receiver 应通过
    /// `agent.set_sub_agent_rx(rx)` 注入父 Agent。
    pub fn new(agent: Arc<Agent>) -> (Self, tokio::sync::broadcast::Receiver<AgentEvent>) {
        let (tx, rx) = tokio::sync::broadcast::channel(64);
        let tool = Self {
            agent,
            completion_tx: tx,
            semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SUBAGENTS)),
        };
        (tool, rx)
    }

    fn sub_agent_system_prompt() -> String {
        format!(
            "You are a sub-agent. Given the task below, use available tools to complete it fully. \
             Don't over-engineer — do the minimum needed. \
             When done, respond with a concise report of what was done and key findings.\n\n{}",
            miniagent_core::context_info::env_block(".")
        )
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str { "agent" }

    fn description(&self) -> &str {
        "Spawn a sub-agent to handle a subtask autonomously. The sub-agent runs in the background \
         with its own context and tools. Use this to parallelize work or delegate focused subtasks. \
         The sub-agent's result will be available in subsequent turns."
    }

    fn class(&self) -> ToolClass { ToolClass::ReadOnly }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "A self-contained task description. Include all context the sub-agent needs."
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Max tool-call iterations (default: 10, max: 20)"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let task = input["task"].as_str()
            .ok_or_else(|| AgentError::tool("agent", "missing 'task' parameter"))?;
        let max_turns = input["max_turns"].as_u64().unwrap_or(10).min(20) as usize;

        // 检查并发数
        if self.semaphore.available_permits() == 0 {
            return Ok(ToolOutput {
                content: format!(
                    "Sub-agent queue full ({MAX_CONCURRENT_SUBAGENTS} running). \
                     Wait for existing sub-agents to complete."
                ),
                metadata: None,
            });
        }

        let task_id = format!("sub_{}", &uuid::Uuid::new_v4().to_string()[..8]);

        // 准备子 agent
        let agent = self.agent.clone();
        let working_dir = ctx.working_dir.clone();
        let completion_tx = self.completion_tx.clone();
        let semaphore = self.semaphore.clone();
        let system_prompt = Self::sub_agent_system_prompt();
        let task_text = task.to_string();
        let sub_task_id = task_id.clone();
        let sub_cancel = cancel.child_token();

        tokio::spawn(async move {
            let _permit = match semaphore.acquire_owned().await {
                Ok(p) => p,
                Err(_) => { tracing::error!("sub-agent semaphore closed"); return; }
            };

            tracing::info!(task_id = %sub_task_id, "sub-agent started");

            let project_md = miniagent_core::context_info::project_md_block(&working_dir)
                .unwrap_or_default();
            let mut history = vec![Message::user(format!("{task_text}\n\n{project_md}"))];

            let mut run_ctx = RunContext::new(&system_prompt)
                .with_complexity(TaskComplexity::Moderate);
            run_ctx.max_tool_iterations = max_turns;
            // 排除 "agent" 工具防递归
            run_ctx.allowed_tools = Some(sub_agent_allowed_tools());

            // 超时保护：子 agent 最多运行 300 秒（防止卡死永久占用 semaphore slot）
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(300),
                agent.run_with_loop(&mut history, &run_ctx, sub_cancel)
            ).await;

            let (result_text, success) = match result {
                Ok(Ok(delta)) => {
                    let text: String = history.iter()
                        .filter(|m| matches!(m.role, miniagent_core::message::MessageRole::Assistant))
                        .map(|m| m.text_content())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let text = if text.is_empty() {
                        delta.new_messages.iter().map(|m| m.text_content())
                            .collect::<Vec<_>>().join("\n")
                    } else { text };
                    (text, true)
                }
                Ok(Err(e)) => (format!("Sub-agent failed: {e}"), false),
                Err(_) => {
                    tracing::error!(task_id = %sub_task_id, "sub-agent timed out after 300s");
                    ("Sub-agent timed out after 300 seconds".to_string(), false)
                }
            };

            tracing::info!(task_id = %sub_task_id, success, len = result_text.len(), "sub-agent completed");
            let _ = completion_tx.send(AgentEvent::SubAgentCompleted {
                task_id: sub_task_id, result: result_text, success,
            });
        });

        Ok(ToolOutput {
            content: format!(
                "Sub-agent '{task_id}' started in background (max {max_turns} turns). \
                 Results will appear in subsequent turns."
            ),
            metadata: None,
        })
    }
}

/// 子 agent 允许的工具列表（排除 AgentTool 防递归）。
fn sub_agent_allowed_tools() -> Vec<String> {
    vec![
        "read".into(), "write".into(), "edit".into(),
        "glob".into(), "grep".into(), "bash".into(),
        "web_fetch".into(), "web_search".into(),
        "pubmed_search".into(), "git".into(), "conda".into(),
        "patent_search".into(), "clinical_trials_search".into(),
        "ask_user".into(), "notebook_edit".into(),
    ]
}

/// 构造带 AgentTool 的工具集。
///
/// 返回 (ToolRegistry, Receiver) —— receiver 注入父 Agent 的 set_sub_agent_rx。
pub fn build_tools_with_agent(
    agent: Arc<Agent>,
) -> (miniagent_tool::registry::ToolRegistry, tokio::sync::broadcast::Receiver<AgentEvent>) {
    let mut registry = miniagent_tool::tools::defaults();
    let (agent_tool, rx) = AgentTool::new(agent);
    registry.register(agent_tool);
    (registry, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_tools_exclude_agent() {
        let tools = sub_agent_allowed_tools();
        assert!(!tools.contains(&"agent".to_string()), "agent tool must be excluded");
        assert!(tools.contains(&"read".to_string()));
        assert!(tools.contains(&"bash".to_string()));
    }
}
