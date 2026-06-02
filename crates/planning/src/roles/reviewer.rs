use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Reviewer (审查者) — final quality check against standards.
///
/// Performs comprehensive quality review of the complete output pipeline.
/// Checks logical consistency, citation accuracy, reproducibility,
/// statistical rigor, and adherence to reporting standards.
pub struct ReviewerRole {
    provider: Box<dyn LlmProvider>,
}

impl ReviewerRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for ReviewerRole {
    fn name(&self) -> &str { "reviewer" }
    fn description(&self) -> &str {
        "Quality reviewer. Final check against reporting standards, reproducibility, \
         and scientific rigor. Reads all prior outputs, persists review to reviewer/."
    }

    async fn execute(
        &self, _task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.json")
            .unwrap_or_default();
        let critique = load_checkpoint(&blackboard.work_dir, "critic", "critique.json")
            .unwrap_or_default();
        let synthesis = load_checkpoint(&blackboard.work_dir, "synthesizer", "synthesis.json")
            .unwrap_or_default();
        let report = load_checkpoint(&blackboard.work_dir, "writer", "report.json")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "writer", "draft.md"))
            .unwrap_or_default();
        let executor_output = load_checkpoint(&blackboard.work_dir, "executor", "output.json");

        let executor_section = executor_output.map(|e| {
            format!("\n## Executor Results\n{e}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Reviewer** in a multi-agent system.

## Todo
{todo}

## Research Findings
{findings}

## Critique
{critique}

## Synthesis
{synthesis}

## Report
{report}{executor_section}

## Your Role
Perform a FINAL quality review covering:

1. **Logical Consistency**: Do conclusions follow from evidence?
2. **Citation Accuracy**: Are all claims properly sourced?
3. **Reproducibility**: Could another researcher replicate this?
4. **Statistical Rigor**: Are statistical claims justified?
5. **Completeness**: Are there unanswered questions?
6. **Formatting**: Is the report well-structured?
7. **Language**: Is the writing clear and appropriate?

## Output Format (JSON)
{{
  "passed": true/false,
  "overall_score": 0.0-1.0,
  "issues": [
    {{
      "category": "logical|citation|reproducibility|statistical|completeness|formatting|language",
      "severity": "critical|major|minor",
      "description": "what's wrong",
      "location": "where in the output",
      "fix": "how to fix it"
    }}
  ],
  "strengths": ["what was done well"],
  "recommendation": "publish|revise|reject",
  "revision_priority": ["most important fix first"]
}}
"#
        );

        let system = "You are a thorough scientific peer reviewer. Check every dimension \
                       of quality. Be fair but rigorous. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "reviewer: starting final quality review");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist review
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "passed": parsed.metadata.get("passed"),
            "overall_score": parsed.confidence,
            "issues": parsed.metadata.get("issues"),
            "recommendation": parsed.metadata.get("recommendation"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "reviewer", "review.json", &json);

        let md = format!(
            "# Review Report\n\n## Verdict: {}\n## Score: {:.2}\n## Passed: {}\n\n## Issues\n{}\n\n## Strengths\n{}",
            parsed.metadata.get("recommendation").unwrap_or(&"unknown".into()),
            parsed.confidence,
            parsed.metadata.get("passed").unwrap_or(&"false".into()),
            parsed.content,
            parsed.metadata.get("strengths").cloned().unwrap_or_default(),
        );
        persist_output(&blackboard.work_dir, "reviewer", "review.md", &md);

        append_event(&blackboard.work_dir, "reviewer: review persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for ReviewerRole {
    fn workspace_name(&self) -> &str { "reviewer" }
}

impl ReviewerRole {
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

        let issues_text = parsed["issues"].as_array().map(|arr| {
            arr.iter().filter_map(|i| {
                let severity = i["severity"].as_str().unwrap_or("unknown");
                let desc = i["description"].as_str()?;
                let fix = i["fix"].as_str().unwrap_or("");
                Some(format!("- [{severity}] {desc} → {fix}"))
            }).collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("passed".into(), parsed["passed"].as_bool().unwrap_or(false).to_string());
        metadata.insert("issues".into(), serde_json::to_string(&parsed["issues"]).unwrap_or_default());
        if let Some(r) = parsed["recommendation"].as_str() {
            metadata.insert("recommendation".into(), r.into());
        }
        if let Some(arr) = parsed["strengths"].as_array() {
            metadata.insert("strengths".into(),
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n"));
        }
        if let Some(arr) = parsed["revision_priority"].as_array() {
            metadata.insert("revision_priority".into(),
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "));
        }

        RoleOutput {
            content: issues_text,
            evidence: vec![],
            confidence: parsed["overall_score"].as_f64().unwrap_or(0.5),
            metadata,
            output_files: vec!["reviewer/review.json".into(), "reviewer/review.md".into()],
            status: "success".into(),
        }
    }
}
