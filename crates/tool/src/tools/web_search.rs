use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

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
         'site:arxiv.org' for ArXiv."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
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

        // Try backends in order: Serper → Tavily → Bocha
        if let Some("tavily") = backend {
            return self.search_tavily(query, num, cancel).await;
        }
        if let Some("bocha") = backend {
            return self.search_bocha(query, num, cancel).await;
        }

        // Default: try Serper first, fallback to Tavily → Bocha on failure
        let serper_key = env_opt("SERPER_API_KEY").or_else(|| env_opt("SERPAPI_API_KEY"));
        let tavily_key = env_opt("TAVILY_API_KEY");
        let bocha_key = env_opt("BOCHA_API_KEY");

        if let Some(ref key) = serper_key {
            match self.search_serper(query, num, key, cancel.clone()).await {
                Ok(out) => return Ok(out),
                Err(e) => {
                    eprintln!("[web_search] Serper failed ({e}), falling back...");
                }
            }
        }
        if let Some(ref key) = tavily_key {
            match self.search_tavily_with_key(query, num, key, cancel.clone()).await {
                Ok(out) => return Ok(out),
                Err(e) => {
                    eprintln!("[web_search] Tavily failed ({e}), falling back...");
                }
            }
        }
        if bocha_key.is_some() {
            self.search_bocha(query, num, cancel).await
        } else if serper_key.is_some() || tavily_key.is_some() {
            // All keys existed but all failed
            Err(AgentError::tool("web_search", "All search backends (Serper, Tavily, Bocha) failed"))
        } else {
            Ok(ToolOutput {
                content: "No search API key configured. Set SERPER_API_KEY, TAVILY_API_KEY, or BOCHA_API_KEY.".into(),
                metadata: None,
            })
        }
    }
}

impl WebSearchTool {
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
            r = self.client.get("https://api.bochaai.com/v1/ai/search")
                .header("Authorization", format!("Bearer {key}"))
                .query(&[("query", query), ("count", &num.to_string())])
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
        if out.is_empty() || out == "## Bocha Search Results\n\n" {
            out.push_str(&format!("No results for '{}'", query));
        }
        Ok(ToolOutput { content: out, metadata: None })
    }
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
