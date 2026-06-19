pub mod context;
pub mod hooks;

use std::sync::Arc;

use miniagent_checkpoint::CheckpointStore;
use miniagent_core::checkpoint::Checkpoint;
use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::error::AgentError;
use miniagent_core::event::{AgentEvent, ContentBlock, StopReason, Usage};
use miniagent_core::message::Message;
use miniagent_core::types::{RunId, StepId};
use miniagent_memory::manager::MemoryManager;
use miniagent_memory::ConsolidationLevel;
use miniagent_provider::router::ProviderRouter;
use miniagent_provider::traits::{CompletionRequest, LlmProvider, ToolDef};
use miniagent_self_improve::SelfImprover;
use miniagent_tool::executor::{ToolCallRequest, ToolExecutor};
use miniagent_tool::traits::ToolContext;
use tokio_util::sync::CancellationToken;

use crate::hooks::{HookAction, HookEvent, HookRegistry};

pub use context::RunContext;

/// Max estimated tokens before we trim history (leaves room for output in 128K window)
const MAX_HISTORY_TOKENS: usize = 96_000;
/// Max chars from discarded messages to feed into the summariser
const SUMMARIZE_CHAR_LIMIT: usize = 12_000;
/// Number of recent messages to keep verbatim during summarization
const KEEP_RECENT_MSGS: usize = 5;
/// Max consecutive all-error tool rounds before breaking the agent loop
const MAX_CONSECUTIVE_ERRORS: usize = 3;

/// Rough token count: chars/3 works for mixed Chinese/English/code
fn estimate_history_tokens(history: &[Message]) -> usize {
    history
        .iter()
        .map(|m| m.text_content().chars().count() / 3)
        .sum()
}

pub struct Agent {
    provider_router: ProviderRouter,
    tool_executor: Option<Arc<ToolExecutor>>,
    memory: Option<Arc<MemoryManager>>,
    checkpoint_store: Option<Arc<CheckpointStore>>,
    self_improver: Option<Arc<tokio::sync::Mutex<SelfImprover>>>,
    hooks: Option<Arc<HookRegistry>>,
    config: Option<Arc<miniagent_core::settings::AppConfig>>,
    /// Optional broadcast channel so external consumers (e.g. the web server)
    /// can observe fine-grained events: tool call start/end, lifecycle, etc.
    /// Wrapped in Arc<Mutex<>> so it can be set on an Arc-shared agent without
    /// requiring &mut self.
    event_sender: Option<Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<AgentEvent>>>>>,
}

// Agent 含 `Box<dyn LlmProvider>` 等不可 Debug 的字段，无法 derive(Debug)。
// 手写一个占位 Debug，使持有 `Option<Arc<Agent>>` 的容器（如 planning::Blackboard）
// 仍能 derive(Debug)。
impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("has_tools", &self.tool_executor.is_some())
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
            provider_router: ProviderRouter::new(flash, pro),
            tool_executor: None,
            memory: None,
            checkpoint_store: None,
            self_improver: None,
            hooks: None,
            config: None,
            event_sender: None,
        }
    }

    pub fn with_tools(mut self, executor: ToolExecutor) -> Self {
        self.tool_executor = Some(Arc::new(executor));
        self
    }

    pub fn with_memory(mut self, memory: MemoryManager) -> Self {
        self.memory = Some(Arc::new(memory));
        self
    }

    pub fn with_checkpoints(mut self, store: CheckpointStore) -> Self {
        self.checkpoint_store = Some(Arc::new(store));
        self
    }

    pub fn with_self_improver(mut self, improver: SelfImprover) -> Self {
        self.self_improver = Some(Arc::new(tokio::sync::Mutex::new(improver)));
        self
    }

    pub fn with_hooks(mut self, registry: HookRegistry) -> Self {
        self.hooks = Some(Arc::new(registry));
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

    pub fn self_improver(&self) -> Option<&tokio::sync::Mutex<SelfImprover>> {
        self.self_improver.as_deref()
    }

    pub fn router(&self) -> &ProviderRouter {
        &self.provider_router
    }

    pub fn memory(&self) -> Option<&MemoryManager> {
        self.memory.as_deref()
    }

    pub fn tool_executor(&self) -> Option<&ToolExecutor> {
        self.tool_executor.as_deref()
    }

    /// Single turn: user prompt → agent response (no tool loop)
    pub async fn run(
        &self,
        history: &[Message],
        context: &RunContext,
        cancel: CancellationToken,
    ) -> Result<AgentDelta, AgentError> {
        let provider = self.provider_router.select(context.complexity, context.provider_override);
        let mut inference_config = Self::config_for_complexity(context.complexity);
        if let Some(max_tokens) = context.max_tokens {
            inference_config.max_tokens = Some(max_tokens.min(393216));
        }

        // Gather tool definitions if available, optionally filtered by allowed_tools.
        let tools: Vec<ToolDef> = self
            .tool_executor
            .as_ref()
            .map(|e| {
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
            })
            .unwrap_or_default();

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

    /// Run registered hooks for a given event. Returns the action to take.
    async fn run_hooks(&self, event: HookEvent, data: serde_json::Value, run_id: RunId, iteration: usize) -> HookAction {
        let Some(ref registry) = self.hooks else { return HookAction::Continue };
        registry.run_hooks(event, data, format!("{}", run_id.0), iteration).await
    }

    /// Fire-and-forget event emission to the optional broadcast channel.
    pub async fn emit_event(&self, event: AgentEvent) {
        if let Some(ref inner) = self.event_sender {
            let sender_guard = inner.lock().await;
            if let Some(ref sender) = *sender_guard {
                let _ = sender.send(event);
            }
        }
    }

    /// Set the broadcast sender so external consumers receive events.
    /// This is intended to be called once at server startup before any run.
    pub async fn set_event_sender(&self, sender: tokio::sync::broadcast::Sender<AgentEvent>) {
        if let Some(ref inner) = self.event_sender {
            let mut guard = inner.lock().await;
            *guard = Some(sender);
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

        self.emit_event(AgentEvent::RunStarted { run_id, timestamp: chrono::Utc::now() }).await;

        for iteration in 0..max_iterations {
            // BeforeAgentLoop hook
            let _ = self.run_hooks(
                HookEvent::BeforeAgentLoop,
                serde_json::json!({ "messages": history.len(), "iteration": iteration }),
                run_id, iteration,
            ).await;

            let delta = self.run(history, context, cancel.child_token()).await?;
            let stop_reason = delta.stop_reason.clone();

            total_usage.input_tokens += delta.usage.input_tokens;
            total_usage.output_tokens += delta.usage.output_tokens;

            // AfterLlmCall hook (track token usage)
            let _ = self.run_hooks(
                HookEvent::AfterLlmCall,
                serde_json::json!({
                    "input_tokens": delta.usage.input_tokens,
                    "output_tokens": delta.usage.output_tokens,
                    "stop_reason": format!("{:?}", stop_reason),
                }),
                run_id, iteration,
            ).await;

            history.extend(delta.new_messages.clone());

            // Auto-save checkpoint if configured
            if context.checkpoint_enabled
                && let Some(ref store) = self.checkpoint_store
                    && let Some(ref project_id) = context.project_id
                        && iteration % context.checkpoint_interval.unwrap_or(5) == 0 {
                            let ckpt = Checkpoint::new(
                                run_id,
                                StepId::new(),
                                iteration,
                                history.clone(),
                            )
                            .with_project(*project_id);
                            let _ = store.save(&ckpt);
                            let _ = self.run_hooks(
                                HookEvent::OnCheckpoint,
                                serde_json::json!({ "step": iteration }),
                                run_id, iteration,
                            ).await;
                        }

            match stop_reason {
                StopReason::ToolUse => {
                    if let Some(ref executor) = self.tool_executor {
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

                        // BeforeToolCall hook: check each tool call
                        let mut blocked = false;
                        for (_, info) in &raw_tool_calls {
                            match self.run_hooks(
                                HookEvent::BeforeToolCall,
                                info.clone(),
                                run_id, iteration,
                            ).await {
                                HookAction::Block(reason) => {
                                    tracing::warn!(reason = %reason, "Hook blocked tool call");
                                    history.push(Message::tool(
                                        "hook_blocked",
                                        &format!("Operation blocked: {reason}"),
                                    ));
                                    blocked = true;
                                    break;
                                }
                                HookAction::Skip => {
                                    tracing::warn!("Hook skipped tool call");
                                    blocked = true;
                                    break;
                                }
                                _ => {}
                            }
                        }
                        if blocked { continue; }

                        let tool_calls: Vec<ToolCallRequest> = raw_tool_calls.into_iter()
                            .map(|(req, _)| req)
                            .collect();

                        // Emit tool-call-start events (fire-and-forget).
                        for tc in &tool_calls {
                            self.emit_event(AgentEvent::ToolCallRequested {
                                call_id: tc.id,
                                tool_name: tc.name.clone(),
                                input: tc.input.clone(),
                            }).await;
                        }

                        let ctx = ToolContext {
                            working_dir: context.working_dir.clone(),
                            session_id: format!("{}", run_id.0),
                        };

                        let results = executor
                            .execute_batch(&tool_calls, &ctx, cancel.child_token())
                            .await;

                        // ── Self-improvement: track tool reliability ──
                        if let Some(ref improver) = self.self_improver {
                            let mut imp = improver.lock().await;
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
                                    imp.on_tool_failure(tool_name, &output.content);
                                } else {
                                    imp.on_tool_success(tool_name, latency);
                                }
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

                        // Append tool results and run AfterToolCall hook
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
                            let _ = self.run_hooks(
                                HookEvent::AfterToolCall,
                                serde_json::json!({
                                    "tool_call_id": format!("{}", call_id.0),
                                    "output_preview": output.content.chars().take(200).collect::<String>(),
                                    "is_error": output.metadata.as_ref().map(|m| m.is_error).unwrap_or(false),
                                }),
                                run_id, iteration,
                            ).await;
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
                _ => {
                    last_delta = Some(AgentDelta {
                        new_messages: vec![],
                        stop_reason,
                        usage: total_usage.clone(),
                    });
                    break;
                }
            }
        }

        // Episode-end consolidation
        if let Some(ref mem) = self.memory {
            mem.consolidate(ConsolidationLevel::EpisodeEnd).await;
        }

        // ── Self-improvement: reflect on the completed episode ──
        if let Some(ref improver) = self.self_improver {
            let sm_delta = miniagent_self_improve::integrator::AgentDelta {
                new_messages: vec![],
                stop_reason: last_delta.as_ref()
                    .map(|d| d.stop_reason.clone())
                    .unwrap_or(StopReason::EndTurn),
                usage: total_usage.clone(),
            };
            let mut imp = improver.lock().await;
            let reflection = imp.on_step(history, &sm_delta, cancel.child_token()).await;
            tracing::debug!(
                self_score = reflection.self_score,
                error_detected = reflection.error_detected,
                "Self-improvement step reflection"
            );
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
            .select(TaskComplexity::Simple, None);

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
        let dir = match std::path::PathBuf::from("./miniagent_context").canonicalize() {
            Ok(d) => d,
            Err(_) => {
                let d = std::path::PathBuf::from("./miniagent_context");
                if std::fs::create_dir_all(&d).is_err() {
                    return;
                }
                d
            }
        };

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
