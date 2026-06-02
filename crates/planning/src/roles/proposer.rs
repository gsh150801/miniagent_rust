use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, EvidenceItem, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Proposer (正方) — generates hypotheses with supporting evidence.
/// In the debate triad, the Proposer makes the initial claim and
/// defends it against the Opponent's challenges.
pub struct ProposerRole {
    provider: Box<dyn LlmProvider>,
}

impl ProposerRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for ProposerRole {
    fn name(&self) -> &str { "proposer" }
    fn description(&self) -> &str {
        "Scientific hypothesis proposer. Generates testable hypotheses backed by evidence \
         from literature, data, or mechanistic reasoning. Must cite specific sources. \
         Persists to proposer/ directory."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let opponent_critique = load_checkpoint(&blackboard.work_dir, "opponent", "critique.json");
        let researcher_findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.json");

        let round_context = opponent_critique.as_ref().map(|critique| {
            format!(
                "\n\n## Opponent's Previous Critique\n{critique}\n\n\
                 Refine your hypothesis to address the opponent's concerns. \
                 Strengthen weak evidence. Concede invalid points honestly."
            )
        }).unwrap_or_default();

        let findings_section = researcher_findings.map(|f| {
            format!("\n## Available Research Findings\n{f}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are a scientific hypothesis proposer (正方). Your task: propose a well-supported hypothesis.

**Research Topic:** {task}

## Todo
{todo}{findings_section}{round_context}

## Your Role
1. State a clear, falsifiable hypothesis
2. Provide supporting evidence with specific citations (PMIDs, DOIs, datasets)
3. Explain the mechanistic rationale
4. Rate your confidence (0-1) honestly
5. Acknowledge limitations proactively

## Output Format (JSON)
{{
  "hypothesis": "clear statement",
  "mechanism": "proposed mechanism",
  "evidence": [
    {{"claim": "...", "source": "PMID:12345 or URL", "strength": 0.85}}
  ],
  "confidence": 0.75,
  "limitations": ["limitation 1", "limitation 2"],
  "testable_prediction": "if hypothesis is true, we expect X under condition Y"
}}
"#
        );

        let system = "You are a rigorous scientific proposer. Be honest about uncertainty. \
                       Cite real sources. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "proposer: generating hypothesis");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist to filesystem
        let output_json = serde_json::to_string_pretty(&serde_json::json!({
            "hypothesis": parsed.content,
            "evidence": parsed.evidence.iter().map(|e| serde_json::json!({
                "claim": e.claim, "source": e.source, "strength": e.strength,
            })).collect::<Vec<_>>(),
            "confidence": parsed.confidence,
            "mechanism": parsed.metadata.get("mechanism"),
            "testable_prediction": parsed.metadata.get("prediction"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "proposer", "hypothesis.json", &output_json);

        append_event(&blackboard.work_dir, "proposer: hypothesis persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for ProposerRole {
    fn workspace_name(&self) -> &str { "proposer" }
}

impl ProposerRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.2), max_tokens: Some(3000), ..Default::default()
            },
        };
        let resp = self.provider.complete(&request, cancel).await?;
        Ok(resp.content.iter()
            .filter_map(|b| match b { miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()), _ => None })
            .collect::<Vec<_>>().join(""))
    }

    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let mut metadata = std::collections::HashMap::new();
        if let Some(mech) = parsed["mechanism"].as_str() { metadata.insert("mechanism".into(), mech.into()); }
        if let Some(pred) = parsed["testable_prediction"].as_str() { metadata.insert("prediction".into(), pred.into()); }

        RoleOutput {
            content: parsed["hypothesis"].as_str().unwrap_or("").to_string(),
            evidence: parsed["evidence"].as_array().map(|a| a.iter().map(|e| EvidenceItem {
                claim: e["claim"].as_str().unwrap_or("").to_string(),
                source: e["source"].as_str().unwrap_or("").to_string(),
                strength: e["strength"].as_f64().unwrap_or(0.5),
                counter_evidence: vec![],
            }).collect()).unwrap_or_default(),
            confidence: parsed["confidence"].as_f64().unwrap_or(0.5),
            metadata,
            output_files: vec!["proposer/hypothesis.json".into()],
            status: "success".into(),
        }
    }
}
