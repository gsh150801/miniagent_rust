use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct PubMedTool {
    client: reqwest::Client,
}

impl Default for PubMedTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PubMedTool {
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
impl Tool for PubMedTool {
    fn name(&self) -> &str { "pubmed_search" }
    fn description(&self) -> &str {
        "Search PubMed for biomedical/ life sciences literature. \
         Returns article titles, PMIDs, publication years, and abstracts when available. \
         Use for biology, medicine, genetics, drug discovery, and related fields."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "PubMed search query (supports MeSH terms, boolean operators AND/OR/NOT)"},
                "max_results": {"type": "integer", "description": "Results per page (default: 50, max: 500)"},
                "offset": {"type": "integer", "description": "Starting offset for pagination (default: 0)"},
                "min_year": {"type": "string", "description": "Filter: minimum publication year (e.g. 2024)"}
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
            .ok_or_else(|| AgentError::tool("pubmed", "missing 'query'"))?;
        let max_results = input["max_results"].as_u64().unwrap_or(50).min(500);
        let offset = input["offset"].as_u64().unwrap_or(0);
        let min_year = input["min_year"].as_str();

        let pubmed_key = std::env::var("PUBMED_API_KEY").unwrap_or_default();
        let base_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";

        // Step 1: ESearch — find PMIDs with pagination support
        let mut esearch_url = format!(
            "{base_url}/esearch.fcgi?db=pubmed&retmode=json&retmax={max_results}&retstart={offset}&sort=relevance&term={}",
            urlencoding::encode(query)
        );
        if !pubmed_key.is_empty() {
            esearch_url.push_str(&format!("&api_key={pubmed_key}"));
        }
        if let Some(year) = min_year {
            esearch_url.push_str(&format!("&mindate={year}&datetype=pdat"));
        }

        let esearch_resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&esearch_url).send() => r,
        }.map_err(|e| AgentError::tool("pubmed", format!("esearch HTTP: {e}")))?;

        if !esearch_resp.status().is_success() {
            let b = esearch_resp.text().await.unwrap_or_default();
            return Err(AgentError::tool("pubmed", format!("esearch: {}", b)));
        }

        let esearch: serde_json::Value = esearch_resp.json().await
            .map_err(|e| AgentError::tool("pubmed", format!("esearch parse: {e}")))?;

        let ids: Vec<String> = esearch["esearchresult"]["idlist"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let total = esearch["esearchresult"]["count"]
            .as_str()
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap_or(0);

        if ids.is_empty() {
            return Ok(ToolOutput {
                content: format!("No PubMed results for '{}' (total: {})", query, total),
                metadata: None,
            });
        }

        // Step 2: ESummary — get article metadata
        let id_list = ids.join(",");
        let mut esummary_url = format!(
            "{base_url}/esummary.fcgi?db=pubmed&retmode=json&id={id_list}"
        );
        if !pubmed_key.is_empty() {
            esummary_url.push_str(&format!("&api_key={pubmed_key}"));
        }

        let esummary_resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&esummary_url).send() => r,
        }.map_err(|e| AgentError::tool("pubmed", format!("esummary HTTP: {e}")))?;

        let esummary: serde_json::Value = esummary_resp.json().await
            .map_err(|e| AgentError::tool("pubmed", format!("esummary parse: {e}")))?;

        let mut out = format!(
            "## PubMed Search: '{query}'\nTotal results: {total} | Showing: {}\n\n",
            ids.len()
        );

        for (i, pmid) in ids.iter().enumerate() {
            let article = &esummary["result"][pmid];
            let title = article["title"].as_str().unwrap_or("(no title)");
            let pubdate = article["pubdate"].as_str().unwrap_or("?");
            let source = article["source"].as_str().unwrap_or("");
            let authors = article["authors"].as_array().map(|a| {
                a.iter()
                    .filter_map(|au| au["name"].as_str())
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(", ")
            }).unwrap_or_default();
            let authors_display = if !authors.is_empty() {
                format!("{} et al.", authors)
            } else {
                String::new()
            };

            let doi = article["elocationid"].as_str()
                .filter(|d| d.starts_with("doi:"))
                .map(|d| d.trim_start_matches("doi: "));

            out.push_str(&format!(
                "{}. **{}**\n   PMID: {} | {}\n   {} — {}\n",
                i + 1,
                title,
                pmid,
                pubdate,
                authors_display,
                source,
            ));
            if let Some(doi) = doi {
                out.push_str(&format!("   https://doi.org/{doi}\n"));
            }
            out.push_str(&format!("   https://pubmed.ncbi.nlm.nih.gov/{pmid}/\n"));
            out.push('\n');
        }

        Ok(ToolOutput { content: out, metadata: None })
    }
}
