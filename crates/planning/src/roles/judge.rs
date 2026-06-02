use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, DecisionRecord, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Judge (裁判) — evaluates both sides and issues binding verdict.
/// ACCEPT (passes), REVISE (needs work), or REJECT (fatal flaws).
pub struct JudgeRole {
    provider: Box<dyn LlmProvider>,
}

impl JudgeRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for JudgeRole {
    fn name(&self) -> &str { "judge" }
    fn description(&self) -> &str {
        "Scientific debate judge (裁判). Evaluates proposer and opponent arguments, \
         weighs evidence independently, issues binding verdict. Persists to judge/ directory."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let hypothesis = load_checkpoint(&blackboard.work_dir, "proposer", "hypothesis.json")
            .unwrap_or_default();
        let critique = load_checkpoint(&blackboard.work_dir, "opponent", "critique.json")
            .unwrap_or_default();
        let scores = load_checkpoint(&blackboard.work_dir, "opponent", "scores.json")
            .unwrap_or_default();

        let prompt = format!(
            r#"You are a scientific debate judge (裁判). Issue a fair, evidence-based verdict.

**Research Topic:** {task}

## Todo
{todo}

## Proposer's Case (正方)
{hypothesis}

## Opponent's Challenge (反方)
{critique}

## Opponent's Scores
{scores}

## Your Role
Evaluate BOTH sides independently:
1. Which side has stronger evidence?
2. Are there fatal flaws, or can they be fixed?
3. Is this genuinely new?
4. Can the hypothesis be empirically tested?

## Verdict Options
- **ACCEPT**: Well-supported, passes scrutiny
- **REVISE**: Promising but needs improvements
- **REJECT**: Fatal flaws, cannot be fixed

## Output Format (JSON)
{{
  "verdict": "ACCEPT|REVISE|REJECT",
  "rationale": "detailed reasoning",
  "proposer_strength": 0.0-1.0,
  "opponent_strength": 0.0-1.0,
  "evidence_support": 0.0-1.0,
  "overall_confidence": 0.0-1.0,
  "accepted_claims": ["claims that survived"],
  "rejected_claims": ["claims that failed"],
  "required_revisions": ["revision 1", "revision 2"],
  "next_steps": "what should happen next"
}}
"#
        );

        let system = "You are an impartial scientific judge. Be fair, rigorous, evidence-based. \
                       Your verdict is binding. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "judge: evaluating debate");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist verdict
        let output_json = serde_json::to_string_pretty(&serde_json::json!({
            "verdict": parsed.metadata.get("verdict"),
            "rationale": parsed.content,
            "scores": {
                "proposer_strength": parsed.metadata.get("proposer_strength"),
                "opponent_strength": parsed.metadata.get("opponent_strength"),
                "evidence_support": parsed.metadata.get("evidence_support"),
            },
            "overall_confidence": parsed.confidence,
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "judge", "verdict.json", &output_json);

        // Record decision
        let decision = DecisionRecord {
            issuer: "judge".into(),
            decision: parsed.metadata.get("verdict").cloned().unwrap_or_default(),
            reasoning: parsed.content.clone(),
            timestamp: chrono::Utc::now(),
        };
        persist_output(&blackboard.work_dir, "judge", "decision.json",
            &serde_json::to_string_pretty(&decision).unwrap_or_default());

        append_event(&blackboard.work_dir,
            &format!("judge: verdict={}", parsed.metadata.get("verdict").unwrap_or(&"UNKNOWN".into())));

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for JudgeRole {
    fn workspace_name(&self) -> &str { "judge" }
}

impl JudgeRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.1), max_tokens: Some(4000), ..Default::default()
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
        for key in &["verdict", "next_steps"] {
            if let Some(v) = parsed[key].as_str() { metadata.insert(key.to_string(), v.to_string()); }
        }
        for key in &["proposer_strength", "opponent_strength", "evidence_support", "overall_confidence"] {
            if let Some(v) = parsed[key].as_f64() { metadata.insert(key.to_string(), v.to_string()); }
        }

        let rationale = parsed["rationale"].as_str().unwrap_or("");
        let accepted: Vec<&str> = parsed["accepted_claims"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
        let rejected: Vec<&str> = parsed["rejected_claims"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
        let revisions: Vec<&str> = parsed["required_revisions"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();

        let content = format!(
            "## Verdict: {}\n\n### Rationale\n{rationale}\n\n### Accepted Claims\n{}\n\n### Rejected Claims\n{}\n\n### Required Revisions\n{}",
            metadata.get("verdict").map(|s| s.as_str()).unwrap_or("UNKNOWN"),
            accepted.iter().enumerate().map(|(i, c)| format!("{}. {c}", i+1)).collect::<Vec<_>>().join("\n"),
            rejected.iter().enumerate().map(|(i, c)| format!("{}. {c}", i+1)).collect::<Vec<_>>().join("\n"),
            revisions.iter().enumerate().map(|(i, r)| format!("{}. {r}", i+1)).collect::<Vec<_>>().join("\n"),
        );

        RoleOutput {
            content,
            evidence: vec![],
            confidence: parsed["overall_confidence"].as_f64().unwrap_or(0.5),
            metadata,
            output_files: vec!["judge/verdict.json".into(), "judge/decision.json".into()],
            status: "success".into(),
        }
    }
}
