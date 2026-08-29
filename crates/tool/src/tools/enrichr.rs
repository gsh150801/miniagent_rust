use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Run gene-set over-representation enrichment via the Enrichr REST API
/// (Ma'ayan Lab) — no API key required.
///
/// Biomni-style analysis tool: turns a gene list into GO / KEGG / MSigDB
/// Hallmark terms with adjusted p-values, directly usable as a deliverable
/// of a data-analysis task (or an inline check during debate/review) without
/// needing gseapy or an R environment.
pub struct EnrichrTool {
    client: reqwest::Client,
}

impl Default for EnrichrTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EnrichrTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("miniagent/0.1")
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}

#[async_trait]
impl Tool for EnrichrTool {
    fn name(&self) -> &str {
        "enrichr"
    }

    fn description(&self) -> &str {
        "Gene-set over-representation enrichment via Enrichr (GO Biological \
         Process / KEGG / MSigDB Hallmark). Input: a gene symbol list (HUGO, \
         newline or comma separated) plus optional background. Returns top \
         terms with overlap, adjusted p-value, and member genes. Use for \
         interpreting DE gene sets; cite results as \
         {claim, sources:[{db:'Enrichr', id, url, field}], verdict}."
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "genes": {"type": "array", "items": {"type": "string"},
                    "description": "Gene symbols (HUGO uppercase)"},
                "libraries": {"type": "array", "items": {"type": "string"},
                    "description": "Default: GO_Biological_Process_2023, KEGG_2021_Human, MSigDB_Hallmark_2020"},
                "top_terms": {"type": "integer",
                    "description": "Top terms per library to return (default 10, max 25)"}
            },
            "required": ["genes"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let genes: Vec<String> = input["genes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|g| g.as_str())
                    .map(|s| s.trim().to_uppercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if genes.len() < 3 {
            return Err(AgentError::tool(
                "enrichr",
                format!("need >=3 gene symbols, got {}", genes.len()),
            ));
        }
        let top = input["top_terms"].as_u64().unwrap_or(10).clamp(1, 25) as usize;
        let libraries: Vec<String> = input["libraries"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    "GO_Biological_Process_2023".into(),
                    "KEGG_2021_Human".into(),
                    "MSigDB_Hallmark_2020".into(),
                ]
            });

        // 1. submit the list (Enrichr expects multipart form, not urlencoded —
        // a urlencoded POST returns 400).
        let part_list = reqwest::multipart::Part::text(genes.join("\n"))
            .file_name("list")
            .mime_str("text/plain")
            .map_err(|e| AgentError::tool("enrichr", format!("multipart: {e}")))?;
        let part_desc = reqwest::multipart::Part::text("miniagent");
        let form = reqwest::multipart::Form::new()
            .part("list", part_list)
            .part("description", part_desc);
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client
                .post("https://maayanlab.cloud/Enrichr/addList")
                .multipart(form)
                .send() => r,
        }
        .map_err(|e| AgentError::tool("enrichr", format!("addList HTTP: {e}")))?;
        if !resp.status().is_success() {
            let b = resp.text().await.unwrap_or_default();
            return Err(AgentError::tool("enrichr", format!("addList: {b}")));
        }
        let added: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AgentError::tool("enrichr", format!("addList parse: {e}")))?;
        let list_id = added["userListId"]
            .as_i64()
            .map(|v| v.to_string())
            .or_else(|| added["userListId"].as_str().map(|s| s.to_string()))
            .ok_or_else(|| AgentError::tool("enrichr", "no userListId in response"))?;

        // 2. fetch enrichment per library
        let mut out = format!(
            "Enrichr ORA for {} genes (userListId {list_id}) — https://maayanlab.cloud/Enrichr/#enrich\nterm | library | overlap | adj_p | genes\n",
            genes.len()
        );
        let mut any = false;
        for lib in &libraries {
            let url = format!(
                "https://maayanlab.cloud/Enrichr/enrich?userListId={list_id}&backgroundType={lib}"
            );
            let resp = tokio::select! {
                _ = cancel.cancelled() => return Err(AgentError::Cancelled),
                r = self.client.get(&url).send() => r,
            };
            let Ok(resp) = resp else {
                continue;
            };
            let Ok(v) = resp.json::<serde_json::Value>().await else {
                continue;
            };
            let Some(rows) = v[lib.as_str()].as_array() else {
                continue;
            };
            for r in rows.iter().take(top) {
                // row = [rank, term, p, z, combined, genes, adj_p, ...]
                let term = r[1].as_str().unwrap_or("?");
                let genes_hit = r[5].as_array().cloned().unwrap_or_default();
                let overlap = genes_hit.len();
                let adj_p = r[6].as_f64().unwrap_or(1.0);
                if adj_p > 0.1 {
                    continue;
                }
                any = true;
                let member = genes_hit
                    .iter()
                    .filter_map(|g| g.as_str())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push_str(&format!(
                    "{term} | {lib} | {overlap} | {adj_p:.3e} | {member}\n"
                ));
            }
        }
        if !any {
            out.push_str("(no term with adj_p <= 0.1 — set too small or no coherent signal)\n");
        }
        Ok(ToolOutput { content: out, metadata: None })
    }
}
