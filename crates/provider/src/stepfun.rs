use std::error::Error;
use std::time::Duration;

use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::event::{ContentBlock, StopReason, Usage};
use miniagent_core::message::MessageRole;
use miniagent_core::secrets::ApiKey;
use miniagent_core::types::ToolCallId;
use reqwest::{Client, Proxy};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamChunk, StreamResponse};

// ── StepFun Configuration ──────────────────────────────────────

const DEFAULT_BASE_URL: &str = "https://api.stepfun.com/step_plan/v1";
const FLASH_MODEL: &str = "step-3.7-flash";

// ── StepFun API Types (OpenAI-compatible) ──────────────────────

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
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
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
#[allow(dead_code)]
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

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatResponse {
    id: String,
    choices: Vec<Choice>,
    usage: Option<UsageResponse>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Choice {
    index: usize,
    message: ChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChoiceMessage {
    role: Option<String>,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallResponse>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UsageResponse {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

// ── Streaming types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct StreamChunkRaw {
    choices: Vec<StreamChoice>,
    usage: Option<UsageResponse>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamChoice {
    index: usize,
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
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

// ── StepFun Client ─────────────────────────────────────────────

#[derive(Clone)]
pub struct StepFunClient {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl StepFunClient {
    pub fn new(api_key: &ApiKey) -> Self {
        Self::with_model(api_key, FLASH_MODEL)
    }

    pub fn with_model(api_key: &ApiKey, model: &str) -> Self {
        let mut builder = Client::builder().timeout(Duration::from_secs(300));
        if let Some(proxy_url) = Self::proxy_from_env()
            && let Ok(proxy) = Proxy::all(&proxy_url) {
                builder = builder.proxy(proxy);
            }
        Self {
            client: builder
                .build()
                .expect("failed to create HTTP client"),
            base_url: std::env::var("STEPFUN_BASE_URL")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key: api_key.as_str().to_string(),
            model: std::env::var("STEPFUN_MODEL_NAME")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| model.to_string()),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Build a proxy from standard environment variables.
    /// Checks `ALL_PROXY` / `all_proxy` → `HTTPS_PROXY` / `https_proxy` → `HTTP_PROXY` / `http_proxy`.
    fn proxy_from_env() -> Option<String> {
        std::env::var("ALL_PROXY")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var("all_proxy").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("HTTPS_PROXY").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("https_proxy").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("HTTP_PROXY").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("http_proxy").ok().filter(|v| !v.is_empty()))
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
                let content = msg.text_content();

                let (tool_calls, tool_call_id) = match msg.role {
                    MessageRole::Assistant => {
                        let calls: Vec<ToolCall> = msg
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, name, input } => {
                                    Some(ToolCall {
                                        id: format!("{}", id.0),
                                        call_type: "function".into(),
                                        function: FunctionCall {
                                            name: name.clone(),
                                            arguments: serde_json::to_string(input).unwrap_or_default(),
                                        },
                                    })
                                }
                                _ => None,
                            })
                            .collect();
                        let tc = if calls.is_empty() { None } else { Some(calls) };
                        (tc, None)
                    }
                    MessageRole::Tool => {
                        let text = msg.text_content();
                        let tid = text
                            .strip_prefix("[toolu_vrtx_")
                            .and_then(|s| s.split(']').next())
                            .map(|s| s.to_string());
                        (None, tid)
                    }
                    _ => (None, None),
                };

                ChatMessage {
                    role: role.to_string(),
                    content,
                    tool_calls,
                    tool_call_id,
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

        ChatRequest {
            model: self.model.clone(),
            messages,
            temperature: request.config.temperature,
            max_tokens: request.config.max_tokens,
            top_p: request.config.top_p,
            tools,
            tool_choice: None,
            stream,
        }
    }

    fn parse_response(&self, response: ChatResponse) -> CompletionResponse {
        let choice = response.choices.into_iter().next();
        let mut content = Vec::new();

        if let Some(ref choice) = choice {
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
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        );

        CompletionResponse {
            content,
            usage,
            stop_reason,
        }
    }

    fn api_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }

    fn system_message(request: &CompletionRequest) -> ChatMessage {
        ChatMessage {
            role: "system".into(),
            content: request.system.clone(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

// ── LlmProvider Implementation ─────────────────────────────────

#[async_trait]
impl LlmProvider for StepFunClient {
    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        let chat_request = {
            let mut req = self.build_request(request, false);
            let mut all_messages = vec![Self::system_message(request)];
            all_messages.append(&mut req.messages);
            req.messages = all_messages;
            req
        };

        let url = self.api_url();

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&chat_request)
                .send() => r,
        }
        .map_err(|e| {
            let cause = e
                .source()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            if let Some(c) = cause {
                AgentError::provider(format!("HTTP request failed: {e} (cause: {c})"))
            } else {
                AgentError::provider(format!("HTTP request failed: {e}"))
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::provider(format!(
                "StepFun API error {status}: {body}"
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

        let url = self.api_url();

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
                    let cause = e
                        .source()
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty());
                    let detail = if let Some(c) = cause {
                        format!("HTTP error: {e} (cause: {c})")
                    } else {
                        format!("HTTP error: {e}")
                    };
                    let _ = tx.send(Err(AgentError::provider(detail))).await;
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(AgentError::provider(format!(
                        "StepFun API error {status}: {body}"
                    ))))
                    .await;
                return;
            }

            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
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

                    if let Some(ref u) = parsed.usage {
                        let _ = tx
                            .send(Ok(StreamChunk::Usage(Usage {
                                input_tokens: u.prompt_tokens,
                                output_tokens: u.completion_tokens,
                                cache_creation_input_tokens: None,
                                cache_read_input_tokens: None,
                            })))
                            .await;
                    }

                    for choice in parsed.choices {
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

                        if let Some(ref text) = delta.content
                            && !text.is_empty() {
                                let _ = tx
                                    .send(Ok(StreamChunk::TextDelta {
                                        text: text.clone(),
                                    }))
                                    .await;
                            }

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

// ── Public convenience types ───────────────────────────────────

pub struct StepFunFlash;

impl StepFunFlash {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(api_key: &ApiKey) -> StepFunClient {
        StepFunClient::new(api_key)
    }
}
