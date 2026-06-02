use std::time::Duration;

use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::event::{ContentBlock, StopReason, Usage};
use miniagent_core::message::MessageRole;
use miniagent_core::types::ToolCallId;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamChunk, StreamResponse};

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const FLASH_MODEL: &str = "deepseek-chat";
const PRO_MODEL: &str = "deepseek-reasoner";

// ── DeepSeek API types ──────────────────────────────────────────
// OpenAI-compatible format with DeepSeek extensions

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    stream: bool,
    // DeepSeek-specific: reasoning_effort for thinking control
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: ChatContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ChatContent {
    Text(String),
    MultiPart(Vec<ContentPart>),
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart {
    Text { text: String },
    #[allow(dead_code)]
    ImageUrl {
        image_url: ImageUrl,
    },
}

#[derive(Debug, Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Debug, Serialize)]
struct Tool {
    #[serde(rename = "type")]
    tool_type: String,
    function: FunctionDef,
}

#[derive(Debug, Serialize)]
struct FunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: FunctionCall,
}

#[derive(Debug, Serialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ChatResponse {
    id: String,
    choices: Vec<Choice>,
    usage: Option<UsageResponse>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Choice {
    index: usize,
    message: ChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    role: Option<String>,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallResponse>,
    // DeepSeek reasoning content (Pro model)
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ToolCallResponse {
    id: String,
    #[serde(rename = "type", default)]
    call_type: Option<String>,
    function: FunctionCallResponse,
}

#[derive(Debug, Deserialize)]
struct FunctionCallResponse {
    name: String,
    arguments: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct UsageResponse {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<usize>,
    #[serde(default)]
    prompt_cache_miss_tokens: Option<usize>,
}

// ── Streaming types ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct StreamChunkRaw {
    choices: Vec<StreamChoice>,
    usage: Option<UsageResponse>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct StreamChoice {
    index: usize,
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCall>,
}

#[derive(Debug, Deserialize)]
struct StreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<StreamFunction>,
}

#[derive(Debug, Deserialize)]
struct StreamFunction {
    name: Option<String>,
    arguments: Option<String>,
}

// ── DeepSeek Client ─────────────────────────────────────────

#[derive(Clone)]
pub struct DeepSeekClient {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    is_reasoner: bool,
    thinking_budget: u32,
}

impl DeepSeekClient {
    pub fn new(api_key: impl Into<String>, model: &str, is_reasoner: bool) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("failed to create HTTP client"),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
            model: model.to_string(),
            is_reasoner,
            thinking_budget: 8000,
        }
    }

    pub fn with_thinking_budget(mut self, tokens: u32) -> Self {
        self.thinking_budget = tokens;
        self
    }

    fn build_request(&self, request: &CompletionRequest, stream: bool) -> ChatRequest {
        let messages: Vec<ChatMessage> = request
            .messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                };
                let content = ChatContent::Text(msg.text_content());

                let (tool_calls, tool_call_id, name) = match msg.role {
                    MessageRole::Assistant => {
                        let calls: Vec<ToolCall> = msg
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                                    id: format!("{}", id.0),
                                    call_type: "function".into(),
                                    function: FunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input).unwrap_or_default(),
                                    },
                                }),
                                _ => None,
                            })
                            .collect();
                        let tc = if calls.is_empty() { None } else { Some(calls) };
                        (tc, None, None)
                    }
                    MessageRole::Tool => {
                        // For tool result messages, we include the tool_call_id
                        // Extract from the text content which has format "[toolu_vrtx_ID] result"
                        let text = msg.text_content();
                        let tid = text
                            .strip_prefix("[toolu_vrtx_")
                            .and_then(|s| s.split(']').next())
                            .map(|s| s.to_string());
                        (None, tid, None)
                    }
                    _ => (None, None, None),
                };

                ChatMessage {
                    role: role.to_string(),
                    content,
                    tool_calls,
                    tool_call_id,
                    name,
                }
            })
            .collect();

        let tools: Vec<Tool> = request
            .tools
            .iter()
            .map(|t| Tool {
                tool_type: "function".into(),
                function: FunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();

        let reasoning_effort = if self.is_reasoner && request.config.enable_thinking {
            let budget = request
                .config
                .thinking_budget
                .unwrap_or(self.thinking_budget);
            let effort = if budget <= 4000 {
                "low"
            } else if budget <= 12000 {
                "medium"
            } else {
                "high"
            };
            Some(effort.to_string())
        } else {
            None
        };

        ChatRequest {
            model: self.model.clone(),
            messages,
            temperature: request.config.temperature,
            max_tokens: request.config.max_tokens,
            top_p: request.config.top_p,
            tools,
            tool_choice: None,
            stream,
            reasoning_effort,
        }
    }

    fn parse_response(&self, response: ChatResponse) -> CompletionResponse {
        let choice = response.choices.into_iter().next();
        let mut content = Vec::new();

        if let Some(ref choice) = choice {
            if let Some(ref reasoning) = choice.message.reasoning_content
                && !reasoning.is_empty() {
                    content.push(ContentBlock::Thinking {
                        thinking: reasoning.clone(),
                        signature: None,
                    });
                }

            if let Some(ref text) = choice.message.content
                && !text.is_empty() {
                    content.push(ContentBlock::Text {
                        text: text.clone(),
                    });
                }

            for tc in &choice.message.tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                content.push(ContentBlock::ToolUse {
                    id: ToolCallId(uuid::Uuid::new_v4()),
                    name: tc.function.name.clone(),
                    input,
                });
            }
        }

        let stop_reason = match choice.and_then(|c| c.finish_reason) {
            Some(s) if s == "stop" => StopReason::EndTurn,
            Some(s) if s == "length" => StopReason::MaxTokens,
            Some(s) if s == "tool_calls" => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };

        let usage = response.usage.map_or(
            Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            |u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                cache_creation_input_tokens: u.prompt_cache_miss_tokens,
                cache_read_input_tokens: u.prompt_cache_hit_tokens,
            },
        );

        CompletionResponse {
            content,
            usage,
            stop_reason,
        }
    }

    fn system_message(request: &CompletionRequest) -> ChatMessage {
        ChatMessage {
            role: "system".into(),
            content: ChatContent::Text(request.system.clone()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

}

#[async_trait]
impl LlmProvider for DeepSeekClient {
    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        let chat_request = {
            let mut req = self.build_request(request, false);
            // Prepend system message
            let mut all_messages = vec![Self::system_message(request)];
            all_messages.append(&mut req.messages);
            req.messages = all_messages;
            req
        };

        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&chat_request)
                .send() => r,
        }
        .map_err(|e| AgentError::provider(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::provider(format!(
                "API error {status}: {body}"
            )));
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| AgentError::provider(format!("Failed to parse response: {e}")))?;

        Ok(self.parse_response(chat_response))
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError> {
        let chat_request = {
            let mut req = self.build_request(request, true);
            let mut all_messages = vec![Self::system_message(request)];
            all_messages.append(&mut req.messages);
            req.messages = all_messages;
            req
        };

        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let client = self.client.clone();
        let api_key = self.api_key.clone();

        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tx.send(Err(AgentError::Cancelled)).await;
                    return;
                }
                r = client
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .json(&chat_request)
                    .send() => r,
            };

            let response = match result {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(Err(AgentError::provider(format!("HTTP error: {e}"))))
                        .await;
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(AgentError::provider(format!(
                        "API error {status}: {body}"
                    ))))
                    .await;
                return;
            }

            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            // Track tool call accumulation state
            let mut pending_tool_calls: Vec<(usize, Option<String>, Option<String>, String)> = Vec::new();

            use futures_util::StreamExt;
            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(Err(AgentError::provider(format!("Stream error: {e}"))))
                            .await;
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer.drain(..=line_end);

                    let data = line.strip_prefix("data: ").unwrap_or(&line);
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }

                    let parsed: StreamChunkRaw = match serde_json::from_str(data) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };

                    // Emit usage if present
                    if let Some(ref u) = parsed.usage {
                        let _ = tx
                            .send(Ok(StreamChunk::Usage(Usage {
                                input_tokens: u.prompt_tokens,
                                output_tokens: u.completion_tokens,
                                cache_creation_input_tokens: u.prompt_cache_miss_tokens,
                                cache_read_input_tokens: u.prompt_cache_hit_tokens,
                            })))
                            .await;
                    }

                    for choice in parsed.choices {
                        // Handle finish reason
                        if let Some(ref fr) = choice.finish_reason {
                            let reason = match fr.as_str() {
                                "stop" => StopReason::EndTurn,
                                "length" => StopReason::MaxTokens,
                                "tool_calls" => StopReason::ToolUse,
                                _ => StopReason::EndTurn,
                            };
                            let _ = tx.send(Ok(StreamChunk::Stop(reason))).await;
                            continue;
                        }

                        let delta = choice.delta;

                        // Reasoning content (Pro model thinking)
                        if let Some(ref reasoning) = delta.reasoning_content
                            && !reasoning.is_empty() {
                                let _ = tx
                                    .send(Ok(StreamChunk::TextDelta {
                                        text: format!("<thinking>{reasoning}</thinking>"),
                                    }))
                                    .await;
                            }

                        // Regular text content
                        if let Some(ref text) = delta.content
                            && !text.is_empty() {
                                let _ = tx
                                    .send(Ok(StreamChunk::TextDelta {
                                        text: text.clone(),
                                    }))
                                    .await;
                            }

                        // Tool calls (streamed incrementally)
                        for tc in &delta.tool_calls {
                            let idx = tc.index;
                            let id = tc.id.clone();
                            let name = tc
                                .function
                                .as_ref()
                                .and_then(|f| f.name.clone());
                            let args = tc
                                .function
                                .as_ref()
                                .and_then(|f| f.arguments.clone())
                                .unwrap_or_default();

                            // Accumulate tool calls
                            if let Some(existing) = pending_tool_calls.iter_mut().find(|(i, _, _, _)| *i == idx) {
                                if let Some(a) = existing.2.as_mut() { a.push_str(&args) }
                                if id.is_some() {
                                    existing.1 = id;
                                }
                                if name.is_some() {
                                    existing.3 = name.unwrap_or_default();
                                }
                            } else {
                                pending_tool_calls.push((idx, id, Some(args), name.unwrap_or_default()));
                            }
                        }
                    }
                }
            }

            // Emit any completed tool calls
            for (_idx, id, args, name) in &pending_tool_calls {
                if let (Some(_id), Some(args)) = (id, args) {
                    let input: serde_json::Value =
                        serde_json::from_str(args).unwrap_or_default();
                    let _ = tx
                        .send(Ok(StreamChunk::ContentBlockStart {
                            block: ContentBlock::ToolUse {
                                id: ToolCallId(uuid::Uuid::new_v4()),
                                name: name.clone(),
                                input,
                            },
                        }))
                        .await;
                }
            }
        });

        Ok(StreamResponse {
            content_receiver: rx,
        })
    }
}

// ── Public convenience types ─────────────────────────────────

pub struct DeepSeekFlash;

impl DeepSeekFlash {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(api_key: impl Into<String>) -> DeepSeekClient {
        DeepSeekClient::new(api_key, FLASH_MODEL, false)
    }
}

pub struct DeepSeekPro;

impl DeepSeekPro {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(api_key: impl Into<String>) -> DeepSeekClient {
        DeepSeekClient::new(api_key, PRO_MODEL, true)
    }

    pub fn with_thinking(api_key: impl Into<String>, thinking_budget: u32) -> DeepSeekClient {
        DeepSeekClient::new(api_key, PRO_MODEL, true).with_thinking_budget(thinking_budget)
    }
}
