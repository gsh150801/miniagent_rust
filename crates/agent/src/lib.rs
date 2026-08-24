pub mod context;
pub mod agent_tool;

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

/// Rough token estimate using UTF-8 byte count.
///
/// 参考 cc-python-claude 的 token_estimation.py：用 UTF-8 字节数而非字符数。
/// 英文 ~4 bytes/token，中文 UTF-8 编码后 ~3 bytes/字（每字约 1.5 token），
/// 用 bytes/4 对中英混合更准确。旧的 chars/3 对纯中文严重低估（中文 1 char ≈ 1.5 token，
/// chars/3 算成 0.33 token/char，偏差 4.5x）。
fn estimate_history_tokens(history: &[Message]) -> usize {
    history
        .iter()
        .map(|m| m.text_content().len() / 4) // len() = UTF-8 字节数
        .sum()
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
            event_sender: None,
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

        let system = if memory_context.is_empty() {
            context.system_prompt.clone()
        } else {
            format!("{}\n\n{}", context.system_prompt, memory_context)
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

        // transcript 修复：在循环开始前修补孤立 tool_use（防 API 校验错误）
        let fixed = miniagent_core::message::validate_transcript(history);
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

        let keep_recent = self.keep_recent_msgs().min(history.len().saturating_sub(1));
        let discard_count = history.len().saturating_sub(keep_recent + 1);

        // Collect text from messages being discarded (owned, no borrow conflict)
        let discarded_text: String = history
            .iter()
            .skip(1)
            .take(discard_count)
            .map(|m| {
                let role = format!("{:?}", m.role);
                format!("[{role}] {}", m.text_content())
            })
            .collect::<Vec<_>>()
            .join("\n---\n");

        // Generate summary via LLM
        let summary = self
            .summarize_discarded(&discarded_text, context, &cancel)
            .await;

        // Persist to memory database
        if let Some(ref mem) = self.memory {
            let rec = miniagent_memory::types::StructuredSummary {
                raw_summary: summary.clone(),
                ..Default::default()
            };
            let _ = mem.store_paper_summary(
                "Context History Summary",
                &rec,
                &["context_summary".to_string()],
                None,
            );
        }

        // Persist to disk file
        Self::save_context_file(&summary);

        // Rebuild: prompt + summary + last 5 messages
        let first = history.first().cloned();
        let recent: Vec<Message> = history
            .iter()
            .rev()
            .take(keep_recent)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let mut trimmed = Vec::with_capacity(keep_recent + 2);
        if let Some(msg) = first {
            trimmed.push(msg);
        }
        trimmed.push(Message::assistant_text(format!(
            "[Context trimmed. Summary of earlier work:\n{summary}\n\n\
             Continue the task with the latest results below.]"
        )));
        trimmed.extend(recent);
        *history = trimmed;
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

        let request = CompletionRequest {
            system: "You are a context summarizer. Extract key findings, tool results, \
                     decisions, and progress into a concise summary. Use the same \
                     language as the input. Keep it under 500 words. Focus on what \
                     was accomplished and what information was gathered."
                .into(),
            messages: vec![Message::user(format!(
                "Summarize the key points from this conversation history:\n\n{truncated}"
            ))],
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

    /// Write the compressed context summary to disk.
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
