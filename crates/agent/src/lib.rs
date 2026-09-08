pub mod context;
pub mod agent_tool;
pub mod compaction;

use std::sync::Arc;

use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::error::AgentError;
use miniagent_core::event::{AgentEvent, ContentBlock, StopReason, Usage};
use miniagent_core::message::Message;
use miniagent_core::types::RunId;
use miniagent_memory::manager::MemoryManager;
use miniagent_memory::ConsolidationLevel;
use miniagent_provider::router::ProviderRouter;
use miniagent_provider::traits::{CompletionRequest, LlmProvider, ToolDef};
use miniagent_tool::executor::{ToolCallRequest, ToolExecutor};
use miniagent_tool::traits::ToolContext;
use tokio_util::sync::CancellationToken;

pub use context::RunContext;

/// Max estimated tokens before we trim history (leaves room for output in 128K window)
const MAX_HISTORY_TOKENS: usize = 96_000;
/// Max chars from discarded messages to feed into the summariser
const SUMMARIZE_CHAR_LIMIT: usize = 12_000;
/// Number of recent messages to keep verbatim during summarization
const KEEP_RECENT_MSGS: usize = 5;
/// Max consecutive all-error tool rounds before breaking the agent loop
const MAX_CONSECUTIVE_ERRORS: usize = 3;

/// 近似 token 估算（CJK 感知，含 ToolUse input）。
///
/// 委托给 core::budget 的共享实现：CJK 按 1 token/字（旧 bytes/4 对
/// 中文低估 3-4 倍，导致 trim 触发太晚——live：估算 96K 时实际已
/// 40 万+ token，MiniMax 直接拒绝请求），ToolUse 的 input JSON 计入
/// （write 任务的全部交付内容都在参数里，text_content() 看不见）。
fn estimate_history_tokens(history: &[Message]) -> usize {
    miniagent_core::budget::estimate_history_tokens(history)
}

pub struct Agent {
    /// 运行时可替换的 provider 路由（RwLock 支持在 Arc<Agent> 上调用
    /// replace_providers 热切换模型，无需重建 Agent）。
    provider_router: Arc<std::sync::RwLock<ProviderRouter>>,
    /// 运行时可替换的 ToolExecutor（用 Arc<Mutex<Option<...>>> 支持
    /// 在 Arc<Agent> 上调用 replace_tools 而不需要 &mut self）。
    tool_executor: Arc<std::sync::Mutex<Option<Arc<ToolExecutor>>>>,
    memory: Option<Arc<MemoryManager>>,
    config: Option<Arc<miniagent_core::settings::AppConfig>>,
    event_sender: Option<Arc<tokio::sync::Mutex<Vec<tokio::sync::broadcast::Sender<AgentEvent>>>>>,
    sub_agent_rx: Option<std::sync::Mutex<tokio::sync::broadcast::Receiver<AgentEvent>>>,
}

// Agent 含 `Box<dyn LlmProvider>` 等不可 Debug 的字段，无法 derive(Debug)。
// 手写一个占位 Debug，使持有 `Option<Arc<Agent>>` 的容器（如 planning::Blackboard）
// 仍能 derive(Debug)。
impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("has_tools", &self.tool_executor.lock().map(|e| e.is_some()).unwrap_or(false))
            .field("has_memory", &self.memory.is_some())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct AgentDelta {
    pub new_messages: Vec<Message>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

impl Agent {
    pub fn new(flash: Box<dyn LlmProvider>, pro: Box<dyn LlmProvider>) -> Self {
        Self {
            provider_router: Arc::new(std::sync::RwLock::new(ProviderRouter::new(flash, pro))),
            tool_executor: Arc::new(std::sync::Mutex::new(None)),
            memory: None,
            config: None,
            // 空容器而非 None：None 会让 emit_event 静默丢弃所有事件、
            // register_event_sender 返回空操作 guard（live: workflow 与
            // loop 模式的工具事件从未到达前端，操作卡不渲染，根因即此）。
            event_sender: Some(Arc::new(tokio::sync::Mutex::new(Vec::new()))),
            sub_agent_rx: None,
        }
    }

    /// 运行时热切换 flash/pro provider（模型注册表激活新模型时调用）。
    /// 进行中的请求继续用旧 provider 完成，新请求立即生效。
    pub fn replace_providers(
        &self,
        flash: std::sync::Arc<dyn LlmProvider>,
        pro: std::sync::Arc<dyn LlmProvider>,
    ) {
        *self.provider_router.write().unwrap() = ProviderRouter::new_arc(flash, pro);
    }

    pub fn with_tools(self, executor: ToolExecutor) -> Self {
        *self.tool_executor.lock().unwrap() = Some(Arc::new(executor));
        self
    }

    /// 运行时替换 ToolExecutor（用于 AgentTool server 接入：
    /// 先构建 Agent→包 Arc→构造 AgentTool→replace_tools 注入含 AgentTool 的 registry）。
    pub fn replace_tools(&self, executor: ToolExecutor) {
        *self.tool_executor.lock().unwrap() = Some(Arc::new(executor));
    }

    pub fn with_memory(mut self, memory: MemoryManager) -> Self {
        self.memory = Some(Arc::new(memory));
        self
    }

    /// Attach an `AppConfig` so runtime parameters (history limits, error
    /// thresholds, etc.) are read from `.env` instead of compiled-in defaults.
    pub fn with_config(mut self, config: Arc<miniagent_core::settings::AppConfig>) -> Self {
        self.config = Some(config);
        self
    }

    /// Effective history token limit: from config if attached, else const fallback.
    fn history_token_limit(&self) -> usize {
        self.config.as_ref()
            .map(|c| c.agent_history_token_limit)
            .unwrap_or(MAX_HISTORY_TOKENS)
    }

    /// Effective keep-recent count: from config if attached, else const fallback.
    fn keep_recent_msgs(&self) -> usize {
        self.config.as_ref()
            .map(|c| c.agent_keep_recent_msgs)
            .unwrap_or(KEEP_RECENT_MSGS)
    }

    /// Effective max consecutive errors: from config if attached, else const fallback.
    fn max_consecutive_errors(&self) -> usize {
        self.config.as_ref()
            .map(|c| c.agent_max_consecutive_errors)
            .unwrap_or(MAX_CONSECUTIVE_ERRORS)
    }

    /// Owned provider handles (safe to hold across `.await`).
    /// flash 用于简单任务，pro 用于复杂/推理任务。
    pub fn flash_provider(&self) -> std::sync::Arc<dyn LlmProvider> {
        self.provider_router.read().unwrap().flash_arc()
    }

    pub fn pro_provider(&self) -> std::sync::Arc<dyn LlmProvider> {
        self.provider_router.read().unwrap().pro_arc()
    }

    pub fn memory(&self) -> Option<&MemoryManager> {
        self.memory.as_deref()
    }

    pub fn tool_executor(&self) -> Option<std::sync::MutexGuard<'_, Option<Arc<ToolExecutor>>>> {
        self.tool_executor.lock().ok()
    }

    /// Single turn: user prompt → agent response (no tool loop)
    pub async fn run(
        &self,
        history: &[Message],
        context: &RunContext,
        cancel: CancellationToken,
    ) -> Result<AgentDelta, AgentError> {
        let provider = self
            .provider_router
            .read()
            .unwrap()
            .select_arc(context.complexity, context.provider_override);
        let mut inference_config = Self::config_for_complexity(context.complexity);
        if let Some(max_tokens) = context.max_tokens {
            inference_config.max_tokens = Some(max_tokens.min(393216));
        }

        // Gather tool definitions if available, optionally filtered by allowed_tools.
        let tools: Vec<ToolDef> = {
            let guard = self.tool_executor.lock().unwrap();
            guard.as_ref().map(|e| {
                let mut defs: Vec<ToolDef> = e
                    .registry()
                    .get_definitions()
                    .into_iter()
                    .map(|d| ToolDef {
                        name: d.name,
                        description: d.description,
                        parameters: d.parameters,
                    })
                    .collect();

                if let Some(ref allowed) = context.allowed_tools {
                    defs.retain(|d| allowed.iter().any(|a| a == &d.name));
                }

                defs
            }).unwrap_or_default()
        };

        // Assemble memory context
        let memory_context = if let Some(ref mem) = self.memory {
            let assembled = mem.assemble_context(
                &history.iter().map(|m| m.text_content()).collect::<Vec<_>>().join(" "),
                5,
            );
            assembled.memory_context
        } else {
            String::new()
        };

        // ── 上下文占用透明化（MemGPT memory-pressure 思想）──────────
        // LLM 知道当前 token 占用、窗口预算与预算占比，可以主动调整
        // 行为（少读全文、及时收尾）。占用接近预算时附加行动指令：
        // 立即总结收尾，别再发起大结果工具调用。
        let used = miniagent_core::budget::estimate_history_tokens(history);
        let budget = self.history_token_limit();
        let system = if memory_context.is_empty() {
            context.system_prompt.clone()
        } else {
            format!("{}\n\n{}", context.system_prompt, memory_context)
        };
        let pct = (used as f64 / budget.max(1) as f64 * 100.0) as u64;
        let pressure_note = if pct >= 75 {
            format!(
                "\n\n⚠️ CONTEXT PRESSURE: ~{used} tokens used of ~{budget} budget ({pct}%). \
                 Wrap up NOW: summarize findings and produce your final answer. \
                 Do NOT start new tool calls that return large outputs. \
                 If you have not done so recently, record essential state via write_note."
            )
        } else {
            format!(
                "\n\n[Context: ~{used}/~{budget} tokens ({pct}%). Tool outputs are capped (~32KB, full text offloaded to file with path in the result). \
                 Prefer read(offset/limit) over whole-file reads.]"
            )
        };
        // Codex 式状态记忆层：write_note 落盘的 notes 每轮注入——历史
        // 正文可能被裁剪，notes 是模型唯一可靠的长期记忆。
        let notes_mode = self.config.as_ref().map(|c| c.agent_notes_mode).unwrap_or(false);
        let notes_block = if !context.working_dir.is_empty() {
            miniagent_tool::tools::write_note::render_notes_block(&context.working_dir)
        } else {
            String::new()
        };
        let system = format!("{system}{pressure_note}{notes_block}");
        let system = if notes_mode {
            format!(
                "{system}\n\n[NOTES MODE: 历史正文会在上下文超限时移出窗口归档（不会销毁）。\
                 务必在关键节点用 write_note 记录状态：目标细化/决策敲定时写 goal，\
                 里程碑完成写 done，假设被否决时写 rejected 及原因，产物落盘写 artifacts，\
                 并注明信息来源（如归档 item_id、文件路径）。\
                 需要被归档的历史细节时用 search_history（关键词检索）或按 item_id 读取；\
                 窗口外历史不会自动回来，但永远可以查回。]"
            )
        } else {
            system
        };

        let request = CompletionRequest {
            system,
            messages: history.to_vec(),
            tools,
            config: inference_config,
        };

        let response = provider.complete(&request, cancel).await?;
        let new_messages = Self::response_to_messages(&response);

        Ok(AgentDelta {
            new_messages,
            stop_reason: response.stop_reason,
            usage: response.usage,
        })
    }

    /// Fire-and-forget event emission to every registered broadcast sender.
    ///
    /// Multiple consumers can subscribe concurrently (e.g. one WebSocket per
    /// running task); each gets its own cloned sender so a slow or dropped
    /// consumer does not steal events from the others.
    pub async fn emit_event(&self, event: AgentEvent) {
        if let Some(ref inner) = self.event_sender {
            let guard = inner.lock().await;
            for sender in guard.iter() {
                let _ = sender.send(event.clone());
            }
        }
    }

    /// Register a new broadcast sender and return a guard that removes it
    /// when dropped. Each call returns an independent sender, so concurrent
    /// runs no longer clobber each other's event streams.
    ///
    /// The guard's `Drop` impl is best-effort: it locks the inner vec and
    /// removes its sender. If the runtime is already shutting down, the
    /// senders are simply dropped together with the Agent.
    pub async fn register_event_sender(
        &self,
        sender: tokio::sync::broadcast::Sender<AgentEvent>,
    ) -> EventSenderGuard {
        if let Some(ref inner) = self.event_sender {
            let mut guard = inner.lock().await;
            // Drop senders whose receivers are gone to keep the list small.
            guard.retain(|s| s.receiver_count() > 0);
            guard.push(sender.clone());
            let shared = Arc::clone(inner);
            let raw = sender.clone();
            EventSenderGuard {
                inner: Some(shared),
                sender: Some(raw),
            }
        } else {
            // No container: emit nothing and return a no-op guard.
            EventSenderGuard {
                inner: None,
                sender: None,
            }
        }
    }

    /// Set the sub-agent completion receiver (for AgentTool).
    /// When set, run_with_loop will collect completed sub-agent results between iterations.
    pub fn set_sub_agent_rx(&self, rx: tokio::sync::broadcast::Receiver<AgentEvent>) {
        if let Some(ref inner) = self.sub_agent_rx {
            let mut guard = inner.lock().unwrap();
            *guard = rx;
        }
    }

    /// Collect any completed sub-agent results into history (non-blocking).
    fn collect_sub_agent_results(&self, history: &mut Vec<Message>) {
        let Some(ref inner) = self.sub_agent_rx else { return };
        let mut guard = match inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        loop {
            match guard.try_recv() {
                Ok(AgentEvent::SubAgentCompleted { task_id, result, success }) => {
                    let label = if success { "completed" } else { "failed" };
                    history.push(Message::tool(
                        &task_id,
                        format!("[Sub-agent {label}]\n{result}"),
                    ));
                }
                Ok(_) => {} // other events, ignore
                Err(_) => break,
            }
        }
    }

    /// Multi-turn with tool-call loop
    pub async fn run_with_loop(
        &self,
        history: &mut Vec<Message>,
        context: &RunContext,
        cancel: CancellationToken,
    ) -> Result<AgentDelta, AgentError> {
        let max_iterations = context.max_tool_iterations;
        let mut total_usage = Usage::default();
        let run_id = RunId::new();
        let mut last_delta = None;
        let mut consecutive_errors: usize = 0;
        let mut pairing_retries: u32 = 0;

        // transcript 修复：循环开始前做双向配对修复（孤立 tool_use 补合成
        // 结果且紧邻插入；孤立/重复 tool result 丢弃——后者是 MiniMax 400
        // "tool call result does not follow tool call (2013)" 的直接诱因）
        let fixed = miniagent_core::message::repair_tool_pairing(history);
        if fixed > 0 {
            tracing::info!(fixed, "transcript repaired at run_with_loop start");
        }

        self.emit_event(AgentEvent::RunStarted { run_id, timestamp: chrono::Utc::now() }).await;

        for iteration in 0..max_iterations {
            // LLM 调用 + 重试（参考 cc-python-claude query_loop 的错误恢复策略）
            // 429/529 瞬时错误 → 指数退避重试（最多 3 次）
            // 其他错误 → 直接失败
            let delta = {
                let mut retry_count = 0u32;
                let max_retries = 3u32;
                loop {
                    match self.run(history, context, cancel.child_token()).await {
                        Ok(d) => break d,
                        Err(e) => {
                            let err_str = e.to_string();
                            // 429 (rate limit) / 529 (overloaded) / 瞬时网络错误 → 重试
                            let is_transient = err_str.contains("429")
                                || err_str.contains("529")
                                || err_str.contains("rate limit")
                                || err_str.contains("overloaded")
                                || err_str.contains("connection")
                                || err_str.contains("timeout")
                                || err_str.contains("timed out");

                            if is_transient && retry_count < max_retries {
                                retry_count += 1;
                                let delay = std::time::Duration::from_secs(
                                    2u64.pow(retry_count) // 2s, 4s, 8s 指数退避
                                );
                                tracing::error!(
                                    retry = retry_count,
                                    max_retries = max_retries,
                                    delay_secs = delay.as_secs(),
                                    error = %err_str,
                                    "transient LLM error, retrying with backoff"
                                );
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                            // tool 配对违例（MiniMax 2013 / OpenAI 严格校验）：
                            // 机械修复 history 后立即重试，而非让整个子任务
                            // 失败。live: b3337de9 任务 6 个子任务因此全灭。
                            let is_pairing = err_str.contains("does not follow tool call")
                                || err_str.contains("(2013)")
                                || (err_str.contains("400") && err_str.contains("tool"));
                            if is_pairing && pairing_retries < 2 {
                                pairing_retries += 1;
                                let repaired = miniagent_core::message::repair_tool_pairing(history);
                                tracing::warn!(
                                    attempt = pairing_retries,
                                    repaired = repaired,
                                    error = %err_str,
                                    "tool-pairing provider error — repaired transcript, retrying"
                                );
                                continue;
                            }
                            // 非瞬时错误或重试耗尽 → 返回错误
                            return Err(e);
                        }
                    }
                }
            };
            let stop_reason = delta.stop_reason.clone();

            total_usage.input_tokens += delta.usage.input_tokens;
            total_usage.output_tokens += delta.usage.output_tokens;

            history.extend(delta.new_messages.clone());

            match stop_reason {
                StopReason::ToolUse => {
                    // Clone Arc<ToolExecutor> and immediately drop the guard
                    // to avoid holding MutexGuard across .await points (not Send).
                    let executor_opt = {
                        let guard = self.tool_executor.lock().unwrap();
                        guard.clone()
                    };
                    if let Some(ref executor) = executor_opt {
                        let last_msg = history.last().unwrap();
                        let raw_tool_calls: Vec<(ToolCallRequest, serde_json::Value)> = last_msg
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, name, input } => {
                                    let req = ToolCallRequest {
                                        id: *id,
                                        name: name.clone(),
                                        input: input.clone(),
                                    };
                                    Some((req, serde_json::json!({
                                        "tool_name": name,
                                        "input": input,
                                    })))
                                }
                                _ => None,
                            })
                            .collect();

                        if raw_tool_calls.is_empty() {
                            last_delta = Some(AgentDelta {
                                new_messages: vec![],
                                stop_reason,
                                usage: total_usage.clone(),
                            });
                            break;
                        }

                        let tool_calls: Vec<ToolCallRequest> = raw_tool_calls.into_iter()
                            .map(|(req, _)| req)
                            .collect();

                        // Emit tool-call-start events (fire-and-forget).
                        for tc in &tool_calls {
                            // 日志：记录工具调用请求（target=tool_call，放行到 info 级别）
                            tracing::info!(
                                target: "tool_call",
                                call_id = ?tc.id,
                                tool = %tc.name,
                                input = %tc.input,
                                "tool_call_requested",
                            );
                            self.emit_event(AgentEvent::ToolCallRequested {
                                call_id: tc.id,
                                tool_name: tc.name.clone(),
                                input: tc.input.clone(),
                            }).await;
                        }

                        let ctx = ToolContext::new(
                            context.working_dir.clone(),
                            format!("{}", run_id.0),
                        );

                        let results = executor
                            .execute_batch(&tool_calls, &ctx, cancel.child_token())
                            .await;

                        // ── Tool-call logging (target=tool_call) ──
                        for (call_id, output) in &results {
                            let tool_name = tool_calls.iter()
                                .find(|tc| tc.id == *call_id)
                                .map(|tc| tc.name.as_str())
                                .unwrap_or("unknown");
                            let is_error = output.metadata.as_ref()
                                .map(|m| m.is_error).unwrap_or(false);
                            let latency = output.metadata.as_ref()
                                .map(|m| m.duration_ms).unwrap_or(0);
                            if is_error {
                                tracing::error!(
                                    target: "tool_call",
                                    call_id = ?call_id,
                                    tool = %tool_name,
                                    duration_ms = latency,
                                    result = %output.content.chars().take(500).collect::<String>(),
                                    "tool_call_failed",
                                );
                            } else {
                                tracing::info!(
                                    target: "tool_call",
                                    call_id = ?call_id,
                                    tool = %tool_name,
                                    duration_ms = latency,
                                    result = %output.content.chars().take(500).collect::<String>(),
                                    "tool_call_completed",
                                );
                            }
                        }

                        // Track errors: break if all tool calls failed
                        let error_count = results.iter()
                            .filter(|(_, o)| o.content.starts_with("Error:"))
                            .count();
                        if error_count == results.len() && !results.is_empty() {
                            consecutive_errors += 1;
                        } else {
                            consecutive_errors = 0;
                        }

                        // Append tool results to the transcript.
                        for (call_id, output) in results {
                            let tool_name = tool_calls.iter()
                                .find(|tc| tc.id == call_id)
                                .map(|tc| tc.name.clone())
                                .unwrap_or_default();
                            let duration_ms = output.metadata.as_ref()
                                .map(|m| m.duration_ms)
                                .unwrap_or(0);
                            let is_error = output.metadata.as_ref()
                                .map(|m| m.is_error)
                                .unwrap_or(false);
                            // Emit tool-call-complete event (fire-and-forget).
                            self.emit_event(AgentEvent::ToolCallCompleted {
                                call_id,
                                tool_name,
                                output: output.content.clone(),
                                duration_ms,
                                is_error,
                            }).await;
                            history.push(Message::tool(
                                format!("{}", call_id.0),
                                &output.content,
                            ));
                        }

                        // Trim history if it exceeds context window budget
                        self.trim_and_summarize_history(
                            history,
                            context,
                            cancel.child_token(),
                        )
                        .await;
                        // P-修复：trim 后做双向配对修复——压缩窗口可能切在
                        // 工具序列中间（live: MiniMax 400 "tool call result
                        // does not follow tool call"）
                        miniagent_core::message::repair_tool_pairing(history);

                        // Break on too many consecutive all-error rounds
                        if consecutive_errors >= self.max_consecutive_errors() {
                            last_delta = Some(AgentDelta {
                                new_messages: vec![],
                                stop_reason: StopReason::EndTurn,
                                usage: total_usage.clone(),
                            });
                            break;
                        }
                    } else {
                        last_delta = Some(AgentDelta {
                            new_messages: vec![],
                            stop_reason,
                            usage: total_usage.clone(),
                        });
                        break;
                    }
                }
                StopReason::MaxTokens
                    // 参考 cc-python-claude query_loop：输出被截断时追加"请继续"续写，
                    // 而非直接终止（丢失后续内容）。最多续写 3 次防无限循环。
                    if iteration < max_iterations.saturating_sub(1) => {
                        tracing::info!(
                            iteration,
                            "output truncated (MaxTokens), appending 'continue' to resume"
                        );
                        history.push(Message::user("Please continue from where you left off."));
                        // 不 break，继续下一轮循环让 LLM 续写
                    }
                _ => {
                    last_delta = Some(AgentDelta {
                        new_messages: vec![],
                        stop_reason,
                        usage: total_usage.clone(),
                    });
                    break;
                }
            }

            // 收集已完成的子 agent 结果（AgentTool 后台异步模式）
            self.collect_sub_agent_results(history);
        }

        // 迭代预算耗尽且最后没有可交付文本 → 强制一次"只写最终答案"调用。
        // live（b3337de9 loop4/5）：worker 在迭代上限内完成了调研但没来得及
        // 写最终交付物，dispatch 的 extract_final_deliverable 只拿到过程
        // 叙述 → "Insufficient output (tool calls only)" 整任务失败。
        let has_final_text = history.iter().rev().take(3).any(|m| {
            matches!(m.role, miniagent_core::message::MessageRole::Assistant) && !m.text_content().trim().is_empty()
        });
        if !has_final_text {
            tracing::warn!("tool-iteration budget exhausted without a final answer — forcing one text-only completion call");
            history.push(Message::user(
                "You have reached the tool-iteration limit. Based on all work completed so far, \
                 write your final deliverable NOW as plain text. Do NOT call any more tools.",
            ));
            match self.run(history, context, cancel.child_token()).await {
                Ok(delta) => {
                    total_usage.input_tokens += delta.usage.input_tokens;
                    total_usage.output_tokens += delta.usage.output_tokens;
                    last_delta = Some(delta);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "forced final-answer call failed — returning what we have");
                }
            }
        }

        // Episode-end consolidation
        if let Some(ref mem) = self.memory {
            mem.consolidate(ConsolidationLevel::EpisodeEnd).await;
        }

        let final_delta = last_delta.unwrap_or(AgentDelta {
            new_messages: vec![],
            stop_reason: StopReason::EndTurn,
            usage: total_usage.clone(),
        });
        let event = AgentEvent::RunCompleted {
            run_id,
            stop_reason: final_delta.stop_reason.clone(),
            usage: final_delta.usage.clone(),
            timestamp: chrono::Utc::now(),
        };
        self.emit_event(event).await;

        Ok(final_delta)
    }

    /// Trim history with LLM summarization: keep prompt + summary + last 5 messages.
    /// Saves the compressed context to memory DB and a disk file.
    /// 历史压缩入口（薄分发）：估算超预算时按生效模式路由到可插拔
    /// compactor（见 compaction 模块：llm_summary / notes_history）。
    async fn trim_and_summarize_history(
        &self,
        history: &mut Vec<Message>,
        context: &RunContext,
        cancel: CancellationToken,
    ) {
        if history.len() < 6 {
            return;
        }
        if estimate_history_tokens(history) <= self.history_token_limit() {
            return;
        }
        crate::compaction::run_compaction(self, history, context, cancel).await;
    }

    /// Ask the LLM to summarise discarded conversation turns.
    async fn summarize_discarded(
        &self,
        text: &str,
        _context: &RunContext,
        cancel: &CancellationToken,
    ) -> String {
        if text.is_empty() {
            return "(no previous context)".into();
        }

        let truncated: String = text.chars().take(SUMMARIZE_CHAR_LIMIT).collect();

        let provider = self
            .provider_router
            .read()
            .unwrap()
            .select_arc(TaskComplexity::Simple, None);

        // P-记忆机制 Layer A：结构化压缩（方案 D / A 部分）。过程性叙述
        // 折叠，四类结构化信息必须保留——约束、关键决策与结论、产物清单、
        // 待办。丢失前两类会导致后续轮次违反用户要求（live 事故），丢失
        // 产物清单会导致重复劳动，丢失待办会导致任务静默缩水。
        let request = CompletionRequest {
            system: Self::compaction_system_prompt().into(),
            messages: vec![Message::user(Self::compaction_user_prompt(&truncated))],
            tools: vec![],
            config: InferenceConfig {
                max_tokens: Some(2048),
                ..Default::default()
            },
        };

        match provider.complete(&request, cancel.child_token()).await {
            Ok(response) => response
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Err(_) => {
                // Fallback: extract key lines from discarded text
                truncated
                    .lines()
                    .filter(|l| !l.is_empty())
                    .take(20)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }

    /// P-记忆机制 Layer A：四段式结构化压缩的提示词（约束/决策/产物/待办）。
    fn compaction_system_prompt() -> &'static str {
        "You are a context compactor for a long-running agent. \
         Compress the discarded conversation into FOUR structured sections, \
         keeping durable information and dropping process narration. Use the same \
         language as the input. Be precise: file paths, numbers, names, and \
         constraints must be preserved verbatim. Total under 600 words."
    }

    fn compaction_user_prompt(truncated: &str) -> String {
        format!(
            "Compress this conversation history into:\n\
             ### 约束（用户提出的要求与限制）\n- ...\n\
             ### 关键决策与结论\n- ...\n\
             ### 产物清单（文件路径/数据集/关键数字）\n- ...\n\
             ### 待办/未完成\n- ...\n\n\
             Conversation history:\n\n{truncated}"
        )
    }

    /// Append a compaction record to <working_dir>/compaction_history.json.
    fn append_compaction_record(working_dir: &str, discarded: usize, summary: &str) {
        let dir = std::path::PathBuf::from(working_dir);
        if !dir.is_dir() {
            return;
        }
        let path = dir.join("compaction_history.json");
        let mut list: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        list.push(serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "discarded_messages": discarded,
            "summary": summary,
        }));
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Write the compressed context summary to disk.
    /// 把被裁剪的历史归档到 `<working_dir>/history_archive.jsonl`。
    ///
    /// 对齐 Codex 的 history 层：详情只移出窗口、不销毁。每条记录带
    /// 全局递增 item_id（跨多次 trim 连续），供 search_history /
    /// read_history_item 工具按需查回。返回本次写入的 item_id 列表。
    /// 从将被裁剪的历史中机械提取任务状态 checkpoint（无 LLM 参与）。
    /// 提取规则全部是结构性信号，不含任何主题硬编码：
    /// - goal：首条 user 消息（原始任务陈述）前 500 字
    /// - progress：最后一条有实质文本的 assistant 消息前 400 字
    /// - artifacts：历史中所有 write/edit 工具调用的 path 参数（去重，≤10）
    fn mechanical_checkpoint(messages: &[Message]) -> serde_json::Value {
        let mut goal = String::new();
        let mut progress = String::new();
        let mut artifacts: Vec<String> = Vec::new();

        for msg in messages {
            if goal.is_empty()
                && matches!(msg.role, miniagent_core::message::MessageRole::User)
            {
                let t: String = msg.text_content().chars().take(500).collect();
                if !t.trim().is_empty() {
                    goal = t;
                }
            }
            if matches!(msg.role, miniagent_core::message::MessageRole::Assistant) {
                let t = msg.text_content();
                if !t.trim().is_empty() {
                    progress = t.chars().take(400).collect();
                }
            }
            for b in &msg.content {
                if let miniagent_core::event::ContentBlock::ToolUse { name, input, .. } = b {
                    if matches!(name.as_str(), "write" | "edit") {
                        if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                            if !artifacts.iter().any(|a| a == p) && artifacts.len() < 10 {
                                artifacts.push(p.to_string());
                            }
                        }
                    }
                }
            }
        }

        let mut sections = serde_json::Map::new();
        if !goal.trim().is_empty() {
            sections.insert("goal".into(), serde_json::json!(goal));
        }
        if !progress.trim().is_empty() {
            sections.insert("done".into(), serde_json::json!(vec![format!(
                "[机械提取自被裁剪历史] 最近进展: {progress}"
            )]));
        }
        if !artifacts.is_empty() {
            sections.insert("artifacts".into(), serde_json::json!(artifacts));
        }
        serde_json::Value::Object(sections)
    }

    /// 把机械 checkpoint 种入 notes.json（保留已有节，仅补充缺失节）。
    fn seed_notes_from_checkpoint(working_dir: &str, checkpoint: &serde_json::Value) {
        let Some(new_sections) = checkpoint.as_object() else { return };
        let mut notes = miniagent_tool::tools::write_note::load_notes(working_dir);
        let existing = notes
            .get("sections")
            .and_then(|s| s.as_object())
            .cloned()
            .unwrap_or_default();
        let mut merged = existing;
        for (k, v) in new_sections {
            // 已有节不覆盖（模型/前次提取的内容优先）
            merged.entry(k.clone()).or_insert_with(|| v.clone());
        }
        notes["sections"] = serde_json::json!(merged);
        let _ = miniagent_tool::tools::write_note::save_notes(working_dir, &notes);
    }

    fn append_history_archive(working_dir: &str, messages: &[Message]) -> Vec<u64> {
        use std::io::Write as _;
        let path = std::path::Path::new(working_dir).join("history_archive.jsonl");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 续接既有 item_id（跨 trim 会话连续）
        let mut next_id = 1u64;
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if let Some(last_line) = existing.lines().rev().find(|l| !l.trim().is_empty()) {
                if let Ok(prev) = serde_json::from_str::<serde_json::Value>(last_line) {
                    next_id = prev.get("item_id").and_then(|v| v.as_u64()).unwrap_or(0) + 1;
                }
            }
        }
        let mut ids = Vec::with_capacity(messages.len());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            for msg in messages {
                let role = format!("{:?}", msg.role).to_lowercase();
                let text = msg.text_content();
                let tool_input: Vec<String> = msg.content.iter().filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::ToolUse { name, input, .. } => {
                        Some(format!("{name} {}", serde_json::to_string(input).unwrap_or_default()))
                    }
                    _ => None,
                }).collect();
                let body = if text.is_empty() { tool_input.join("\n") } else { text };
                if body.trim().is_empty() {
                    continue;
                }
                let rec = serde_json::json!({
                    "item_id": next_id,
                    "role": role,
                    "chars": body.len(),
                    "content": body.chars().take(20_000).collect::<String>(),
                });
                if writeln!(f, "{rec}").is_ok() {
                    ids.push(next_id);
                }
                next_id += 1;
            }
        }
        ids
    }

    fn save_context_file(summary: &str) {
        // Anchored under the workspace root (was `./miniagent_context`
        // relative to the process CWD, which scattered dump files whenever
        // the binary was launched from another directory).
        let dir = miniagent_core::paths::workspace_root().join(".miniagent_context");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("history_{ts}.md"));
        let _ = std::fs::write(&path, summary);
    }

    fn config_for_complexity(complexity: TaskComplexity) -> InferenceConfig {
        match complexity {
            TaskComplexity::Simple => InferenceConfig::flash(),
            TaskComplexity::Moderate => InferenceConfig::flash(),
            TaskComplexity::Complex => InferenceConfig::pro(),
            TaskComplexity::DeepResearch => InferenceConfig::pro_deep(),
        }
    }

    fn response_to_messages(response: &miniagent_provider::traits::CompletionResponse) -> Vec<Message> {
        if response.content.is_empty() {
            return vec![];
        }
        vec![Message::assistant(response.content.clone())]
    }
}

/// RAII guard that unregisters a per-task broadcast sender when dropped.
///
/// Returned from [`Agent::register_event_sender`]. Each running task holds
/// one guard; dropping it (naturally or via panic) ensures the shared
/// `Agent`'s event list doesn't grow without bound and that a finished
/// task no longer receives events.
pub struct EventSenderGuard {
    inner: Option<Arc<tokio::sync::Mutex<Vec<tokio::sync::broadcast::Sender<AgentEvent>>>>>,
    sender: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
}

impl Drop for EventSenderGuard {
    fn drop(&mut self) {
        // Best-effort: if the runtime is still alive, take the lock and
        // remove our sender. We use a synchronous block_in_place-safe path
        // by spawning a tiny task; if that fails (no runtime), we leak the
        // sender and let it be cleaned up when the Agent is dropped.
        if let (Some(inner), Some(sender)) = (self.inner.take(), self.sender.take()) {
            let shared = Arc::clone(&inner);
            let raw = sender;
            // Try synchronous removal first via try_lock to avoid spawning.
            if let Ok(mut guard) = shared.try_lock() {
                guard.retain(|s| !s.same_channel(&raw));
                return;
            }
            // Fallback: hand off to the runtime if it's still around.
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let mut guard = shared.lock().await;
                    guard.retain(|s| !s.same_channel(&raw));
                });
            }
        }
    }
}

// ── P-记忆机制 Layer A 测试：结构化压缩 ─────────────────────────
#[cfg(test)]
mod mechanical_checkpoint_tests {
    use super::*;

    /// 机械 checkpoint 提取：goal/progress/artifacts 三节全部来自
    /// 结构性信号，无主题硬编码。
    #[test]
    fn mechanical_checkpoint_extracts_goal_progress_artifacts() {
        use miniagent_core::event::ContentBlock;
        use miniagent_core::types::ToolCallId;
        use miniagent_core::message::MessageRole;

        let mut history = Vec::new();
        history.push(Message::user("调研 mRNA 疫苗进展并写报告 report.md"));
        history.push(Message::assistant_text(""));
        let mut write_msg = Message::new(
            MessageRole::Assistant,
            vec![
                ContentBlock::Text { text: "已把调研结果写入报告文件。".into() },
                ContentBlock::ToolUse {
                    id: ToolCallId(uuid::Uuid::new_v4()),
                    name: "write".into(),
                    input: serde_json::json!({"path": "/task/report.md", "content": "..."}),
                },
            ],
        );
        let _ = &mut write_msg;
        history.push(write_msg);

        let cp = Agent::mechanical_checkpoint(&history);
        let obj = cp.as_object().unwrap();
        assert!(obj.contains_key("goal"), "应有 goal");
        assert!(obj["goal"].as_str().unwrap().contains("mRNA"), "goal 来自原始任务");
        assert!(obj.contains_key("done"), "应有 done（最近 assistant 陈述）");
        assert!(obj["done"].as_array().unwrap()[0].as_str().unwrap().contains("报告文件"));
        assert!(obj.contains_key("artifacts"), "应有 artifacts");
        assert_eq!(obj["artifacts"].as_array().unwrap()[0], "/task/report.md");
    }
}

/// 共享测试支持：捕获 prompt 的 stub provider（返回固定的四段式
/// 结构化压缩结果）。供 lib.rs 与 compaction 子模块的测试使用。
#[cfg(test)]
pub mod test_support {
    use super::*;
    use miniagent_core::config::InferenceConfig;
    use miniagent_core::error::AgentError;
    use miniagent_core::event::{ContentBlock, StopReason};
    use miniagent_provider::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamResponse};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    pub struct CapturingProvider(pub std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl CapturingProvider {
        pub fn new() -> Self {
            Self(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
        }
        pub fn prompts(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for CapturingProvider {
        async fn complete(
            &self,
            req: &CompletionRequest,
            _cancel: CancellationToken,
        ) -> Result<CompletionResponse, AgentError> {
            let user_text = req
                .messages
                .last()
                .map(|m| m.text_content())
                .unwrap_or_default();
            self.0.lock().unwrap().push(user_text);
            Ok(CompletionResponse {
                content: vec![ContentBlock::Text {
                    text: "### 约束（用户提出的要求与限制）\n- 只使用 python3\n\n\
                           ### 关键决策与结论\n- fib(30)=832040\n\n\
                           ### 产物清单（文件路径/数据集/关键数字）\n- fib.py\n\n\
                           ### 待办/未完成\n- 无"
                        .into(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Default::default(),
            })
        }

        async fn stream(
            &self,
            _req: &CompletionRequest,
            _cancel: CancellationToken,
        ) -> Result<StreamResponse, AgentError> {
            Err(AgentError::internal("stub does not support stream"))
        }
    }

    #[tokio::test]
    async fn structured_compaction_returns_four_section_summary() {
        let provider = CapturingProvider::new();
        let agent = Agent::new(
            Box::new(provider.clone()) as Box<dyn LlmProvider>,
            Box::new(provider.clone()) as Box<dyn LlmProvider>,
        );

        let summary = agent
            .summarize_discarded(
                "用户要求只使用 python3。agent 创建了 fib.py 并验证 fib(30)=832040。剩余：统计 miss 数。",
                &RunContext::new("system"),
                &CancellationToken::new(),
            )
            .await;

        for section in ["约束", "关键决策与结论", "产物清单", "待办"] {
            assert!(summary.contains(section), "summary must contain {section}");
        }
        let prompts = provider.prompts();
        assert_eq!(prompts.len(), 1, "exactly one summarizer call");
        assert!(prompts[0].contains("fib(30)=832040"), "source text was sent");
    }

    #[test]
    fn compaction_prompts_request_four_sections() {
        let p = Agent::compaction_user_prompt("some long history");
        for section in ["约束", "关键决策与结论", "产物清单", "待办/未完成"] {
            assert!(p.contains(section), "prompt must contain {section}");
        }
        assert!(Agent::compaction_system_prompt().contains("FOUR structured sections"));
    }

    #[tokio::test]
    async fn trim_never_orphans_tool_results() {
        // live 事故复现：历史以 [assistant tool_use, tool, tool] 结尾且
        // keep_recent 覆盖到 tool 序列——旧实现裁剪后 recent 以孤立 Tool
        // 消息开头，MiniMax 400 "tool call result does not follow tool call"。
        let provider = CapturingProvider::new();
        let agent = Agent::new(
            Box::new(provider.clone()) as Box<dyn LlmProvider>,
            Box::new(provider.clone()) as Box<dyn LlmProvider>,
        );
        let work_dir = std::env::temp_dir().join("miniagent_trim_pair_test");
        let _ = std::fs::remove_dir_all(&work_dir);
        std::fs::create_dir_all(&work_dir).unwrap();

        let tool_id = format!("{}", uuid::Uuid::new_v4());
        let mut history = vec![Message::user("调研任务")];
        for i in 0..20 {
            history.push(Message::assistant_text(format!("step {i}: {}", "y".repeat(6_000))));
        }
        // 结尾：assistant 发起 tool_use → 两条 tool 结果（正是裁剪会切断的形态）
        history.push(Message::assistant(vec![miniagent_core::event::ContentBlock::ToolUse {
            id: miniagent_core::types::ToolCallId::new(),
            name: "web_search".into(),
            input: serde_json::json!({"query": "x"}),
        }]));
        history.push(Message::tool(tool_id.clone(), "result A"));
        history.push(Message::tool(tool_id, "result B"));

        let ctx = RunContext::new("system").with_working_dir(work_dir.to_string_lossy().to_string());
        let mut hist = history.clone();
        agent.trim_and_summarize_history(&mut hist, &ctx, CancellationToken::new()).await;

        // 修复后窗口向左扩张包含配对的 tool_use，或整体不裁剪——
        // 两种实现都不允许出现"孤立 Tool 开头"。
        assert!(
            hist.iter().filter(|m| m.role == miniagent_core::message::MessageRole::Tool).count() == 0
                || hist[1].role != miniagent_core::message::MessageRole::Tool,
            "kept window must not start with an orphaned Tool message"
        );
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn append_compaction_record_persists_versioned_list() {
        let dir = std::env::temp_dir().join("miniagent_compaction_test");
        std::fs::create_dir_all(&dir).unwrap();
        Agent::append_compaction_record(&dir.to_string_lossy(), 12, "第一轮压缩摘要");
        Agent::append_compaction_record(&dir.to_string_lossy(), 8, "第二轮压缩摘要");
        let path = dir.join("compaction_history.json");
        let list: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["discarded_messages"], 12);
        assert_eq!(list[1]["summary"], "第二轮压缩摘要");
        std::fs::remove_file(&path).ok();
    }

    /// 集成测试：超预算历史触发压缩闭环。
    /// 断言：①历史缩减为 首条+摘要+近5条；②摘要含四段式结构；
    /// ③compaction_history.json 持久化到 working_dir 且记录丢弃条数；
    /// ④喂给摘要器的文本包含早期关键约束（首条消息原样保留，天然不丢）。
    #[tokio::test]
    async fn compaction_triggers_on_oversized_history_and_persists() {
        let provider = CapturingProvider::new();
        let agent = Agent::new(
            Box::new(provider.clone()) as Box<dyn LlmProvider>,
            Box::new(provider.clone()) as Box<dyn LlmProvider>,
        );

        let work_dir = std::env::temp_dir().join("miniagent_compaction_integration");
        let _ = std::fs::remove_dir_all(&work_dir); // fresh dir per run (records accumulate otherwise)
        std::fs::create_dir_all(&work_dir).unwrap();

        // 首条消息带任务约束（压缩后原样保留，不进摘要器）
        let mut history = vec![Message::user(
            "任务：分析 dataset_X。约束：只使用 python3，输出 result.csv，随机种子 42。",
        )];
        // 60 条 × 7KB 长消息 → 估算 token ≈ 105k > 96k 预算
        let filler = format!("analysis step with detailed reasoning and numbers: {}", "x".repeat(7_000));
        for i in 0..60 {
            history.push(Message::assistant_text(format!("step {i}: {filler}")));
        }
        let before = history.len();

        let ctx = RunContext::new("system").with_working_dir(work_dir.to_string_lossy().to_string());
        agent
            .trim_and_summarize_history(&mut history, &ctx, CancellationToken::new())
            .await;

        // ① 历史缩减：首条 + 摘要 + 近 N 条（N=keep_recent 配置，.env 可调）
        assert!(history.len() < before, "history must shrink: {} -> {}", before, history.len());
        assert!(history.len() >= 3, "at least first + summary + 1 recent");
        // ② 首条原样保留（约束不丢）
        assert!(history[0].text_content().contains("随机种子 42"));
        // ③ 摘要消息含四段式结构（来自 stub 的固定四段输出）
        let summary_msg = history[1].text_content();
        assert!(summary_msg.contains("### 约束"), "summary must be structured");

        // ④ 持久化：compaction_history.json 记录丢弃条数 60
        let rec_path = work_dir.join("compaction_history.json");
        let list: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&rec_path).unwrap()).unwrap();
        assert_eq!(list.len(), 1);
        // discarded = before - 1(first kept) - keep_recent，keep_recent =
        // after - 2（first + summary）
        let expected_discard = 61 - history.len() as i64 + 2 - 1;
        assert_eq!(list[0]["discarded_messages"], expected_discard);

        // ⑤ 摘要器收到的丢弃文本包含早期 step 内容（输入层保留率保证）
        let prompts = provider.prompts();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("step 0:"), "discarded steps fed to summarizer");

        std::fs::remove_dir_all(&work_dir).ok();
        let _ = before;
    }

}
