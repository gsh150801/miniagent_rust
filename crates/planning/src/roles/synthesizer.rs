use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Synthesizer (合成者) — multi-source integration, contradiction resolution.
///
/// Reads all prior outputs from filesystem, integrates findings and critique
/// into unified conclusions. Resolves contradictions and generates hypotheses.
pub struct SynthesizerRole {
    provider: Box<dyn LlmProvider>,
}

impl SynthesizerRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for SynthesizerRole {
    fn name(&self) -> &str { "synthesizer" }
    fn description(&self) -> &str {
        "Scientific synthesizer. Integrates findings from multiple sources, resolves contradictions, \
         generates unified conclusions. Reads from filesystem, persists to synthesizer/ directory."
    }

    async fn execute(
        &self, _task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.json")
            .unwrap_or_default();
        let critique = load_checkpoint(&blackboard.work_dir, "critic", "critique.json")
            .unwrap_or_default();
        let evaluator = load_checkpoint(&blackboard.work_dir, "evaluator", "evaluation.json");

        let evaluator_section = evaluator.map(|e| {
            format!("\n## Evaluator Assessment\n{e}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Synthesizer** in a multi-agent system.

## Todo
{todo}

## Research Findings
{findings}

## Critic's Evaluation
{critique}
{evaluator_section}

## Your Role
1. Integrate ALL findings into unified conclusions
2. Resolve contradictions with evidence-based reasoning
3. Generate testable hypotheses from the synthesis
4. Identify the strongest and weakest claims
5. Preserve all source citations

## Output Format (JSON)
{{
  "conclusions": [
    {{
      "statement": "clear conclusion",
      "confidence": 0.85,
      "supporting_evidence": ["PMID:xxx", "finding from Y"],
      "resolves_contradiction": true/false
    }}
  ],
  "contradictions": [
    {{
      "claim_a": "...",
      "claim_b": "...",
      "resolution": "evidence-based resolution",
      "confidence": 0.7
    }}
  ],
  "hypotheses": [
    {{
      "statement": "testable hypothesis",
      "mechanism": "proposed mechanism",
      "testable_prediction": "if true, we expect X",
      "confidence": 0.7
    }}
  ],
  "key_insight": "the single most important finding",
  "evidence_strength": "strong|moderate|weak"
}}
"#
        );

        let system = "You are a scientific synthesizer. Integrate all evidence, \
                       resolve conflicts, generate hypotheses. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "synthesizer: integrating findings and critique");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist synthesis
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "conclusions": parsed.metadata.get("conclusions"),
            "contradictions": parsed.metadata.get("contradictions"),
            "hypotheses": parsed.metadata.get("hypotheses"),
            "key_insight": parsed.metadata.get("key_insight"),
            "evidence_strength": parsed.metadata.get("evidence_strength"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "synthesizer", "synthesis.json", &json);

        let md = format!(
            "# Synthesis\n\n## Key Insight\n{}\n\n## Conclusions\n{}\n\n## Hypotheses\n{}",
            parsed.metadata.get("key_insight").unwrap_or(&"N/A".into()),
            parsed.content,
            parsed.metadata.get("hypotheses").cloned().unwrap_or_default(),
        );
        persist_output(&blackboard.work_dir, "synthesizer", "synthesis.md", &md);

        append_event(&blackboard.work_dir, "synthesizer: synthesis persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for SynthesizerRole {
    fn workspace_name(&self) -> &str { "synthesizer" }
}

impl SynthesizerRole {
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

    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let conclusions_text = parsed["conclusions"].as_array().map(|arr| {
            arr.iter().filter_map(|c| {
                let stmt = c["statement"].as_str()?;
                let conf = c["confidence"].as_f64().unwrap_or(0.5);
                Some(format!("- [{conf:.2}] {stmt}"))
            }).collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("conclusions".into(), serde_json::to_string(&parsed["conclusions"]).unwrap_or_default());
        metadata.insert("contradictions".into(), serde_json::to_string(&parsed["contradictions"]).unwrap_or_default());
        metadata.insert("hypotheses".into(), serde_json::to_string(&parsed["hypotheses"]).unwrap_or_default());
        if let Some(ki) = parsed["key_insight"].as_str() {
            metadata.insert("key_insight".into(), ki.into());
        }
        if let Some(es) = parsed["evidence_strength"].as_str() {
            metadata.insert("evidence_strength".into(), es.into());
        }

        RoleOutput {
            content: conclusions_text,
            evidence: vec![],
            confidence: 0.8,
            metadata,
            output_files: vec!["synthesizer/synthesis.json".into(), "synthesizer/synthesis.md".into()],
            status: "success".into(),
        }
    }
}
