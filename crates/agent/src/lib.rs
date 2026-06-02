pub mod context;

use std::sync::Arc;

use miniagent_checkpoint::CheckpointStore;
use miniagent_core::checkpoint::Checkpoint;
use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::error::AgentError;
use miniagent_core::event::{ContentBlock, StopReason, Usage};
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

pub use context::RunContext;

/// Max estimated tokens before we trim history (leaves room for output in 128K window)
const MAX_HISTORY_TOKENS: usize = 96_000;
/// Max chars from discarded messages to feed into the summariser
const SUMMARIZE_CHAR_LIMIT: usize = 12_000;

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

        // Gather tool definitions if available
        let tools: Vec<ToolDef> = self
            .tool_executor
            .as_ref()
            .map(|e| {
                e.registry()
                    .get_definitions()
                    .into_iter()
                    .map(|d| ToolDef {
                        name: d.name,
                        description: d.description,
                        parameters: d.parameters,
                    })
                    .collect()
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

        for iteration in 0..max_iterations {
            let delta = self.run(history, context, cancel.child_token()).await?;
            let stop_reason = delta.stop_reason.clone();

            total_usage.input_tokens += delta.usage.input_tokens;
            total_usage.output_tokens += delta.usage.output_tokens;
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
                        }

            match stop_reason {
                StopReason::ToolUse => {
                    // Execute tools and continue
                    if let Some(ref executor) = self.tool_executor {
                        let last_msg = history.last().unwrap();
                        let tool_calls: Vec<ToolCallRequest> = last_msg
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, name, input } => Some(ToolCallRequest {
                                    id: *id,
                                    name: name.clone(),
                                    input: input.clone(),
                                }),
                                _ => None,
                            })
                            .collect();

                        if tool_calls.is_empty() {
                            last_delta = Some(AgentDelta {
                                new_messages: vec![],
                                stop_reason,
                                usage: total_usage.clone(),
                            });
                            break;
                        }

                        let ctx = ToolContext {
                            working_dir: context.working_dir.clone(),
                            session_id: format!("{}", run_id.0),
                        };

                        let results = executor
                            .execute_batch(&tool_calls, &ctx, cancel.child_token())
                            .await;

                        // Track errors: break if all tool calls failed
                        let error_count = results.iter()
                            .filter(|(_, o)| o.content.starts_with("Error:"))
                            .count();
                        if error_count == results.len() && !results.is_empty() {
                            consecutive_errors += 1;
                        } else {
                            consecutive_errors = 0;
                        }

                        // Append tool results as messages
                        for (call_id, output) in results {
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
                        if consecutive_errors >= 3 {
                            last_delta = Some(AgentDelta {
                                new_messages: vec![],
                                stop_reason: StopReason::EndTurn,
                                usage: total_usage.clone(),
                            });
                            break;
                        }
                        // Continue the loop
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

        Ok(last_delta.unwrap_or(AgentDelta {
            new_messages: vec![],
            stop_reason: StopReason::EndTurn,
            usage: total_usage.clone(),
        }))
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
        if estimate_history_tokens(history) <= MAX_HISTORY_TOKENS {
            return;
        }

        let keep_recent = 5usize.min(history.len().saturating_sub(1));
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
