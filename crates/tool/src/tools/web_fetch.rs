use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct WebFetchTool {
    client: reqwest::Client,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("miniagent/0.1")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }
    fn description(&self) -> &str {
        "Fetch a URL and return its content as markdown text. Handles HTML to text conversion."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to fetch"},
                "max_length": {"type": "integer", "description": "Maximum characters to return (default: 50000)"}
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let url = input["url"].as_str()
            .ok_or_else(|| AgentError::tool("web_fetch", "missing 'url'"))?;
        let max_len = input["max_length"].as_u64().unwrap_or(50000) as usize;

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(url).send() => r,
        }
        .map_err(|e| AgentError::tool("web_fetch", format!("HTTP error: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(AgentError::tool("web_fetch", format!("HTTP {status}")));
        }

        let body = response
            .text()
            .await
            .map_err(|e| AgentError::tool("web_fetch", format!("read body: {e}")))?;

        // Simple HTML to text: strip tags
        let text = strip_html(&body);
        let truncated = if text.len() > max_len {
            format!("{}... (truncated, original: {} chars)", &text[..max_len], text.len())
        } else {
            text
        };

        Ok(ToolOutput {
            content: truncated,
            metadata: None,
        })
    }
}

fn strip_html(html: &str) -> String {
    // Pre-process: remove <style>, <script>, and comment blocks entirely
    let mut cleaned = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0;

    while i < html.len() {
        if lower[i..].starts_with("<style") {
            if let Some(end) = lower[i..].find("</style>") {
                i += end + "</style>".len();
                continue;
            }
        }
        if lower[i..].starts_with("<script") {
            if let Some(end) = lower[i..].find("</script>") {
                i += end + "</script>".len();
                continue;
            }
        }
        if lower[i..].starts_with("<!--") {
            if let Some(end) = lower[i..].find("-->") {
                i += end + "-->".len();
                continue;
            }
        }

        // Safe char-boundary advancement
        let ch = html[i..].chars().next().unwrap_or('\0');
        cleaned.push(ch);
        i += ch.len_utf8();
    }

    // Strip remaining HTML tags using char iteration
    let mut result = String::with_capacity(cleaned.len());
    let mut in_tag = false;

    for ch in cleaned.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    // Decode common HTML entities
    let result = result.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");

    // Collapse whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}
