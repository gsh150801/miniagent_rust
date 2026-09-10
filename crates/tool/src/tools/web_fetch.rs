use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// 域级熔断：同一 host 连续 N 次网络层失败（连接/DNS/5xx/429）后，
/// 在冷却窗口内快速失败。live 观察：代理环境下 agent 对 arxiv.org 连续
/// 4 次 150ms 失败 + wikipedia 30s 超时后仍反复重试同域，浪费大量轮次。
/// 403/404 属页面级问题，不计入。
const DOMAIN_FAILURE_THRESHOLD: u32 = 3;
const DOMAIN_BLOCK_SECS: u64 = 60;

#[derive(Default, Clone, Copy)]
struct DomainState {
    /// 连续失败计数（未开闸时累加；开闸/成功/冷却过期时清零）。
    fails: u32,
    /// 熔断开闸时刻之后的时间点；None = 未开闸。
    blocked_until: Option<std::time::Instant>,
}

fn domain_breaker() -> &'static std::sync::Mutex<std::collections::HashMap<String, DomainState>> {
    static BREAKER: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, DomainState>>> =
        std::sync::OnceLock::new();
    BREAKER.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// host 被熔断时返回 Some(剩余秒数)；冷却过期条目顺带清理（计数归零重计）。
fn breaker_blocked(host: &str) -> Option<u64> {
    let mut map = domain_breaker().lock().ok()?;
    let state = map.get_mut(host)?;
    let Some(until) = state.blocked_until else {
        return None; // 仅计数未开闸：不清除，保留累计
    };
    let remain = until.saturating_duration_since(std::time::Instant::now());
    if remain.is_zero() {
        // 冷却结束：整条复位（计数从零开始）
        map.remove(host);
        None
    } else {
        Some(remain.as_secs())
    }
}

fn breaker_record_failure(host: &str) {
    if let Ok(mut map) = domain_breaker().lock() {
        let state = map.entry(host.to_string()).or_default();
        state.fails += 1;
        if state.fails >= DOMAIN_FAILURE_THRESHOLD {
            state.blocked_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(DOMAIN_BLOCK_SECS));
            tracing::warn!(host = %host, block_secs = DOMAIN_BLOCK_SECS,
                "web_fetch domain circuit opened — fast-failing this host temporarily");
        }
    }
}

fn breaker_record_success(host: &str) {
    if let Ok(mut map) = domain_breaker().lock() {
        map.remove(host);
    }
}

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
        "Fetch a URL and return its content as markdown text. Handles HTML to text conversion.\n\n\
         Tips: Use this to read full articles, documentation, or papers found via web_search \
         or pubmed_search. For non-English pages, the content will be returned in the original \
         language — you can summarize/translate it for the user in your response."
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

        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();
        if let Some(remain) = breaker_blocked(&host) {
            return Err(AgentError::tool("web_fetch", format!(
                "domain '{host}' temporarily blocked by circuit breaker ({remain}s left, \
                 after repeated network failures). Skip this domain for now — use a \
                 different source or search result instead."
            )));
        }

        let response = match tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(url).send() => r,
        } {
            Ok(r) => r,
            Err(e) => {
                breaker_record_failure(&host);
                return Err(AgentError::tool("web_fetch", format!("HTTP error: {e}")));
            }
        };

        let status = response.status();
        if !status.is_success() {
            // 5xx/429 视为域级故障信号；4xx 其他码多为页面级，不熔断。
            if status.is_server_error() || status.as_u16() == 429 {
                breaker_record_failure(&host);
            }
            return Err(AgentError::tool("web_fetch", format!("HTTP {status}")));
        }
        breaker_record_success(&host);

        let body = response
            .text()
            .await
            .map_err(|e| AgentError::tool("web_fetch", format!("read body: {e}")))?;

        // Simple HTML to text: strip tags
        let text = strip_html(&body);
        let truncated = if text.len() > max_len {
            // Use char_indices to avoid breaking multi-byte UTF-8 characters
            let mut end = max_len;
            while !text.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            format!("{}... (truncated, original: {} chars)", &text[..end], text.len())
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
        if lower[i..].starts_with("<style")
            && let Some(end) = lower[i..].find("</style>") {
                i += end + "</style>".len();
                continue;
            }
        if lower[i..].starts_with("<script")
            && let Some(end) = lower[i..].find("</script>") {
                i += end + "</script>".len();
                continue;
            }
        if lower[i..].starts_with("<!--")
            && let Some(end) = lower[i..].find("-->") {
                i += end + "-->".len();
                continue;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_threshold_and_recovers() {
        // 独立实例测试熔断状态机（全局 map 以 host 隔离，不与其他测试互扰）
        let host = "breaker-test.invalid";
        assert!(breaker_blocked(host).is_none(), "fresh host must not be blocked");

        breaker_record_failure(host);
        breaker_record_failure(host);
        // 中间插入 blocked() 探测（真实执行路径每次 fetch 前都会探测）：
        // 未开闸的计数必须保留，不能被探测清掉。
        assert!(breaker_blocked(host).is_none(), "below threshold must not block");

        breaker_record_failure(host);
        let remain = breaker_blocked(host);
        assert!(remain.is_some(), "third failure must open the circuit");
        assert!(remain.unwrap() <= DOMAIN_BLOCK_SECS);

        // 成功后清除：模拟冷却结束由 breaker_blocked 清理，此处直接验证
        // success 路径也能复位（冷却窗口内的成功同样解除熔断）。
        breaker_record_success(host);
        assert!(breaker_blocked(host).is_none(), "success must clear the breaker");
    }

    #[test]
    fn breaker_expired_entry_resets_count() {
        // 手工注入一个已过期的熔断条目：blocked() 应顺带清理，
        // 后续失败从 0 重新计数（不会一次失败就重新开闸）。
        let host = "breaker-expired.invalid";
        domain_breaker().lock().unwrap().insert(
            host.to_string(),
            DomainState {
                fails: 99,
                blocked_until: Some(std::time::Instant::now() - std::time::Duration::from_secs(1)),
            },
        );
        assert!(breaker_blocked(host).is_none(), "expired entry must not block");
        breaker_record_failure(host);
        assert!(breaker_blocked(host).is_none(), "count must reset after expiry");
    }
}
