use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Evaluator (评估者) — result scoring, iteration recommendations.
///
/// Inspired by Anthropic's Evaluator-Optimizer dual-loop pattern.
/// Scores the quality of completed work and decides whether to
/// iterate (send back to planner) or accept and finalize.
pub struct EvaluatorRole {
    provider: Box<dyn LlmProvider>,
}

impl EvaluatorRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for EvaluatorRole {
    fn name(&self) -> &str { "evaluator" }
    fn description(&self) -> &str {
        "Result evaluator. Scores completed work, decides whether to iterate or finalize. \
         Reads all outputs from filesystem, persists evaluation to evaluator/ directory."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let review = load_checkpoint(&blackboard.work_dir, "reviewer", "review.json")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "reviewer", "review.md"))
            .unwrap_or_default();
        let synthesis = load_checkpoint(&blackboard.work_dir, "synthesizer", "synthesis.json")
            .unwrap_or_default();
        let critique = load_checkpoint(&blackboard.work_dir, "critic", "critique.json")
            .unwrap_or_default();
        let report = load_checkpoint(&blackboard.work_dir, "writer", "draft.md")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "writer", "report.json"))
            .unwrap_or_default();
        let prior_eval = load_checkpoint(&blackboard.work_dir, "evaluator", "evaluation.json");

        let iteration_context = prior_eval.as_ref().map(|p| {
            format!("\n## Previous Evaluation (compare progress)\n{p}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Evaluator** in a multi-agent system.

**Original Task:** {task}

## Todo
{todo}

## Review Report
{review}

## Synthesis
{synthesis}

## Critic's Assessment
{critique}

## Final Report
{report}{iteration_context}

## Your Role
Score the overall quality and decide the next action:

1. **Quality Scoring** (0-1 for each dimension):
   - completeness: was the task fully addressed?
   - accuracy: are the findings correct and well-supported?
   - rigor: was the methodology sound?
   - clarity: is the output clear and well-organized?
   - novelty: does this provide new insights?

2. **Decision**: ACCEPT (done) or ITERATE (needs more work)

3. **If ITERATE**: specify exactly what needs to improve

## Output Format (JSON)
{{
  "scores": {{
    "completeness": 0.0-1.0,
    "accuracy": 0.0-1.0,
    "rigor": 0.0-1.0,
    "clarity": 0.0-1.0,
    "novelty": 0.0-1.0
  }},
  "weighted_score": 0.0-1.0,
  "decision": "ACCEPT|ITERATE",
  "improvement_targets": [
    {{
      "dimension": "which score to improve",
      "current": 0.5,
      "target": 0.8,
      "approach": "how to improve it",
      "responsible_agent": "which agent should work on this"
    }}
  ],
  "iteration_count": 1,
  "max_iterations_recommended": 3,
  "summary": "brief overall assessment"
}}
"#
        );

        let system = "You are a rigorous evaluator. Score fairly, decide whether the output \
                       meets quality standards. Be specific about what needs improvement. \
                       Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "evaluator: scoring results");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist evaluation
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "scores": parsed.metadata.get("scores"),
            "weighted_score": parsed.confidence,
            "decision": parsed.metadata.get("decision"),
            "improvement_targets": parsed.metadata.get("improvement_targets"),
            "iteration_count": parsed.metadata.get("iteration_count"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "evaluator", "evaluation.json", &json);

        let md = format!(
            "# Evaluation Report\n\n## Decision: {}\n## Score: {:.2}\n\n## Scores\n{}\n\n## Summary\n{}",
            parsed.metadata.get("decision").unwrap_or(&"UNKNOWN".into()),
            parsed.confidence,
            parsed.metadata.get("scores_text").cloned().unwrap_or_default(),
            parsed.content,
        );
        persist_output(&blackboard.work_dir, "evaluator", "evaluation.md", &md);

        // Update todo.md with decision
        if let Some(decision) = parsed.metadata.get("decision")
            && decision == "ITERATE" {
                let new_todo = format!(
                    "{todo}\n\n## ITERATION NEEDED\n{}\nTargets:\n{}",
                    parsed.content,
                    parsed.metadata.get("improvement_targets_text").cloned().unwrap_or_default(),
                );
                persist_output(&blackboard.work_dir, "", "todo.md", &new_todo);
            }

        append_event(&blackboard.work_dir,
            &format!("evaluator: decision={}", parsed.metadata.get("decision").unwrap_or(&"?".into())));

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for EvaluatorRole {
    fn workspace_name(&self) -> &str { "evaluator" }
}

impl EvaluatorRole {
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

    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let scores = parsed["scores"].as_object();
        let scores_text = scores.map(|s| {
            s.iter().map(|(k, v)| {
                format!("- {k}: {:.2}", v.as_f64().unwrap_or(0.0))
            }).collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let improvement_text = parsed["improvement_targets"].as_array().map(|arr| {
            arr.iter().filter_map(|t| {
                let dim = t["dimension"].as_str()?;
                let approach = t["approach"].as_str().unwrap_or("");
                let agent = t["responsible_agent"].as_str().unwrap_or("unknown");
                Some(format!("- [{agent}] {dim}: {approach}"))
            }).collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("scores".into(), serde_json::to_string(&parsed["scores"]).unwrap_or_default());
        metadata.insert("scores_text".into(), scores_text);
        if let Some(d) = parsed["decision"].as_str() {
            metadata.insert("decision".into(), d.into());
        }
        metadata.insert("improvement_targets".into(), serde_json::to_string(&parsed["improvement_targets"]).unwrap_or_default());
        metadata.insert("improvement_targets_text".into(), improvement_text);
        if let Some(n) = parsed["iteration_count"].as_u64() {
            metadata.insert("iteration_count".into(), n.to_string());
        }

        RoleOutput {
            content: parsed["summary"].as_str().unwrap_or("").to_string(),
            evidence: vec![],
            confidence: parsed["weighted_score"].as_f64().unwrap_or(0.5),
            metadata,
            output_files: vec!["evaluator/evaluation.json".into(), "evaluator/evaluation.md".into()],
            status: "success".into(),
        }
    }
}
