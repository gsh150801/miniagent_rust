use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Critic (批评者) — evaluates quality, identifies flaws, proposes improvements.
///
/// Inspired by Anthropic's Evaluator-Optimizer pattern.
/// Reads findings from filesystem (not in-memory state) to survive context compression.
pub struct CriticRole {
    provider: Box<dyn LlmProvider>,
}

impl CriticRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for CriticRole {
    fn name(&self) -> &str { "critic" }
    fn description(&self) -> &str {
        "Scientific critic. Evaluates claims for methodological soundness, statistical validity, \
         and logical consistency. Reads from filesystem, persists to critic/ directory."
    }

    async fn execute(
        &self, _task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.json")
            .unwrap_or_default();
        let findings_md = load_checkpoint(&blackboard.work_dir, "researcher", "findings.md")
            .unwrap_or_default();
        let prior_critique = load_checkpoint(&blackboard.work_dir, "critic", "critique.json");

        let prior_section = prior_critique.as_ref().map(|p| {
            format!("\n## Previous Critique (update if new findings exist)\n{p}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Critic** in a multi-agent system.

## Todo
{todo}

## Research Findings to Evaluate
{findings_md}

## Raw Findings Data
{findings}
{prior_section}

## Your Role
Critically evaluate EVERY claim. For each:
1. **Methodology**: Was the evidence gathered rigorously?
2. **Statistical Validity**: Are the effect sizes meaningful? Sample sizes adequate?
3. **Logical Consistency**: Do the conclusions follow from the evidence?
4. **Source Quality**: Are the citations credible? Peer-reviewed?
5. **Reproducibility**: Could another researcher replicate these findings?

## Output Format (JSON)
{{
  "flaws": [
    {{
      "claim_evaluated": "the claim being criticized",
      "issue": "specific flaw found",
      "severity": 0.8,
      "fix": "how to address this flaw"
    }}
  ],
  "strengths": ["what was done well"],
  "overall_score": 0.0-1.0,
  "recommendation": "accept|revise|reject",
  "priority_revisions": ["most important fix 1", "fix 2"]
}}
"#
        );

        let system = "You are a rigorous scientific critic. Be specific about flaws, \
                       propose concrete fixes. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "critic: evaluating research findings");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist critique
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "flaws": parsed.metadata.get("flaws"),
            "strengths": parsed.metadata.get("strengths"),
            "overall_score": parsed.confidence,
            "recommendation": parsed.metadata.get("recommendation"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "critic", "critique.json", &json);

        let md = format!(
            "# Critique\n\n## Overall Score: {:.2}\n## Recommendation: {}\n\n## Flaws\n{}\n\n## Strengths\n{}",
            parsed.confidence,
            parsed.metadata.get("recommendation").unwrap_or(&"unknown".into()),
            parsed.content,
            parsed.metadata.get("strengths").cloned().unwrap_or_default(),
        );
        persist_output(&blackboard.work_dir, "critic", "critique.md", &md);

        append_event(&blackboard.work_dir, "critic: critique persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for CriticRole {
    fn workspace_name(&self) -> &str { "critic" }
}

impl CriticRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.2),
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

        let flaws_text = parsed["flaws"].as_array().map(|arr| {
            arr.iter().filter_map(|f| {
                let severity = f["severity"].as_f64().unwrap_or(0.5);
                let issue = f["issue"].as_str()?;
                let fix = f["fix"].as_str().unwrap_or("no fix proposed");
                Some(format!("- [{severity:.1}] {issue} → Fix: {fix}"))
            }).collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("flaws".into(), serde_json::to_string(&parsed["flaws"]).unwrap_or_default());
        if let Some(arr) = parsed["strengths"].as_array() {
            metadata.insert("strengths".into(),
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n"));
        }
        if let Some(r) = parsed["recommendation"].as_str() {
            metadata.insert("recommendation".into(), r.into());
        }
        if let Some(arr) = parsed["priority_revisions"].as_array() {
            metadata.insert("priority_revisions".into(),
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "));
        }

        RoleOutput {
            content: flaws_text,
            evidence: vec![],
            confidence: parsed["overall_score"].as_f64().unwrap_or(0.5),
            metadata,
            output_files: vec!["critic/critique.json".into(), "critic/critique.md".into()],
            status: "success".into(),
        }
    }
}
