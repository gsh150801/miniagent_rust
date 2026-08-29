use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Query UniProtKB for protein function, subcellular location, domains and
/// PTMs via the public REST API — no API key required.
///
/// Mechanism self-consistency checks (debate + report review): does the
/// claimed "cytosolic RNA sensor" actually localize to the cytosol? Does the
/// protein carry the catalytic domain the hypothesis depends on? Every claim
/// gets a traceable uniprot.org URL.
pub struct UniprotTool {
    client: reqwest::Client,
}

impl Default for UniprotTool {
    fn default() -> Self {
        Self::new()
    }
}

impl UniprotTool {
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
impl Tool for UniprotTool {
    fn name(&self) -> &str {
        "uniprot"
    }

    fn description(&self) -> &str {
        "Look up a human protein in UniProtKB: function, subcellular location, \
         domains/sites, post-translational modifications, and interactive \
         pathways. Input a gene symbol (e.g. ZBP1, TREM2) or accession. Use to \
         check that a hypothesis' mechanism is consistent with curated \
         protein biology; cite as \
         {claim, sources:[{db:'UniProt', id, url, field}], verdict}."
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "gene": {"type": "string",
                    "description": "Gene symbol (HUGO, e.g. 'ZBP1') or UniProt accession ('Q9H171')"}
            },
            "required": ["gene"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let gene = input["gene"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AgentError::tool("uniprot", "missing 'gene'"))?;

        // Query human entries; accession queries hit directly, symbols go
        // through gene_exact.
        let is_accession = gene.len() == 6
            && gene.starts_with(|c: char| c.is_ascii_alphabetic() && c.is_uppercase())
            && gene[1..].chars().all(|c| c.is_ascii_digit());
        let query = if is_accession {
            format!("accession:{gene} AND organism_id:9606")
        } else {
            format!("gene_exact:{gene} AND organism_id:9606")
        };
        let url = format!(
            "https://rest.uniprot.org/uniprotkb/search?query={}&fields=accession,id,protein_name,gene_primary,function,cc_subcellular_location,cc_domain,cc_ptm&format=json&size=1",
            urlencoding::encode(&query)
        );
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&url).send() => r,
        }
        .map_err(|e| AgentError::tool("uniprot", format!("HTTP: {e}")))?;
        if !resp.status().is_success() {
            let b = resp.text().await.unwrap_or_default();
            return Err(AgentError::tool("uniprot", format!("HTTP error: {b}")));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AgentError::tool("uniprot", format!("parse: {e}")))?;
        let entry = v["results"]
            .as_array()
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if entry.is_null() {
            return Ok(ToolOutput {
                content: format!(
                    "UniProt: no human entry found for '{gene}' (verdict: not_found)"
                ),
                metadata: None,
            });
        }

        let accession = entry["primaryAccession"].as_str().unwrap_or("?");
        let base = format!("https://www.uniprot.org/uniprotkb/{accession}");
        let name = entry["proteinDescription"]["recommendedName"]["fullName"]["value"]
            .as_str()
            .or_else(|| {
                entry["proteinDescription"]["submissionNames"][0]["fullName"]["value"]
                    .as_str()
            })
            .unwrap_or("?");

        let grab_texts = |key: &str| -> Vec<String> {
            entry["comments"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|c| c["commentType"].as_str() == Some(key))
                        .filter_map(|c| c["texts"].as_array())
                        .flatten()
                        .filter_map(|t| t["value"].as_str())
                        .map(|s| s.to_string())
                        .take(2)
                        .collect()
                })
                .unwrap_or_default()
        };

        let mut out = format!(
            "UniProt {accession} — {name} (gene {gene})\n{base}\n"
        );
        for (label, key) in [
            ("FUNCTION", "FUNCTION"),
            ("SUBCELLULAR LOCATION", "SUBCELLULAR LOCATION"),
            ("DOMAIN", "DOMAIN"),
            ("PTM", "PTM"),
        ] {
            let texts = grab_texts(key);
            if !texts.is_empty() {
                out.push_str(&format!("\n{label}:\n"));
                for t in &texts {
                    out.push_str(&format!("  - {}\n", t.chars().take(400).collect::<String>()));
                }
            }
        }
        Ok(ToolOutput { content: out, metadata: None })
    }
}
