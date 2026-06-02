use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use crate::roles::{
    AgentRole, Blackboard, FileContext, RoleOutput,
    persist_output, load_checkpoint, append_event, parse_llm_json,
};

/// Evidence Accumulator: tracks cumulative evidence scores per hypothesis,
/// identifies gaps, and highlights contradictions.
pub struct EvidenceAccumulatorRole {
    provider: Box<dyn LlmProvider>,
}

impl EvidenceAccumulatorRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct HypothesisEvidence {
    hypothesis_id: String,
    supporting: Vec<EvidenceRecord>,
    contradicting: Vec<EvidenceRecord>,
    evidence_score: f64,
    gaps: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct EvidenceRecord {
    claim: String,
    source: String,
    strength: f64,
    round: usize,
}

#[async_trait]
impl AgentRole for EvidenceAccumulatorRole {
    fn name(&self) -> &str { "evidence_accumulator" }
    fn description(&self) -> &str {
        "Evidence Accumulator. Tracks per-hypothesis evidence scores, identifies \
         gaps and contradictions, produces evidence summaries for tournament scoring."
    }

    async fn execute(
        &self,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        append_event(&blackboard.work_dir, "evidence_accumulator: analyzing evidence");

        // Collect all evidence from researcher and debate results
        let researcher_findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.json")
            .unwrap_or_default();
        let critic_notes = load_checkpoint(&blackboard.work_dir, "critic", "critique.json")
            .unwrap_or_default();
        let tournament_results = load_checkpoint(&blackboard.work_dir, "tournament_master", "round_report.json")
            .unwrap_or_default();

        let prompt = format!(
            r#"You are an evidence accumulator for scientific hypothesis evaluation.
Research topic: {task}

## Researcher Findings
{researcher_findings}

## Critic Notes
{critic_notes}

## Tournament Results
{tournament_results}

## Your Task
Analyze all available evidence and produce a structured evidence assessment.

## Output Format (JSON)
{{
  "hypotheses": [
    {{
      "hypothesis_id": "id",
      "evidence_score": 0.0-1.0,
      "supporting_count": 5,
      "contradicting_count": 2,
      "key_supporting": ["claim 1", "claim 2"],
      "key_contradicting": ["issue 1"],
      "gaps": ["what evidence is missing"],
      "strongest_source": "best piece of evidence"
    }}
  ],
  "overall_assessment": "summary of evidence landscape",
  "recommendations": ["what additional research is needed"]
}}
"#
        );

        let response = self.call_llm(
            "You are an evidence analyst. Be thorough and objective. Output valid JSON.",
            &prompt,
            cancel,
        ).await?;

        let parsed = match parse_llm_json(&response) {
            Ok(v) => v,
            Err(e) => return Ok(RoleOutput::failed(self.name(), &e)),
        };

        let content = serde_json::to_string_pretty(&parsed).unwrap_or_default();
        persist_output(&blackboard.work_dir, "evidence_accumulator", "evidence.json", &content);
        append_event(&blackboard.work_dir, "evidence_accumulator: evidence assessment persisted");

        let confidence = parsed["overall_assessment"].as_str().map(|_| 0.8).unwrap_or(0.5);

        Ok(RoleOutput {
            content,
            evidence: vec![],
            confidence,
            metadata: std::collections::HashMap::new(),
            output_files: vec!["evidence_accumulator/evidence.json".into()],
            status: "success".into(),
        })
    }
}

#[async_trait]
impl FileContext for EvidenceAccumulatorRole {
    fn workspace_name(&self) -> &str { "evidence_accumulator" }
}

impl EvidenceAccumulatorRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.1),
                max_tokens: Some(3000),
                ..Default::default()
            },
        };
        let resp = self.provider.complete(&request, cancel).await?;
        Ok(resp.content.iter()
            .filter_map(|b| match b { miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()), _ => None })
            .collect::<Vec<_>>().join(""))
    }
}
