use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use crate::roles::{
    AgentRole, Blackboard, FileContext, RoleOutput,
    persist_output, load_checkpoint, append_event, parse_llm_json,
};

/// Principal Investigator: sets research direction, approves tournament decisions,
/// adjusts reward function weights, triggers new rounds.
pub struct PrincipalInvestigatorRole {
    provider: Box<dyn LlmProvider>,
}

impl PrincipalInvestigatorRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl AgentRole for PrincipalInvestigatorRole {
    fn name(&self) -> &str { "pi" }
    fn description(&self) -> &str {
        "Principal Investigator. Sets research direction, monitors tournament progress, \
         adjusts evaluation weights, approves checkpoints, decides when to synthesize."
    }

    async fn execute(
        &self,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        append_event(&blackboard.work_dir, "pi: reviewing tournament progress");

        let arena_json = blackboard.artifacts.get("tournament_arena")
            .cloned()
            .unwrap_or_default();
        let evidence_json = load_checkpoint(&blackboard.work_dir, "evidence_accumulator", "evidence.json")
            .unwrap_or_default();
        let observer_context = load_checkpoint(&blackboard.work_dir, "observer", "context.json")
            .unwrap_or_default();

        let prompt = format!(
            r#"You are the Principal Investigator overseeing a multi-hypothesis tournament on: {task}

## Current Tournament State
{arena_json}

## Evidence Summary
{evidence_json}

## Observer Context
{observer_context}

## Your Responsibilities
1. Evaluate overall progress toward answering the research question
2. Decide whether to continue the tournament or begin synthesis
3. Adjust rubric weights if certain dimensions need more emphasis
4. Identify any hypotheses that should be dropped or new ones seeded

## Output Format (JSON)
{{
  "decision": "continue_tournament" | "begin_synthesis" | "seed_new_hypotheses" | "adjust_weights",
  "reasoning": "why this decision",
  "rubric_adjustments": {{
    "evidence_support": 0.30,
    "mechanistic_plausibility": 0.25,
    "falsifiability": 0.20,
    "novelty": 0.15,
    "consistency": 0.10
  }},
  "new_hypotheses": ["optional new hypothesis to seed"],
  "drop_hypotheses": ["optional hypothesis ids to drop"],
  "confidence": 0.85
}}
"#
        );

        let response = self.call_llm("You are a senior PI. Be decisive.", &prompt, cancel).await?;
        let parsed = match parse_llm_json(&response) {
            Ok(v) => v,
            Err(e) => return Ok(RoleOutput::failed(self.name(), &e)),
        };

        let decision = parsed["decision"].as_str().unwrap_or("continue_tournament").to_string();
        let content = serde_json::to_string_pretty(&parsed).unwrap_or_default();

        persist_output(&blackboard.work_dir, "pi", "decision.json", &content);
        append_event(&blackboard.work_dir, &format!("pi: decision={decision}"));

        let confidence = parsed["confidence"].as_f64().unwrap_or(0.7);
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("decision".into(), decision);

        if let Some(adj) = parsed["rubric_adjustments"].as_object() {
            metadata.insert("rubric_adjustments".into(), serde_json::to_string(adj).unwrap_or_default());
        }

        Ok(RoleOutput {
            content,
            evidence: vec![],
            confidence,
            metadata,
            output_files: vec!["pi/decision.json".into()],
            status: "success".into(),
        })
    }
}

#[async_trait]
impl FileContext for PrincipalInvestigatorRole {
    fn workspace_name(&self) -> &str { "pi" }
}

impl PrincipalInvestigatorRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.2),
                max_tokens: Some(2000),
                ..Default::default()
            },
        };
        let resp = self.provider.complete(&request, cancel).await?;
        Ok(resp.content.iter()
            .filter_map(|b| match b { miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()), _ => None })
            .collect::<Vec<_>>().join(""))
    }
}
