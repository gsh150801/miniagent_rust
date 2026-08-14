//! MiniMax provider supporting both API protocols:
//!
//! - **OpenAI-compatible** Chat Completions: `https://api.minimaxi.com/v1`
//! - **Anthropic-compatible** Messages: `https://api.minimaxi.com/anthropic`
//!
//! The protocol is auto-detected from the base URL (contains "anthropic"
//! → Anthropic protocol). Subscription keys are issued from the
//! Token Plan console at platform.minimaxi.com.
//!
//! Env overrides: `MINIMAX_API_KEY`, `MINIMAX_BASE_URL`, `MINIMAX_MODEL_NAME`.

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

const DEFAULT_BASE_URL: &str = "https://api.minimaxi.com/v1";
const DEFAULT_MODEL: &str = "MiniMax-M3";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Detects protocol from base URL: "anthropic" in URL → Anthropic, else OpenAI.
fn is_anthropic_mode(base_url: &str) -> bool {
    base_url.to_lowercase().contains("anthropic")
}

// ═══════════════════════════════════════════════════════════════
//  OpenAI-compatible wire types
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OaiMessage {
    role: String,
    content: OaiContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OaiContent {
    Text(String),
    MultiPart(Vec<OaiContentPart>),
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OaiContentPart {
    Text { text: String },
}

#[derive(Debug, Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiFunctionDef,
}

#[derive(Debug, Serialize)]
struct OaiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct OaiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OaiFunctionCall,
}

#[derive(Debug, Serialize)]
struct OaiFunctionCall {
    name: String,
    arguments: String,
}

// ── OpenAI response types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OaiResponse {
    #[allow(dead_code)]
    id: String,
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Debug, Deserialize)]
struct OaiChoice {
    #[allow(dead_code)]
    index: usize,
    message: OaiChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiChoiceMessage {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OaiToolCallResp>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiToolCallResp {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    #[serde(rename = "type", default)]
    call_type: Option<String>,
    function: OaiFuncResp,
}

#[derive(Debug, Deserialize)]
struct OaiFuncResp {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OaiUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    #[allow(dead_code)]
    total_tokens: usize,
}

// ── OpenAI streaming types ───────────────────────────────────

#[derive(Debug, Deserialize)]
struct OaiStreamChunk {
    choices: Vec<OaiStreamChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Debug, Deserialize)]
struct OaiStreamChoice {
    #[allow(dead_code)]
    index: usize,
    delta: OaiStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiStreamDelta {
    #[allow(dead_code)]
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OaiStreamToolCall>,
}

#[derive(Debug, Deserialize)]
struct OaiStreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<OaiStreamFunc>,
}

#[derive(Debug, Deserialize)]
struct OaiStreamFunc {
    name: Option<String>,
    arguments: Option<String>,
}

// ═══════════════════════════════════════════════════════════════
//  Anthropic-compatible wire types
// ═══════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct AnthRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    system: String,
    messages: Vec<AnthMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthTool>,
    stream: bool,
}

/// Anthropic message `content` field: either a plain string or a block array.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthContent {
    Text(String),
    Blocks(Vec<serde_json::Value>),
}

#[derive(Debug, Serialize)]
struct AnthMessage {
    role: String,
    content: AnthContent,
}

impl AnthMessage {
    fn text(role: &str, text: impl Into<String>) -> Self {
        Self { role: role.to_string(), content: AnthContent::Text(text.into()) }
    }
    fn blocks(role: &str, blocks: Vec<serde_json::Value>) -> Self {
        Self { role: role.to_string(), content: AnthContent::Blocks(blocks) }
    }
}

#[derive(Serialize)]
struct AnthTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
struct AnthResponse {
    #[serde(default)]
    content: Vec<AnthContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthUsage>,
}

#[derive(Debug, serde::Deserialize)]
struct AnthContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
struct AnthUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

// ═══════════════════════════════════════════════════════════════
//  Client
// ═══════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct MiniMaxClient {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    anthropic: bool,
}

impl MiniMaxClient {
    pub fn new(api_key: &ApiKey) -> Self {
        Self::with_model(api_key, DEFAULT_MODEL)
    }

    pub fn with_model(api_key: &ApiKey, model: &str) -> Self {
        let mut builder = Client::builder().timeout(Duration::from_secs(300));
        if let Some(proxy_url) = Self::proxy_from_env()
            && let Ok(proxy) = Proxy::all(&proxy_url)
        {
            builder = builder.proxy(proxy);
        }
        let base_url = std::env::var("MINIMAX_BASE_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let anthropic = is_anthropic_mode(&base_url);
        Self {
            client: builder.build().expect("failed to create HTTP client"),
            base_url,
            api_key: api_key.as_str().to_string(),
            model: std::env::var("MINIMAX_MODEL_NAME")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| model.to_string()),
            anthropic,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self.anthropic = is_anthropic_mode(&self.base_url);
        self
    }

    /// Force-override the model name, bypassing any env-var override.
    /// Used by the model-profile registry so explicit selections always win.
    pub fn with_model_name(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    fn proxy_from_env() -> Option<String> {
        std::env::var("ALL_PROXY").ok().filter(|v| !v.is_empty())
            .or_else(|| std::env::var("all_proxy").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("HTTPS_PROXY").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("https_proxy").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("HTTP_PROXY").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("http_proxy").ok().filter(|v| !v.is_empty()))
    }

    // ── URL construction ─────────────────────────────────────

    fn api_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if self.anthropic {
            format!("{base}/v1/messages")
        } else if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }

    // ── Build OpenAI request ─────────────────────────────────

    fn build_oai_request(&self, request: &CompletionRequest, stream: bool) -> OaiRequest {
        let messages: Vec<OaiMessage> = request.messages.iter().map(|msg| {
            let role = match msg.role {
                MessageRole::System => "system",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::Tool => "tool",
            };
            let content = OaiContent::Text(msg.text_content());
            let (tool_calls, tool_call_id, name) = match msg.role {
                MessageRole::Assistant => {
                    let calls: Vec<OaiToolCall> = msg.content.iter().filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => Some(OaiToolCall {
                            id: format!("{}", id.0),
                            call_type: "function".into(),
                            function: OaiFunctionCall {
                                name: name.clone(),
                                arguments: serde_json::to_string(input).unwrap_or_default(),
                            },
                        }),
                        _ => None,
                    }).collect();
                    (if calls.is_empty() { None } else { Some(calls) }, None, None)
                }
                MessageRole::Tool => {
                    let text = msg.text_content();
                    let tid = text.strip_prefix("[toolu_vrtx_").and_then(|s| s.split(']').next()).map(|s| s.to_string());
                    (None, tid, None)
                }
                _ => (None, None, None),
            };
            OaiMessage { role: role.to_string(), content, tool_calls, tool_call_id, name }
        }).collect();

        let tools: Vec<OaiTool> = request.tools.iter().map(|t| OaiTool {
            tool_type: "function".into(),
            function: OaiFunctionDef {
                name: t.name.clone(), description: t.description.clone(), parameters: t.parameters.clone(),
            },
        }).collect();

        OaiRequest {
            model: self.model.clone(), messages, temperature: request.config.temperature,
            max_tokens: request.config.max_tokens, top_p: request.config.top_p,
            tools, tool_choice: None, stream,
        }
    }

    fn oai_system_message(request: &CompletionRequest) -> OaiMessage {
        OaiMessage {
            role: "system".into(),
            content: OaiContent::Text(request.system.clone()),
            tool_calls: None, tool_call_id: None, name: None,
        }
    }

    // ── Build Anthropic request ──────────────────────────────

    fn build_anth_request(&self, request: &CompletionRequest, stream: bool) -> AnthRequest {
        let mut system = request.system.clone();
        let mut messages: Vec<AnthMessage> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    let text = msg.text_content();
                    if !system.is_empty() && !text.is_empty() { system.push('\n'); }
                    system.push_str(&text);
                }
                MessageRole::User => {
                    messages.push(AnthMessage::text("user", msg.text_content()));
                }
                MessageRole::Assistant => {
                    let mut blocks = Vec::new();
                    for b in &msg.content {
                        match b {
                            ContentBlock::Text { text } if !text.is_empty() => {
                                blocks.push(serde_json::json!({"type": "text", "text": text}));
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                blocks.push(serde_json::json!({
                                    "type": "tool_use", "id": format!("{}", id.0),
                                    "name": name, "input": input,
                                }));
                            }
                            _ => {}
                        }
                    }
                    if blocks.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": " "}));
                    }
                    messages.push(AnthMessage::blocks("assistant", blocks));
                }
                MessageRole::Tool => {
                    let text = msg.text_content();
                    let (tool_use_id, content) = text
                        .strip_prefix("[toolu_vrtx_")
                        .and_then(|s| s.split_once(']'))
                        .map(|(id, rest)| (id.to_string(), rest.trim_start().to_string()))
                        .unwrap_or_else(|| (uuid::Uuid::new_v4().to_string(), text.clone()));
                    messages.push(AnthMessage::blocks("user", vec![serde_json::json!({
                        "type": "tool_result", "tool_use_id": tool_use_id, "content": content,
                    })]));
                }
            }
        }

        let tools = request.tools.iter().map(|t| AnthTool {
            name: t.name.clone(), description: t.description.clone(),
            input_schema: t.parameters.clone(),
        }).collect();

        AnthRequest {
            model: self.model.clone(),
            max_tokens: request.config.max_tokens.unwrap_or(8192),
            temperature: request.config.temperature, top_p: request.config.top_p,
            system, messages, tools, stream,
        }
    }

    // ── Parse responses ──────────────────────────────────────

    fn parse_oai_response(&self, response: OaiResponse) -> CompletionResponse {
        let choice = response.choices.into_iter().next();
        let mut content = Vec::new();
        let stop_reason = choice.as_ref().and_then(|c| c.finish_reason.clone());
        if let Some(choice) = choice {
            if let Some(ref reasoning) = choice.message.reasoning {
                if !reasoning.is_empty() {
                    content.push(ContentBlock::Thinking { thinking: reasoning.clone(), signature: None });
                }
            }
            if let Some(ref text) = choice.message.content {
                if !text.is_empty() {
                    content.push(ContentBlock::Text { text: text.clone() });
                }
            }
            for tc in &choice.message.tool_calls {
                let input: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                content.push(ContentBlock::ToolUse {
                    id: ToolCallId(uuid::Uuid::new_v4()),
                    name: tc.function.name.clone(), input,
                });
            }
        }
        let stop_reason = match stop_reason.as_deref() {
            Some("stop") => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some("tool_calls") => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        let usage = response.usage.map_or(Usage {
            input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None,
        }, |u| Usage {
            input_tokens: u.prompt_tokens, output_tokens: u.completion_tokens,
            cache_creation_input_tokens: None, cache_read_input_tokens: None,
        });
        CompletionResponse { content, usage, stop_reason }
    }

    fn parse_anth_response(&self, response: AnthResponse) -> CompletionResponse {
        let mut content = Vec::new();
        for block in response.content {
            match block.block_type.as_str() {
                "text" => {
                    if let Some(text) = block.text.filter(|t| !t.is_empty()) {
                        content.push(ContentBlock::Text { text });
                    }
                }
                "tool_use" => {
                    let id = block.id.and_then(|s| s.parse::<uuid::Uuid>().ok()).unwrap_or_else(uuid::Uuid::new_v4);
                    content.push(ContentBlock::ToolUse {
                        id: ToolCallId(id), name: block.name.unwrap_or_default(),
                        input: block.input.unwrap_or_default(),
                    });
                }
                _ => {}
            }
        }
        let stop_reason = match response.stop_reason.as_deref() {
            Some("end_turn") => StopReason::EndTurn,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("tool_use") => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        let usage = response.usage.map_or(Usage {
            input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: None, cache_read_input_tokens: None,
        }, |u| Usage {
            input_tokens: u.input_tokens as usize, output_tokens: u.output_tokens as usize,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: if u.cache_read_input_tokens > 0 { Some(u.cache_read_input_tokens as usize) } else { None },
        });
        CompletionResponse { content, usage, stop_reason }
    }

    fn parse_stop_reason(reason: &str) -> StopReason {
        match reason {
            "stop" | "end_turn" => StopReason::EndTurn,
            "length" | "max_tokens" => StopReason::MaxTokens,
            "tool_calls" | "tool_use" => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        }
    }

    fn build_http_error(e: reqwest::Error) -> AgentError {
        let cause = e.source().map(|s| s.to_string()).filter(|s| !s.is_empty());
        match cause {
            Some(c) => AgentError::provider(format!("HTTP request failed: {e} (cause: {c})")),
            None => AgentError::provider(format!("HTTP request failed: {e}")),
        }
    }

    fn format_api_error(status: reqwest::StatusCode, body: String) -> AgentError {
        AgentError::provider(format!("MiniMax API error {status}: {body}"))
    }
}

// ═══════════════════════════════════════════════════════════════
//  LlmProvider Implementation
// ═══════════════════════════════════════════════════════════════

#[async_trait]
impl LlmProvider for MiniMaxClient {
    async fn complete(
        &self, request: &CompletionRequest, cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        let url = self.api_url();

        let response = if self.anthropic {
            let body = self.build_anth_request(request, false);
            tokio::select! {
                _ = cancel.cancelled() => return Err(AgentError::Cancelled),
                r = self.client.post(&url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(&body).send() => r,
            }.map_err(Self::build_http_error)?
        } else {
            let mut chat_req = self.build_oai_request(request, false);
            let mut all_msgs = vec![Self::oai_system_message(request)];
            all_msgs.append(&mut chat_req.messages);
            chat_req.messages = all_msgs;
            tokio::select! {
                _ = cancel.cancelled() => return Err(AgentError::Cancelled),
                r = self.client.post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&chat_req).send() => r,
            }.map_err(Self::build_http_error)?
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Self::format_api_error(status, body));
        }

        if self.anthropic {
            let parsed: AnthResponse = response.json().await
                .map_err(|e| AgentError::provider(format!("Failed to parse response: {e}")))?;
            Ok(self.parse_anth_response(parsed))
        } else {
            let parsed: OaiResponse = response.json().await
                .map_err(|e| AgentError::provider(format!("Failed to parse response: {e}")))?;
            Ok(self.parse_oai_response(parsed))
        }
    }

    async fn stream(
        &self, request: &CompletionRequest, cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError> {
        let url = self.api_url();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let anthropic = self.anthropic;

        let (body_json, headers_mode) = if anthropic {
            let body = self.build_anth_request(request, true);
            (serde_json::to_value(&body).unwrap_or_default(), "anthropic")
        } else {
            let mut chat_req = self.build_oai_request(request, true);
            let mut all_msgs = vec![Self::oai_system_message(request)];
            all_msgs.append(&mut chat_req.messages);
            chat_req.messages = all_msgs;
            (serde_json::to_value(&chat_req).unwrap_or_default(), "openai")
        };

        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tx.send(Err(AgentError::Cancelled)).await;
                    return;
                }
                r = {
                    let mut req = client.post(&url);
                    if headers_mode == "anthropic" {
                        req = req.header("x-api-key", &api_key).header("anthropic-version", ANTHROPIC_VERSION);
                    } else {
                        req = req.header("Authorization", format!("Bearer {api_key}"));
                    }
                    req.json(&body_json).send()
                } => r,
            };

            let response = match result {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(AgentError::provider(format!("HTTP error: {e}")))).await;
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let _ = tx.send(Err(AgentError::provider(format!("MiniMax API error {status}: {body}")))).await;
                return;
            }

            use futures_util::StreamExt;
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();

            if anthropic {
                // ── Anthropic SSE: content_block_start/delta/message_delta ──
                let mut pending_tools: Vec<(usize, String, String, String)> = Vec::new();
                let mut emitted_usage = false;

                while let Some(chunk_result) = stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => { let _ = tx.send(Err(AgentError::provider(format!("Stream error: {e}")))).await; return; }
                    };
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer.drain(..=line_end);
                        let Some(data) = line.strip_prefix("data: ") else { continue; };
                        if data == "[DONE]" || data.is_empty() { continue; }
                        let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else { continue; };

                        match event["type"].as_str().unwrap_or_default() {
                            "message_start" => {
                                let u = &event["message"]["usage"];
                                let inp = u["input_tokens"].as_u64().unwrap_or(0);
                                let out = u["output_tokens"].as_u64().unwrap_or(0);
                                let cache = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                                if inp > 0 || out > 0 {
                                    emitted_usage = true;
                                    let _ = tx.send(Ok(StreamChunk::Usage(Usage {
                                        input_tokens: inp as usize, output_tokens: out as usize,
                                        cache_creation_input_tokens: None,
                                        cache_read_input_tokens: if cache > 0 { Some(cache as usize) } else { None },
                                    }))).await;
                                }
                            }
                            "content_block_start" => {
                                let index = event["index"].as_u64().unwrap_or(0) as usize;
                                let block = &event["content_block"];
                                if block["type"].as_str() == Some("tool_use") {
                                    pending_tools.push((
                                        index,
                                        block["id"].as_str().unwrap_or_default().to_string(),
                                        block["name"].as_str().unwrap_or_default().to_string(),
                                        String::new(),
                                    ));
                                }
                            }
                            "content_block_delta" => {
                                let delta = &event["delta"];
                                match delta["type"].as_str().unwrap_or_default() {
                                    "text_delta" => {
                                        if let Some(text) = delta["text"].as_str().filter(|t| !t.is_empty()) {
                                            let _ = tx.send(Ok(StreamChunk::TextDelta { text: text.to_string() })).await;
                                        }
                                    }
                                    "input_json_delta" => {
                                        if let Some(index) = event["index"].as_u64() {
                                            if let Some(pt) = pending_tools.iter_mut().find(|(i, _, _, _)| *i == index as usize) {
                                                pt.3.push_str(delta["partial_json"].as_str().unwrap_or_default());
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "message_delta" => {
                                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                                    let _ = tx.send(Ok(StreamChunk::Stop(MiniMaxClient::parse_stop_reason(reason)))).await;
                                }
                                let u = &event["usage"];
                                let out = u["output_tokens"].as_u64().unwrap_or(0);
                                if out > 0 && !emitted_usage {
                                    let _ = tx.send(Ok(StreamChunk::Usage(Usage {
                                        input_tokens: 0, output_tokens: out as usize,
                                        cache_creation_input_tokens: None, cache_read_input_tokens: None,
                                    }))).await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                // Flush accumulated tool calls
                for (_idx, id, name, json) in &pending_tools {
                    let input: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
                    let tool_id = id.parse::<uuid::Uuid>().unwrap_or_else(|_| uuid::Uuid::new_v4());
                    let _ = tx.send(Ok(StreamChunk::ContentBlockStart {
                        block: ContentBlock::ToolUse {
                            id: ToolCallId(tool_id), name: name.clone(), input,
                        },
                    })).await;
                }
            } else {
                // ── OpenAI SSE: standard chat completions streaming ──
                let mut pending_tool_calls: Vec<(usize, Option<String>, Option<String>, String)> = Vec::new();

                while let Some(chunk_result) = stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => { let _ = tx.send(Err(AgentError::provider(format!("Stream error: {e}")))).await; return; }
                    };
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(line_end) = buffer.find('\n') {
                        let line = buffer[..line_end].trim().to_string();
                        buffer.drain(..=line_end);
                        let data = line.strip_prefix("data: ").unwrap_or(&line);
                        if data.is_empty() || data == "[DONE]" { continue; }
                        let Ok(parsed) = serde_json::from_str::<OaiStreamChunk>(data) else { continue; };

                        if let Some(ref u) = parsed.usage {
                            let _ = tx.send(Ok(StreamChunk::Usage(Usage {
                                input_tokens: u.prompt_tokens, output_tokens: u.completion_tokens,
                                cache_creation_input_tokens: None, cache_read_input_tokens: None,
                            }))).await;
                        }

                        for choice in parsed.choices {
                            if let Some(ref fr) = choice.finish_reason {
                                let _ = tx.send(Ok(StreamChunk::Stop(MiniMaxClient::parse_stop_reason(fr)))).await;
                                continue;
                            }
                            let delta = choice.delta;
                            if let Some(ref reasoning) = delta.reasoning.filter(|r| !r.is_empty()) {
                                let _ = tx.send(Ok(StreamChunk::TextDelta {
                                    text: format!("<thinking>{reasoning}</thinking>"),
                                })).await;
                            }
                            if let Some(ref text) = delta.content.filter(|t| !t.is_empty()) {
                                let _ = tx.send(Ok(StreamChunk::TextDelta { text: text.clone() })).await;
                            }
                            for tc in &delta.tool_calls {
                                let idx = tc.index;
                                let id = tc.id.clone();
                                let name = tc.function.as_ref().and_then(|f| f.name.clone());
                                let args = tc.function.as_ref().and_then(|f| f.arguments.clone()).unwrap_or_default();
                                if let Some(existing) = pending_tool_calls.iter_mut().find(|(i, _, _, _)| *i == idx) {
                                    if let Some(a) = existing.2.as_mut() { a.push_str(&args); }
                                    if id.is_some() { existing.1 = id; }
                                    if name.is_some() { existing.3 = name.unwrap_or_default(); }
                                } else {
                                    pending_tool_calls.push((idx, id, Some(args), name.unwrap_or_default()));
                                }
                            }
                        }
                    }
                }
                // Flush accumulated tool calls
                for (_idx, id, args, name) in &pending_tool_calls {
                    if let (Some(_id), Some(args)) = (id, args) {
                        let input: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
                        let _ = tx.send(Ok(StreamChunk::ContentBlockStart {
                            block: ContentBlock::ToolUse {
                                id: ToolCallId(uuid::Uuid::new_v4()), name: name.clone(), input,
                            },
                        })).await;
                    }
                }
            }
        });

        Ok(StreamResponse { content_receiver: rx })
    }
}

// ── Public convenience types ───────────────────────────────────

/// MiniMax currently exposes one M-series model; flash and pro tiers both
/// map to it (override with `MINIMAX_MODEL_NAME`).
pub struct MiniMaxFlash;

impl MiniMaxFlash {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(api_key: &ApiKey) -> MiniMaxClient {
        MiniMaxClient::new(api_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(base_url: &str) -> MiniMaxClient {
        MiniMaxClient {
            client: Client::builder().build().unwrap(),
            base_url: base_url.into(),
            api_key: "test".into(),
            model: DEFAULT_MODEL.into(),
            anthropic: is_anthropic_mode(base_url),
        }
    }

    #[test]
    fn openai_request_serializes() {
        let client = test_client("https://api.minimaxi.com/v1");
        let request = CompletionRequest {
            system: "be brief".into(),
            messages: vec![
                miniagent_core::message::Message::user("hello"),
                miniagent_core::message::Message::assistant_text("hi"),
            ],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig::default(),
        };
        let body = client.build_oai_request(&request, false);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "MiniMax-M3");
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert!(json.get("tools").is_none());
        assert_eq!(json["stream"], false);
    }

    #[test]
    fn anth_request_serializes() {
        let client = test_client("https://api.minimaxi.com/anthropic");
        let request = CompletionRequest {
            system: "be brief".into(),
            messages: vec![
                miniagent_core::message::Message::user("hello"),
                miniagent_core::message::Message::assistant_text("hi"),
            ],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig::default(),
        };
        let body = client.build_anth_request(&request, false);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["system"], "be brief");
        assert_eq!(json["model"], "MiniMax-M3");
        assert!(json["max_tokens"].as_u64().unwrap() > 0);
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn oai_tool_roundtrip() {
        let client = test_client("https://api.minimaxi.com/v1");
        let request = CompletionRequest {
            system: String::new(),
            messages: vec![
                miniagent_core::message::Message::user("run it"),
                miniagent_core::message::Message::assistant(vec![ContentBlock::ToolUse {
                    id: ToolCallId(uuid::Uuid::nil()), name: "bash".into(),
                    input: serde_json::json!({"cmd": "ls"}),
                }]),
                miniagent_core::message::Message::tool(uuid::Uuid::nil().to_string(), "file1"),
            ],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig::default(),
        };
        let json = serde_json::to_value(client.build_oai_request(&request, false)).unwrap();
        let msgs = json["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], "bash");
        assert!(msgs[2]["tool_call_id"].is_string());
    }

    #[test]
    fn anth_tool_roundtrip() {
        let client = test_client("https://api.minimaxi.com/anthropic");
        let request = CompletionRequest {
            system: String::new(),
            messages: vec![
                miniagent_core::message::Message::user("run it"),
                miniagent_core::message::Message::assistant(vec![ContentBlock::ToolUse {
                    id: ToolCallId(uuid::Uuid::nil()), name: "bash".into(),
                    input: serde_json::json!({"cmd": "ls"}),
                }]),
                miniagent_core::message::Message::tool(uuid::Uuid::nil().to_string(), "file1"),
            ],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig::default(),
        };
        let json = serde_json::to_value(client.build_anth_request(&request, false)).unwrap();
        let msgs = json["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        let tool_use_id = msgs[1]["content"][0]["id"].as_str().unwrap();
        let tool_result_id = msgs[2]["content"][0]["tool_use_id"].as_str().unwrap();
        assert_eq!(tool_use_id, tool_result_id);
        assert_eq!(msgs[2]["content"][0]["type"].as_str().unwrap(), "tool_result");
    }

    #[test]
    fn oai_url_construction() {
        let c = test_client("https://api.minimaxi.com/v1");
        assert_eq!(c.api_url(), "https://api.minimaxi.com/v1/chat/completions");
    }

    #[test]
    fn oai_url_adds_v1() {
        let c = test_client("https://api.minimaxi.com");
        assert_eq!(c.api_url(), "https://api.minimaxi.com/v1/chat/completions");
    }

    #[test]
    fn anth_url_construction() {
        let c = test_client("https://api.minimaxi.com/anthropic");
        assert_eq!(c.api_url(), "https://api.minimaxi.com/anthropic/v1/messages");
    }

    #[test]
    fn protocol_detection() {
        assert!(is_anthropic_mode("https://api.minimaxi.com/anthropic"));
        assert!(is_anthropic_mode("https://api.minimaxi.com/ANTHROPIC"));
        assert!(!is_anthropic_mode("https://api.minimaxi.com/v1"));
    }
}
