use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::health;

pub struct WebSearchTool {
    client: reqwest::Client,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("miniagent/0.1")
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str {
        "Search the web. Automatically uses available API (Serper > Tavily > Bocha). \
         Returns titles, snippets, and links. Add 'site:pubmed.ncbi.nlm.nih.gov' for PubMed, \
         'site:arxiv.org' for ArXiv.\n\n\
         IMPORTANT query guidelines:\n\
         - ALWAYS use English search queries. If the user's request is in another \
         language (Chinese, Japanese, etc.), translate the key concepts to English \
         before searching — English queries yield far more relevant results.\n\
         - If the first search returns few relevant results, do NOT give up. \
         Retry with different keywords: try synonyms, broader terms, or narrower \
         terms. For example, if 'pancreatic cancer immunotherapy 2024' yields few \
         results, try 'pancreatic adenocarcinoma immune checkpoint', or broaden to \
         'pancreatic cancer treatment advances'.\n\
         - For academic/literature searches, prefer precise domain terminology \
         and consider adding site: filters (site:pubmed.ncbi.nlm.nih.gov, \
         site:nature.com, site:sciencedirect.com)."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query in ENGLISH for best results. Translate non-English queries to English. Use precise domain terminology."},
                "num": {"type": "integer", "description": "Number of results (default: 10, max: 50)"},
                "backend": {"type": "string", "description": "Force backend: serper, tavily, bocha"}
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let query = input["query"].as_str()
            .ok_or_else(|| AgentError::tool("web_search", "missing 'query'"))?;
        let num = input["num"].as_u64().unwrap_or(10).min(50);
        let backend = input["backend"].as_str();

        // Try backends in order: Serper → Tavily → Bocha → LangSearch → DDG
        if let Some("tavily") = backend {
            return self.search_tavily(query, num, cancel).await;
        }
        if let Some("bocha") = backend {
            return self.search_bocha(query, num, cancel).await;
        }
        if let Some("langsearch") = backend {
            return self.search_langsearch(query, num, cancel).await;
        }
        if let Some("ddgs") = backend {
            return self.search_ddgs(query, num, cancel).await;
        }

        // Read global health state to skip unhealthy/disabled backends.
        // Circuit breaker: backends that fail at runtime are disabled for 120s
        // to avoid repeated retries and log spam.
        let serper_key = env_opt("SERPER_API_KEY").or_else(|| env_opt("SERPAPI_API_KEY"));
        let tavily_key = env_opt("TAVILY_API_KEY");
        let bocha_key = env_opt("BOCHA_API_KEY");
        let langsearch_key = env_opt("LANGSEARCH_API_KEY");

        // Try Serper
        if let Some(ref key) = serper_key {
            let hs = health::health_state().await;
            let usable = hs.is_healthy("serper");
            let disabled = hs.is_disabled("serper");
            drop(hs);
            if usable {
                match self.search_serper(query, num, key, cancel.clone()).await {
                    Ok(out) => { self.mark_backend_ok("serper").await; return Ok(out); }
                    Err(e) => { self.disable_backend("serper", &e).await; }
                }
            } else if !disabled {
                eprintln!("[web_search] serper skipped (unhealthy from startup probe)");
            }
        }
        // Try Tavily
        if let Some(ref key) = tavily_key {
            let hs = health::health_state().await;
            let usable = hs.is_healthy("tavily");
            let disabled = hs.is_disabled("tavily");
            drop(hs);
            if usable {
                match self.search_tavily_with_key(query, num, key, cancel.clone()).await {
                    Ok(out) => { self.mark_backend_ok("tavily").await; return Ok(out); }
                    Err(e) => { self.disable_backend("tavily", &e).await; }
                }
            } else if !disabled {
                eprintln!("[web_search] tavily skipped (unhealthy from startup probe)");
            }
        }
        // Try Bocha
        if bocha_key.is_some() {
            let hs = health::health_state().await;
            let usable = hs.is_healthy("bocha");
            let disabled = hs.is_disabled("bocha");
            drop(hs);
            if usable {
                match self.search_bocha(query, num, cancel.clone()).await {
                    Ok(out) => { self.mark_backend_ok("bocha").await; return Ok(out); }
                    Err(e) => { self.disable_backend("bocha", &e).await; }
                }
            } else if !disabled {
                eprintln!("[web_search] bocha skipped (unhealthy from startup probe)");
            }
        }
        // Try LangSearch
        if langsearch_key.is_some() {
            let hs = health::health_state().await;
            let usable = hs.is_healthy("langsearch");
            let disabled = hs.is_disabled("langsearch");
            drop(hs);
            if usable {
                match self.search_langsearch(query, num, cancel.clone()).await {
                    Ok(out) => { self.mark_backend_ok("langsearch").await; return Ok(out); }
                    Err(e) => { self.disable_backend("langsearch", &e).await; }
                }
            } else if !disabled {
                eprintln!("[web_search] langsearch skipped (unhealthy from startup probe)");
            }
        }
        // Try DDG (no key required — always available unless blocked)
        {
            let hs = health::health_state().await;
            let usable = hs.is_healthy("ddgs");
            let disabled = hs.is_disabled("ddgs");
            drop(hs);
            if usable {
                match self.search_ddgs(query, num, cancel.clone()).await {
                    Ok(out) => { self.mark_backend_ok("ddgs").await; return Ok(out); }
                    Err(e) => { self.disable_backend("ddgs", &e).await; }
                }
            } else if !disabled {
                eprintln!("[web_search] ddgs skipped (unhealthy from startup probe)");
            }
        }
        Err(AgentError::tool("web_search", "All search backends (Serper, Tavily, Bocha, LangSearch, DDG) failed or were unavailable"))
    }
}

impl WebSearchTool {
    /// Mark a backend as healthy (clear circuit breaker).
    async fn mark_backend_ok(&self, name: &str) {
        let hs = health::health_state().await;
        let was_disabled = hs.is_disabled(name);
        drop(hs);
        if was_disabled {
            let mut hw = health::health_state_mut().await;
            hw.mark_healthy(name);
            eprintln!("[web_search] {} recovered — re-enabled", name);
        }
    }

    /// Disable a backend via circuit breaker (supresses retries for 120s).
    async fn disable_backend(&self, name: &str, error: &miniagent_core::error::AgentError) {
        let mut hw = health::health_state_mut().await;
        let already_disabled = hw.is_disabled(name);
        hw.disable_runtime(name);
        if !already_disabled {
            eprintln!("[web_search] {} failed ({}) — disabled for 120s", name, error);
        }
    }

    async fn search_serper(
        &self, query: &str, num: u64, api_key: &str, cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let body = json!({ "q": query, "num": num });

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.post("https://google.serper.dev/search")
                .header("X-API-KEY", api_key).json(&body).send() => r,
        }.map_err(|e| AgentError::tool("serper", format!("HTTP: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let b = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("serper", format!("{status}: {b}")));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("serper", format!("parse: {e}")))?;

        let mut out = String::from("## Serper Search Results\n\n");
        if let Some(items) = result["organic"].as_array() {
            for (i, item) in items.iter().enumerate() {
                let t = item["title"].as_str().unwrap_or("");
                let l = item["link"].as_str().unwrap_or("");
                let s = item["snippet"].as_str().unwrap_or("");
                out.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n", i + 1, t, l, s));
            }
        }
        if out.is_empty() || out == "## Serper Search Results\n\n" {
            out.push_str(&format!("No results for '{}'", query));
        }
        Ok(ToolOutput { content: out, metadata: None })
    }

    async fn search_tavily(
        &self, query: &str, num: u64, cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let key = env_opt("TAVILY_API_KEY").unwrap_or_default();
        if key.is_empty() {
            return Ok(ToolOutput {
                content: "Tavily search unavailable: TAVILY_API_KEY not set.".into(),
                metadata: None,
            });
        }
        self.search_tavily_with_key(query, num, &key, cancel).await
    }

    async fn search_tavily_with_key(
        &self, query: &str, num: u64, api_key: &str, cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let body = json!({
            "api_key": api_key,
            "query": query,
            "max_results": num.min(20),
            "search_depth": "basic",
        });

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.post("https://api.tavily.com/search")
                .json(&body).send() => r,
        }.map_err(|e| AgentError::tool("tavily", format!("HTTP: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let b = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("tavily", format!("{status}: {b}")));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("tavily", format!("parse: {e}")))?;

        let mut out = String::from("## Tavily Search Results\n\n");
        if let Some(items) = result["results"].as_array() {
            for (i, item) in items.iter().enumerate() {
                let t = item["title"].as_str().unwrap_or("");
                let u = item["url"].as_str().unwrap_or("");
                let c = item["content"].as_str().unwrap_or("");
                out.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n", i + 1, t, u, c));
            }
        }
        if out.is_empty() || out == "## Tavily Search Results\n\n" {
            out.push_str(&format!("No results for '{}'", query));
        }
        Ok(ToolOutput { content: out, metadata: None })
    }

    async fn search_bocha(
        &self, query: &str, num: u64, cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let key = env_opt("BOCHA_API_KEY").unwrap_or_default();
        if key.is_empty() {
            return Ok(ToolOutput {
                content: "Bocha search unavailable: BOCHA_API_KEY not set.".into(),
                metadata: None,
            });
        }

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.post("https://api.bochaai.com/v1/web-search")
                .header("Authorization", format!("Bearer {key}"))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "query": query,
                    "count": num,
                    "summary": true,
                }))
                .send() => r,
        }.map_err(|e| AgentError::tool("bocha", format!("HTTP: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let b = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("bocha", format!("{status}: {b}")));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("bocha", format!("parse: {e}")))?;

        let mut out = String::from("## Bocha Search Results\n\n");
        if let Some(items) = result["data"]["webPages"]["value"].as_array() {
            for (i, item) in items.iter().enumerate() {
                let t = item["name"].as_str().unwrap_or("");
                let u = item["url"].as_str().unwrap_or("");
                let s = item["snippet"].as_str().unwrap_or("");
                out.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n", i + 1, t, u, s));
            }
        }
        if out == "## Bocha Search Results\n\n" {
            // Try alternative result structure
            if let Some(items) = result["data"]["webPages"]["searchResults"].as_array() {
                for (i, item) in items.iter().enumerate() {
                    let t = item["title"].as_str().or_else(|| item["name"].as_str()).unwrap_or("");
                    let u = item["url"].as_str().unwrap_or("");
                    let s = item["snippet"].as_str().unwrap_or("");
                    out.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n", i + 1, t, u, s));
                }
            }
        }
        if out == "## Bocha Search Results\n\n" {
            out.push_str(&format!("No results for '{}'", query));
        }
        Ok(ToolOutput { content: out, metadata: None })
    }

    /// Search via LangSearch API (OpenAI-compatible search, requires API key).
    async fn search_langsearch(&self, query: &str, num: u64, cancel: CancellationToken) -> Result<ToolOutput, AgentError> {
        let key = env_opt("LANGSEARCH_API_KEY")
            .ok_or_else(|| AgentError::tool("web_search", "LANGSEARCH_API_KEY not set"))?;

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.post("https://api.langsearch.com/v1/web-search")
                .header("Authorization", format!("Bearer {key}"))
                .header("Content-Type", "application/json")
                .json(&json!({"query": query, "num": num}))
                .send() => r,
        }.map_err(|e| AgentError::tool("langsearch", format!("HTTP: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let b = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("langsearch", format!("{status}: {b}")));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("langsearch", format!("parse: {e}")))?;

        let mut out = String::from("## LangSearch Results\n\n");
        if let Some(items) = result["data"]["webPages"]["value"].as_array() {
            for (i, item) in items.iter().take(num as usize).enumerate() {
                let t = item["name"].as_str().unwrap_or("");
                let u = item["url"].as_str().unwrap_or("");
                let s = item["snippet"].as_str().unwrap_or("");
                out.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n", i + 1, t, u, s));
            }
        }
        if out == "## LangSearch Results\n\n" {
            out.push_str(&format!("No results for '{}'", query));
        }

        Ok(ToolOutput { content: out, metadata: None })
    }

    /// Search via DuckDuckGo HTML endpoint (no API key required).
    /// Uses a proxy if the `all_proxy` or `https_proxy` env var is set.
    async fn search_ddgs(&self, query: &str, num: u64, cancel: CancellationToken) -> Result<ToolOutput, AgentError> {
        // DDG may be blocked from some regions — use proxy if configured.
        let proxy_url = env_opt("all_proxy")
            .or_else(|| env_opt("https_proxy"))
            .or_else(|| env_opt("http_proxy"));

        let client = if let Some(ref proxy_str) = proxy_url {
            // Parse proxy URL — support both http:// and socks5:// formats.
            let proxy = reqwest::Proxy::all(proxy_str.as_str())
                .map_err(|e| AgentError::tool("ddgs", format!("proxy parse: {e}")))?;
            reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
                .timeout(std::time::Duration::from_secs(20))
                .proxy(proxy)
                .build()
                .map_err(|e| AgentError::tool("ddgs", format!("client build: {e}")))?
        } else {
            self.client.clone()
        };

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = client.get("https://html.duckduckgo.com/html/")
                .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .query(&[("q", query)])
                .send() => r,
        }.map_err(|e| AgentError::tool("ddgs", format!("HTTP: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let b = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("ddgs", format!("{status}: {b}")));
        }

        let html = response.text().await
            .map_err(|e| AgentError::tool("ddgs", format!("read body: {e}")))?;

        // Parse DDG HTML results: titles in result__a, snippets in result__snippet
        let mut out = String::from("## DuckDuckGo Search Results\n\n");
        let mut count = 0;

        // Extract result blocks
        let title_re = regex::Regex::new(r#"class="result__a"[^>]*>(.*?)</a>"#).unwrap();
        let snippet_re = regex::Regex::new(r#"class="result__snippet">(.*?)</a>"#).unwrap();
        let url_re = regex::Regex::new(r#"class="result__url"[^>]*>(.*?)</a>"#).unwrap();

        let titles: Vec<String> = title_re.captures_iter(&html)
            .map(|c| strip_html_tags(&c[1]))
            .collect();
        let snippets: Vec<String> = snippet_re.captures_iter(&html)
            .map(|c| strip_html_tags(&c[1]))
            .collect();
        let urls: Vec<String> = url_re.captures_iter(&html)
            .map(|c| strip_html_tags(&c[1]).trim().to_string())
            .collect();

        for (i, title) in titles.iter().enumerate().take(num as usize) {
            let snippet = snippets.get(i).map(|s| s.as_str()).unwrap_or("");
            let url = urls.get(i).map(|s| s.as_str()).unwrap_or("");
            count += 1;
            out.push_str(&format!("{count}. **{title}**\n   {url}\n   {snippet}\n\n"));
        }

        if count == 0 {
            out.push_str(&format!("No results for '{}'", query));
        }

        Ok(ToolOutput { content: out, metadata: None })
    }
}

/// Strip HTML tags and decode basic entities from a string.
fn strip_html_tags(s: &str) -> String {
    // Remove tags
    let no_tags: String = s.chars()
        .fold((String::new(), false), |(mut acc, in_tag), c| {
            if c == '<' { (acc, true) }
            else if c == '>' { (acc, false) }
            else if !in_tag { acc.push(c); (acc, false) }
            else { (acc, true) }
        }).0;
    // Decode common entities
    no_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
