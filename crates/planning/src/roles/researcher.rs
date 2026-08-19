use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, EvidenceItem, FileContext,
            call_llm_with_tools, load_todo, append_event, parse_llm_json};

/// Researcher (研究者) — information retrieval, fact extraction, evidence collection.
///
/// Key design: all findings are persisted to `researcher/` directory.
/// Other roles read from filesystem, not from in-memory state.
/// This prevents output loss during context compression (Manus principle).
pub struct ResearcherRole {
    provider: Box<dyn LlmProvider>,
}

impl ResearcherRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for ResearcherRole {
    fn name(&self) -> &str { "researcher" }
    fn description(&self) -> &str {
        "Scientific researcher. Searches literature, extracts facts, cites specific sources (PMIDs). \
         All findings are persisted to researcher/ directory for other agents to read."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let prior_findings = blackboard.get("researcher/findings.json");
        let planner_steps = blackboard.get("planner/current_plan.json");

        let continuation = prior_findings.as_ref().map(|p| {
            format!("\n## Previous Findings (continue from here)\n{p}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Researcher** in a multi-agent system.

**Task:** {task}

## Todo (attention anchor)
{todo}
{plan_section}{continuation}

## Your Role
1. Search for relevant literature and data sources
2. Extract key facts with verifiable citations (PMIDs, DOIs, URLs)
3. Identify knowledge gaps and conflicting evidence
4. Rate confidence for each finding
5. Preserve ALL findings — do not summarize away details

## Output Format (JSON)
{{
  "findings": [
    {{
      "claim": "specific factual claim",
      "source": "PMID:12345 or DOI or URL",
      "confidence": 0.85,
      "methodology": "how this evidence was obtained",
      "limitations": "caveats about this finding"
    }}
  ],
  "summary": "2-3 sentence overview of findings",
  "gaps": ["knowledge gap 1", "knowledge gap 2"],
  "conflicting_evidence": [
    {{"claim_a": "...", "claim_b": "...", "resolution": "..." }}
  ],
  "search_queries_used": ["query1", "query2"]
}}
"#,
            plan_section = planner_steps.map(|p| format!("\n## Planner's Instructions\n{p}")).unwrap_or_default(),
        );

        let system = "You are a rigorous scientific researcher. Cite real sources with PMIDs/DOIs. \
                       Preserve ALL findings with full detail. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "researcher: starting literature search");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = call_llm_with_tools(
            blackboard.agent(), &*self.provider, &[],
            &system, &prompt, &blackboard.work_dir_str(), cancel,
        ).await?;
        let parsed = self.parse_response(&response);

        // Persist all findings to filesystem
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "findings": parsed.evidence.iter().map(|e| serde_json::json!({
                "claim": e.claim,
                "source": e.source,
                "confidence": e.strength,
            })).collect::<Vec<_>>(),
            "summary": parsed.content,
            "gaps": parsed.metadata.get("gaps"),
        })).unwrap_or_default();
        blackboard.put("researcher/findings.json", &json)?;

        // Also persist a human-readable markdown version
        let md = format!(
            "# Research Findings\n\n## Summary\n{}\n\n## Evidence\n{}\n\n## Gaps\n{}",
            parsed.content,
            parsed.evidence.iter().enumerate().map(|(i, e)| {
                format!("{}. [{}] {} (source: {})", i + 1, e.strength, e.claim, e.source)
            }).collect::<Vec<_>>().join("\n"),
            parsed.metadata.get("gaps").cloned().unwrap_or_default(),
        );
        blackboard.put("researcher/findings.md", &md)?;

        append_event(&blackboard.work_dir,
            &format!("researcher: {} findings persisted", parsed.evidence.len()));

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for ResearcherRole {
    fn workspace_name(&self) -> &str { "researcher" }
}

impl ResearcherRole {
    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let evidence = parsed["findings"].as_array().map(|arr| {
            arr.iter().map(|f| EvidenceItem {
                claim: f["claim"].as_str().unwrap_or("").to_string(),
                source: f["source"].as_str().unwrap_or("").to_string(),
                strength: f["confidence"].as_f64().unwrap_or(0.7),
                counter_evidence: vec![],
            }).collect()
        }).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        if let Some(gaps) = parsed["gaps"].as_array() {
            metadata.insert("gaps".into(),
                gaps.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "));
        }
        if let Some(queries) = parsed["search_queries_used"].as_array() {
            metadata.insert("search_queries".into(),
                queries.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "));
        }

        RoleOutput {
            content: parsed["summary"].as_str().unwrap_or("").to_string(),
            evidence,
            confidence: 0.8,
            metadata,
            output_files: vec!["researcher/findings.json".into(), "researcher/findings.md".into()],
            status: "success".into(),
        }
    }
}
