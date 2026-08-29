use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Query OpenTargets Platform (GraphQL) for structured target↔disease
/// associations with score breakdowns.
///
/// Biomni-style evidence lookup: instead of answering "is gene X associated
/// with disease Y?" from parametric memory, the agent (debate stage, report
/// review, or a generated analysis) can pull the association score AND its
/// component breakdowns (genetic association / expression / known drug) with
/// source URLs, so every claim carries a traceable reference. No API key
/// needed.
pub struct OpenTargetsTool {
    client: reqwest::Client,
}

impl Default for OpenTargetsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenTargetsTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("miniagent/0.1")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    async fn graphql(&self, query: &str, variables: serde_json::Value, cancel: CancellationToken) -> Result<serde_json::Value, AgentError> {
        let body = json!({ "query": query, "variables": variables });
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client
                .post("https://api.platform.opentargets.org/api/v4/graphql")
                .json(&body)
                .send() => r,
        }
        .map_err(|e| AgentError::tool("opentargets", format!("HTTP: {e}")))?;
        if !resp.status().is_success() {
            let b = resp.text().await.unwrap_or_default();
            return Err(AgentError::tool("opentargets", format!("HTTP error: {b}")));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AgentError::tool("opentargets", format!("parse: {e}")))?;
        if let Some(errors) = v["errors"].as_array() {
            let msg = errors
                .iter()
                .filter_map(|e| e["message"].as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(AgentError::tool("opentargets", format!("graphql: {msg}")));
        }
        Ok(v["data"].clone())
    }
}

#[async_trait]
impl Tool for OpenTargetsTool {
    fn name(&self) -> &str {
        "opentargets"
    }

    fn description(&self) -> &str {
        "Query the OpenTargets Platform for structured target↔disease evidence. \
         Operations: search (resolve a disease/drug name to an EFO id), \
         associated_targets (top targets for a disease EFO id with score \
         breakdowns: geneticAssociation, differentialExpression, knownDrug), \
         target_disease (score details for one targetId↔diseaseId pair). \
         Use to verify gene-disease hypotheses against curated GWAS/omics \
         evidence instead of relying on memory. Output claims as \
         {claim, sources:[{db:'OpenTargets', id, url, field}], verdict}."
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": ["search", "associated_targets", "target_disease"],
                    "description": "Which query to run"},
                "query": {"type": "string", "description": "search: disease/drug name, e.g. 'alzheimer'"},
                "efo_id": {"type": "string", "description": "associated_targets: disease EFO id, e.g. 'EFO_0000249'"},
                "limit": {"type": "integer", "description": "associated_targets: max targets (default 20, max 50)"},
                "target_id": {"type": "string", "description": "target_disease: Ensembl gene id, e.g. 'ENSG00000137642'"},
                "disease_id": {"type": "string", "description": "target_disease: disease EFO id"}
            },
            "required": ["operation"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let op = input["operation"].as_str().unwrap_or("search");
        match op {
            "search" => {
                let q = input["query"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("opentargets", "search needs 'query'"))?;
                let gql = r#"query Search($q: String!) {
                    search(queryString: $q, entityNames: ["disease"], page: {size: 5, index: 0}) {
                        hits { id name entity }
                    }
                }"#;
                let data = self.graphql(gql, json!({ "q": q }), cancel).await?;
                let mut out = format!("OpenTargets disease search for '{q}':\n");
                if let Some(hits) = data["search"]["hits"].as_array() {
                    for h in hits {
                        out.push_str(&format!(
                            "- {} — {} (link: https://platform.opentargets.org/disease/{})\n",
                            h["id"].as_str().unwrap_or("?"),
                            h["name"].as_str().unwrap_or("?"),
                            h["id"].as_str().unwrap_or("?")
                        ));
                    }
                }
                Ok(ToolOutput { content: out, metadata: None })
            }
            "associated_targets" => {
                let efo = input["efo_id"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("opentargets", "associated_targets needs 'efo_id'"))?;
                let limit = input["limit"].as_u64().unwrap_or(20).clamp(1, 50);
                let gql = r#"query Assoc($efo: String!, $limit: Int!) {
                    disease(efoId: $efo) {
                        id name
                        associatedTargets(page: {size: $limit, index: 0}) {
                            rows {
                                target { id approvedSymbol }
                                score
                                datatypeScores { id score }
                            }
                        }
                    }
                }"#;
                let data = self
                    .graphql(gql, json!({ "efo": efo, "limit": limit }), cancel)
                    .await?;
                let disease = &data["disease"];
                let name = disease["name"].as_str().unwrap_or("?");
                let mut out = format!(
                    "Top targets for {name} ({efo}) — https://platform.opentargets.org/disease/{efo}\n\
                     symbol | overall | genetic | expression | knownDrug\n"
                );
                let rows = disease["associatedTargets"]["rows"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for r in &rows {
                    let mut comp = std::collections::HashMap::new();
                    for ds in r["datatypeScores"].as_array().unwrap_or(&vec![]) {
                        comp.insert(
                            ds["id"].as_str().unwrap_or("").to_string(),
                            ds["score"].as_f64().unwrap_or(0.0),
                        );
                    }
                    out.push_str(&format!(
                        "{} | {:.3} | {:.3} | {:.3} | {:.3}\n",
                        r["target"]["approvedSymbol"].as_str().unwrap_or("?"),
                        r["score"].as_f64().unwrap_or(0.0),
                        comp.get("genetic_association").copied().unwrap_or(0.0),
                        comp.get("rna_expression")
                            .or_else(|| comp.get("differential_expression"))
                            .copied()
                            .unwrap_or(0.0),
                        comp.get("known_drug").copied().unwrap_or(0.0),
                    ));
                }
                Ok(ToolOutput { content: out, metadata: None })
            }
            "target_disease" => {
                let target = input["target_id"].as_str().ok_or_else(|| {
                    AgentError::tool("opentargets", "target_disease needs 'target_id'")
                })?;
                let disease = input["disease_id"].as_str().ok_or_else(|| {
                    AgentError::tool("opentargets", "target_disease needs 'disease_id'")
                })?;
                let gql = r#"query Pair($t: String!, $d: String!) {
                    target(ensemblId: $t) { approvedSymbol }
                    disease(efoId: $d) { name
                        evidences(targetId: $t, enableIndirect: false, size: 3) { count }
                    }
                }"#;
                let data = self
                    .graphql(gql, json!({ "t": target, "d": disease }), cancel)
                    .await?;
                let symbol = data["target"]["approvedSymbol"].as_str().unwrap_or(target);
                let dname = data["disease"]["name"].as_str().unwrap_or(disease);
                let n = data["disease"]["evidences"]["count"].as_u64().unwrap_or(0);
                Ok(ToolOutput {
                    content: format!(
                        "{symbol} ↔ {dname}: {n} evidence rows — \
                         https://platform.opentargets.org/evidence/{disease}/{target}"
                    ),
                    metadata: None,
                })
            }
            other => Err(AgentError::tool(
                "opentargets",
                format!("unknown operation '{other}'"),
            )),
        }
    }
}
