//! MiniMax provider over the Anthropic-compatible Messages API.
//!
//! Targets the MiniMax Token-Plan (subscription) endpoint
//! `https://api.minimaxi.com/anthropic` (Anthropic Messages protocol,
//! models like `MiniMax-M3`). Subscription keys are issued from the
//! Token Plan console; pay-as-you-go platform keys are NOT interchangeable.
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
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamChunk, StreamResponse};

const DEFAULT_BASE_URL: &str = "https://api.minimaxi.com/anthropic";
const DEFAULT_MODEL: &str = "MiniMax-M3";
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ── Anthropic Messages wire types (subset) ─────────────────────

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    system: String,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    stream: bool,
}

#[derive(Serialize)]
struct WireMessage {
    role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    content_text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    content_blocks: Vec<serde_json::Value>,
}

impl WireMessage {
    fn text(role: &str, text: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content_text: text.into(),
            content_blocks: Vec::new(),
        }
    }

    fn blocks(role: &str, blocks: Vec<serde_json::Value>) -> Self {
        Self {
            role: role.to_string(),
            content_text: String::new(),
            content_blocks: blocks,
        }
    }
}

#[derive(Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlockRaw>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<UsageRaw>,
}

#[derive(Debug, serde::Deserialize)]
struct ContentBlockRaw {
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
struct UsageRaw {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

// ── Client ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct MiniMaxClient {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl MiniMaxClient {
    pub fn new(api_key: &ApiKey) -> Self {
        Self::with_model(api_key, DEFAULT_MODEL)
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
            base_url: std::env::var("MINIMAX_BASE_URL")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key: api_key.as_str().to_string(),
            model: std::env::var("MINIMAX_MODEL_NAME")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| model.to_string()),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// `ALL_PROXY` → `HTTPS_PROXY` → `HTTP_PROXY` (plus lowercase variants).
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

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn build_request(&self, request: &CompletionRequest, stream: bool) -> MessagesRequest {
        let mut system = request.system.clone();
        let mut messages: Vec<WireMessage> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    let text = msg.text_content();
                    if !system.is_empty() && !text.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&text);
                }
                MessageRole::User => {
                    messages.push(WireMessage::text("user", msg.text_content()));
                }
                MessageRole::Assistant => {
                    let mut blocks = Vec::new();
                    for b in &msg.content {
                        match b {
                            ContentBlock::Text { text } if !text.is_empty() => {
                                blocks.push(serde_json::json!({"type": "text", "text": text}));
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                // Round-trip the raw id so later tool_result
                                // messages (which carry the same id) pair up.
                                blocks.push(serde_json::json!({
                                    "type": "tool_use",
                                    "id": format!("{}", id.0),
                                    "name": name,
                                    "input": input,
                                }));
                            }
                            _ => {}
                        }
                    }
                    if blocks.is_empty() {
                        // Degenerate empty assistant turn; keep the protocol happy.
                        blocks.push(serde_json::json!({"type": "text", "text": " "}));
                    }
                    messages.push(WireMessage::blocks("assistant", blocks));
                }
                MessageRole::Tool => {
                    // Our tool results embed the call id as "[toolu_vrtx_{id}] result".
                    let text = msg.text_content();
                    let (tool_use_id, content) = text
                        .strip_prefix("[toolu_vrtx_")
                        .and_then(|s| s.split_once(']'))
                        .map(|(id, rest)| (id.to_string(), rest.trim_start().to_string()))
                        .unwrap_or_else(|| (uuid::Uuid::new_v4().to_string(), text.clone()));
                    messages.push(WireMessage::blocks(
                        "user",
                        vec![serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                        })],
                    ));
                }
            }
        }

        let tools = request
            .tools
            .iter()
            .map(|t| WireTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            })
            .collect();

        MessagesRequest {
            model: self.model.clone(),
            // The Messages API requires max_tokens; mirror the agent default
            // when the caller leaves it unset.
            max_tokens: request.config.max_tokens.unwrap_or(8192),
            temperature: request.config.temperature,
            top_p: request.config.top_p,
            system,
            messages,
            tools,
            stream,
        }
    }

    fn parse_response(&self, response: MessagesResponse) -> CompletionResponse {
        let mut content = Vec::new();
        for block in response.content {
            match block.block_type.as_str() {
                "text" => {
                    if let Some(text) = block.text.filter(|t| !t.is_empty()) {
                        content.push(ContentBlock::Text { text });
                    }
                }
                "tool_use" => {
                    let id = block
                        .id
                        .and_then(|s| s.parse::<uuid::Uuid>().ok())
                        .unwrap_or_else(uuid::Uuid::new_v4);
                    content.push(ContentBlock::ToolUse {
                        id: ToolCallId(id),
                        name: block.name.unwrap_or_default(),
                        input: block.input.unwrap_or_default(),
                    });
                }
                _ => {} // thinking / redacted_thinking → omitted
            }
        }

        let stop_reason = match response.stop_reason.as_deref() {
            Some("max_tokens") => StopReason::MaxTokens,
            Some("tool_use") => StopReason::ToolUse,
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
                input_tokens: u.input_tokens as usize,
                output_tokens: u.output_tokens as usize,
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

    fn parse_stop_reason(reason: &str) -> StopReason {
        match reason {
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        }
    }
}

// ── LlmProvider Implementation ─────────────────────────────────

#[async_trait]
impl LlmProvider for MiniMaxClient {
    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        let body = self.build_request(request, false);
        let url = self.messages_url();

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send() => r,
        }
        .map_err(|e| {
            let cause = e
                .source()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            match cause {
                Some(c) => AgentError::provider(format!("HTTP request failed: {e} (cause: {c})")),
                None => AgentError::provider(format!("HTTP request failed: {e}")),
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::provider(format!(
                "MiniMax API error {status}: {body}"
            )));
        }

        let parsed: MessagesResponse = response
            .json()
            .await
            .map_err(|e| AgentError::provider(format!("Failed to parse response: {e}")))?;

        Ok(self.parse_response(parsed))
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError> {
        let body = self.build_request(request, true);
        let url = self.messages_url();

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
                    .header("x-api-key", api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(&body)
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
                        "MiniMax API error {status}: {body}"
                    ))))
                    .await;
                return;
            }

            // SSE event stream: text deltas flow straight through; tool_use
            // blocks are accumulated (content_block_start + input_json_delta)
            // and emitted once complete, mirroring the other providers.
            use futures_util::StreamExt;
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut pending_tools: Vec<(usize, String, String, String)> = Vec::new(); // (index, id, name, json)
            let mut emitted_usage = false;

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
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    if data == "[DONE]" || data.is_empty() {
                        continue;
                    }
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };

                    match event["type"].as_str().unwrap_or_default() {
                        "message_start" => {
                            let u = &event["message"]["usage"];
                            let (inp, out) = (
                                u["input_tokens"].as_u64().unwrap_or(0),
                                u["output_tokens"].as_u64().unwrap_or(0),
                            );
                            if inp > 0 || out > 0 {
                                emitted_usage = true;
                                let _ = tx
                                    .send(Ok(StreamChunk::Usage(Usage {
                                        input_tokens: inp as usize,
                                        output_tokens: out as usize,
                                        cache_creation_input_tokens: None,
                                        cache_read_input_tokens: None,
                                    })))
                                    .await;
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
                                    if let Some(text) =
                                        delta["text"].as_str().filter(|t| !t.is_empty())
                                    {
                                        let _ = tx
                                            .send(Ok(StreamChunk::TextDelta {
                                                text: text.to_string(),
                                            }))
                                            .await;
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(index) = event["index"].as_u64() {
                                        if let Some(pt) = pending_tools
                                            .iter_mut()
                                            .find(|(i, _, _, _)| *i == index as usize)
                                        {
                                            pt.3.push_str(
                                                delta["partial_json"].as_str().unwrap_or_default(),
                                            );
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        "message_delta" => {
                            if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                                let _ = tx
                                    .send(Ok(StreamChunk::Stop(
                                        MiniMaxClient::parse_stop_reason(reason),
                                    )))
                                    .await;
                            }
                            let u = &event["usage"];
                            let out = u["output_tokens"].as_u64().unwrap_or(0);
                            if out > 0 && !emitted_usage {
                                let _ = tx
                                    .send(Ok(StreamChunk::Usage(Usage {
                                        input_tokens: 0,
                                        output_tokens: out as usize,
                                        cache_creation_input_tokens: None,
                                        cache_read_input_tokens: None,
                                    })))
                                    .await;
                            }
                        }
                        _ => {}
                    }
                }
            }

            for (_idx, id, name, json) in &pending_tools {
                let input: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
                let tool_id = id
                    .parse::<uuid::Uuid>()
                    .unwrap_or_else(|_| uuid::Uuid::new_v4());
                let _ = tx
                    .send(Ok(StreamChunk::ContentBlockStart {
                        block: ContentBlock::ToolUse {
                            id: ToolCallId(tool_id),
                            name: name.clone(),
                            input,
                        },
                    }))
                    .await;
            }
        });

        Ok(StreamResponse {
            content_receiver: rx,
        })
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

    fn test_client() -> MiniMaxClient {
        MiniMaxClient {
            client: Client::builder().build().unwrap(),
            base_url: DEFAULT_BASE_URL.into(),
            api_key: "test".into(),
            model: DEFAULT_MODEL.into(),
        }
    }

    #[test]
    fn request_serializes_anthropic_shape() {
        let client = test_client();
        let request = CompletionRequest {
            system: "be brief".into(),
            messages: vec![
                miniagent_core::message::Message::user("hello"),
                miniagent_core::message::Message::assistant_text("hi"),
            ],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig::default(),
        };
        let body = client.build_request(&request, false);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["system"], "be brief");
        assert_eq!(json["model"], "MiniMax-M3");
        assert!(json["max_tokens"].as_u64().unwrap() > 0);
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        // empty tools are omitted (Anthropic rejects empty tool arrays)
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn tool_roundtrip_pairs_tool_use_and_result() {
        let client = test_client();
        let request = CompletionRequest {
            system: String::new(),
            messages: vec![
                miniagent_core::message::Message::user("run it"),
                miniagent_core::message::Message::assistant(vec![ContentBlock::ToolUse {
                    id: ToolCallId(uuid::Uuid::nil()),
                    name: "bash".into(),
                    input: serde_json::json!({"cmd": "ls"}),
                }]),
                miniagent_core::message::Message::tool(
                    uuid::Uuid::nil().to_string(),
                    "file1",
                ),
            ],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig::default(),
        };
        let json = serde_json::to_value(client.build_request(&request, false)).unwrap();
        let msgs = json["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3); // user, assistant(tool_use), user(tool_result)
        let tool_use_id = msgs[1]["content_blocks"][0]["id"].as_str().unwrap();
        let tool_result_id = msgs[2]["content_blocks"][0]["tool_use_id"].as_str().unwrap();
        assert_eq!(tool_use_id, tool_result_id); // ids pair up
        assert_eq!(
            msgs[2]["content_blocks"][0]["type"].as_str().unwrap(),
            "tool_result"
        );
    }

    #[test]
    fn messages_url_appends_path() {
        let mut client = test_client();
        client.base_url = "https://api.minimaxi.com/anthropic/".into();
        assert_eq!(
            client.messages_url(),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );
    }
}
