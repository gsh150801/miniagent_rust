use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Search NCBI GEO (Gene Expression Omnibus) DataSets for public datasets.
///
/// Lets the agent locate real datasets (by accession, e.g. `GSE12345`) for the
/// data-analysis tasks in a validation plan. Queries the GEO DataSets (`gds`)
/// database via ESearch + ESummary.
pub struct GeoSearchTool {
    client: reqwest::Client,
}

impl Default for GeoSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GeoSearchTool {
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
impl Tool for GeoSearchTool {
    fn name(&self) -> &str {
        "geo_search"
    }

    fn description(&self) -> &str {
        "Search NCBI GEO (Gene Expression Omnibus) for public genomic datasets. \
         Returns dataset accessions (GSE...), titles, study types, sample counts, \
         and organism. Use to find real datasets (expression profiling, RNA-seq, \
         methylation, etc.) for computational validation analyses. \
         Translate non-English queries to English first."
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "ENGLISH GEO DataSet query, e.g. 'BRCA1 breast cancer expression profiling' or 'ovarian cancer RNA-seq'."},
                "max_results": {"type": "integer", "description": "Max datasets to return (default 5, max 20)"}
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
        let query = input["query"]
            .as_str()
            .ok_or_else(|| AgentError::tool("geo_search", "missing 'query'"))?;
        let max_results = input["max_results"].as_u64().unwrap_or(5).clamp(1, 20);

        let api_key = std::env::var("PUBMED_API_KEY").unwrap_or_default();
        let base_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";

        // ESearch against the GEO DataSets (gds) database.
        let mut esearch_url = format!(
            "{base_url}/esearch.fcgi?db=gds&retmode=json&retmax={max_results}&term={}",
            urlencoding::encode(query)
        );
        if !api_key.is_empty() {
            esearch_url.push_str(&format!("&api_key={api_key}"));
        }

        let esearch_resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&esearch_url).send() => r,
        }
        .map_err(|e| AgentError::tool("geo_search", format!("esearch HTTP: {e}")))?;

        if !esearch_resp.status().is_success() {
            let b = esearch_resp.text().await.unwrap_or_default();
            return Err(AgentError::tool("geo_search", format!("esearch: {b}")));
        }

        let esearch: serde_json::Value = esearch_resp
            .json()
            .await
            .map_err(|e| AgentError::tool("geo_search", format!("esearch parse: {e}")))?;

        let ids: Vec<String> = esearch["esearchresult"]["idlist"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let total = esearch["esearchresult"]["count"]
            .as_str()
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap_or(0);

        if ids.is_empty() {
            return Ok(ToolOutput {
                content: format!("No GEO datasets found for '{query}' (total: {total})"),
                metadata: None,
            });
        }

        // ESummary for dataset metadata.
        let id_list = ids.join(",");
        let mut esummary_url =
            format!("{base_url}/esummary.fcgi?db=gds&retmode=json&id={id_list}");
        if !api_key.is_empty() {
            esummary_url.push_str(&format!("&api_key={api_key}"));
        }

        let esummary_resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&esummary_url).send() => r,
        }
        .map_err(|e| AgentError::tool("geo_search", format!("esummary HTTP: {e}")))?;

        let esummary: serde_json::Value = esummary_resp
            .json()
            .await
            .map_err(|e| AgentError::tool("geo_search", format!("esummary parse: {e}")))?;

        let mut out = format!(
            "## GEO DataSet Search: '{query}'\nTotal: {total} | Showing: {}\n\n",
            ids.len()
        );

        for (i, id) in ids.iter().enumerate() {
            let entry = &esummary["result"][id];
            let accession = entry["accession"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("GDS{}", id));
            let title = entry["title"].as_str().unwrap_or("(no title)");
            let entry_type = entry["entryType"].as_str().unwrap_or("");
            let gds_type = entry["gdsType"].as_str().unwrap_or("");
            let n_samples = entry["n_samples"].as_u64().or_else(|| {
                entry["n_samples"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
            });
            let taxa = entry["platform_taxa"].as_str().unwrap_or("");
            let summary = entry["summary"].as_str().unwrap_or("");

            out.push_str(&format!(
                "{}. **{}** — {}\n",
                i + 1,
                accession,
                title
            ));
            if !entry_type.is_empty() {
                out.push_str(&format!("   Type: {} | {}\n", entry_type, gds_type));
            } else if !gds_type.is_empty() {
                out.push_str(&format!("   Type: {}\n", gds_type));
            }
            if let Some(n) = n_samples {
                out.push_str(&format!("   Samples: {n}"));
                if !taxa.is_empty() {
                    out.push_str(&format!(" | Organism: {taxa}"));
                }
                out.push('\n');
            } else if !taxa.is_empty() {
                out.push_str(&format!("   Organism: {taxa}\n"));
            }
            out.push_str(&format!(
                "   https://www.ncbi.nlm.nih.gov/geo/query/acc.cgi?acc={accession}\n"
            ));
            if !summary.is_empty() {
                let trimmed = if summary.len() > 280 {
                    format!("{}...", &summary[..280])
                } else {
                    summary.to_string()
                };
                out.push_str(&format!("   Summary: {trimmed}\n"));
            }
            out.push('\n');
        }

        Ok(ToolOutput {
            content: out,
            metadata: None,
        })
    }
}
