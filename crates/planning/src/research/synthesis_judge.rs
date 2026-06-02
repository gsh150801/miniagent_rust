use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use crate::roles::{
    AgentRole, Blackboard, FileContext, RoleOutput,
    persist_output, load_checkpoint, append_event, parse_llm_json,
};

/// Synthesis Judge: produces the final integrated multi-mechanism hypothesis
/// by combining the top-rated hypotheses from the tournament.
pub struct SynthesisJudgeRole {
    provider: Box<dyn LlmProvider>,
}

impl SynthesisJudgeRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl AgentRole for SynthesisJudgeRole {
    fn name(&self) -> &str { "synthesis_judge" }
    fn description(&self) -> &str {
        "Synthesis Judge. Produces final integrated hypothesis by combining top-rated \
         tournament hypotheses. Resolves contradictions and identifies synergies."
    }

    async fn execute(
        &self,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        append_event(&blackboard.work_dir, "synthesis_judge: producing final synthesis");

        let standings = load_checkpoint(&blackboard.work_dir, "tournament_master", "final_standings.json")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "tournament_master", "round_report.json"))
            .unwrap_or_default();
        let evidence = load_checkpoint(&blackboard.work_dir, "evidence_accumulator", "evidence.json")
            .unwrap_or_default();
        let pi_decision = load_checkpoint(&blackboard.work_dir, "pi", "decision.json")
            .unwrap_or_default();
        let researcher_findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.json")
            .unwrap_or_default();

        let prompt = format!(
            r#"You are the Synthesis Judge. Produce a comprehensive, integrated hypothesis
that synthesizes the strongest elements from the tournament's top hypotheses.

## Research Topic
{task}

## Tournament Standings
{standings}

## Evidence Assessment
{evidence}

## PI Guidance
{pi_decision}

## Research Findings
{researcher_findings}

## Your Task
1. Identify the top 2-3 hypotheses with strongest evidence
2. Find complementary mechanisms (e.g., amyloid + tau + inflammation)
3. Resolve contradictions with evidence-based reasoning
4. Produce an integrated multi-mechanism hypothesis
5. Identify remaining open questions

## Output Format (JSON)
{{
  "integrated_hypothesis": "clear statement combining top mechanisms",
  "component_mechanisms": [
    {{
      "mechanism": "name",
      "source_hypothesis": "id",
      "evidence_strength": 0.0-1.0,
      "role": "primary/secondary/modulating"
    }}
  ],
  "mechanism_interactions": [
    {{
      "from": "mechanism_a",
      "to": "mechanism_b",
      "interaction": "synergistic/antagonistic/independent",
      "evidence": "supporting evidence"
    }}
  ],
  "resolved_contradictions": [
    {{
      "issue": "description",
      "resolution": "how resolved"
    }}
  ],
  "open_questions": ["remaining unknowns"],
  "testable_predictions": [
    "if the integrated hypothesis is correct, we predict X"
  ],
  "confidence": 0.0-1.0
}}
"#
        );

        let response = self.call_llm(
            "You are a senior synthesis expert. Integrate rigorously. Output valid JSON.",
            &prompt,
            cancel,
        ).await?;

        let parsed = match parse_llm_json(&response) {
            Ok(v) => v,
            Err(e) => return Ok(RoleOutput::failed(self.name(), &e)),
        };

        let content = serde_json::to_string_pretty(&parsed).unwrap_or_default();
        persist_output(&blackboard.work_dir, "synthesis_judge", "synthesis.json", &content);

        // Also write a human-readable markdown report
        let md = Self::json_to_markdown(&parsed);
        persist_output(&blackboard.work_dir, "synthesis_judge", "report.md", &md);

        append_event(&blackboard.work_dir, "synthesis_judge: synthesis complete");

        let confidence = parsed["confidence"].as_f64().unwrap_or(0.7);
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("hypothesis".into(), parsed["integrated_hypothesis"].as_str().unwrap_or("").to_string());

        Ok(RoleOutput {
            content,
            evidence: vec![],
            confidence,
            metadata,
            output_files: vec!["synthesis_judge/synthesis.json".into(), "synthesis_judge/report.md".into()],
            status: "success".into(),
        })
    }
}

#[async_trait]
impl FileContext for SynthesisJudgeRole {
    fn workspace_name(&self) -> &str { "synthesis_judge" }
}

impl SynthesisJudgeRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.2),
                max_tokens: Some(4000),
                ..Default::default()
            },
        };
        let resp = self.provider.complete(&request, cancel).await?;
        Ok(resp.content.iter()
            .filter_map(|b| match b { miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()), _ => None })
            .collect::<Vec<_>>().join(""))
    }

    fn json_to_markdown(parsed: &serde_json::Value) -> String {
        let mut md = String::new();
        md.push_str("# Integrated Hypothesis Synthesis\n\n");
        if let Some(h) = parsed["integrated_hypothesis"].as_str() {
            md.push_str(&format!("## Integrated Hypothesis\n{h}\n\n"));
        }
        if let Some(mechs) = parsed["component_mechanisms"].as_array() {
            md.push_str("## Component Mechanisms\n");
            for m in mechs {
                md.push_str(&format!(
                    "- **{}** ({}): strength {:.0}%\n",
                    m["mechanism"].as_str().unwrap_or("?"),
                    m["role"].as_str().unwrap_or("?"),
                    m["evidence_strength"].as_f64().unwrap_or(0.0) * 100.0,
                ));
            }
            md.push('\n');
        }
        if let Some(preds) = parsed["testable_predictions"].as_array() {
            md.push_str("## Testable Predictions\n");
            for p in preds {
                md.push_str(&format!("- {}\n", p.as_str().unwrap_or("?")));
            }
            md.push('\n');
        }
        if let Some(questions) = parsed["open_questions"].as_array() {
            md.push_str("## Open Questions\n");
            for q in questions {
                md.push_str(&format!("- {}\n", q.as_str().unwrap_or("?")));
            }
        }
        md
    }
}
