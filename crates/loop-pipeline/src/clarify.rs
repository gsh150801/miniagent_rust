//! Clarify stage: optional task-requirement clarification via user questions.
//!
//! After Explore, the model decides whether the task description leaves
//! material ambiguity (goals, constraints, scope, success criteria). If so
//! and an interactive channel is wired, it asks the user up to a couple of
//! concrete questions (with suggested options) and merges the answers into
//! the working task description. Fully optional: CLI runs (no channel) and
//! silent/timeout channels simply skip clarification and proceed on the
//! stated assumptions — the pipeline records which assumptions it made.

use async_trait::async_trait;
use miniagent_core::config::InferenceConfig;
use miniagent_core::error::AgentError;
use miniagent_core::event::ContentBlock;
use miniagent_core::message::Message;
use miniagent_provider::traits::CompletionRequest;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use miniagent_core::json_util::extract_and_repair;

/// Interactive ask channel: question + suggested options → user answer.
/// The server wires this to the WS ask/reply protocol; CLI passes nothing.
pub type ClarifyHook = std::sync::Arc<
    dyn Fn(String, Vec<String>) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
        + Send
        + Sync,
>;

/// One asked-and-answered clarification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clarification {
    pub question: String,
    pub answer: String,
    /// True when the channel answered; false = timeout/silent (assumption noted).
    pub answered: bool,
}

#[derive(Debug, Deserialize)]
struct ClarifyPlan {
    #[serde(default)]
    need_clarification: bool,
    #[serde(default)]
    questions: Vec<ClarifyQuestion>,
}

#[derive(Debug, Deserialize)]
struct ClarifyQuestion {
    question: String,
    #[serde(default)]
    options: Vec<String>,
}

/// Decide ambiguities and ask the user through the wired channel.
pub struct ClarifyStage;

impl ClarifyStage {
    /// Max questions per clarify round — keeps the interaction lightweight.
    pub const MAX_QUESTIONS: usize = 3;
}

#[async_trait]
impl PipelineStage for ClarifyStage {
    fn name(&self) -> &str {
        "clarify"
    }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let Some(hook) = ctx.clarify_hook.clone() else {
            // No interactive channel (CLI, tests): skip silently.
            return Ok(StageOutput {
                updated_state: ctx.state.clone(),
                new_messages: vec![],
                summary: "Clarify skipped: no interactive channel".into(),
            });
        };
        if ctx.state.clarified {
            // Ask once per run — later loops refine via repair/evaluate.
            return Ok(StageOutput {
                updated_state: ctx.state.clone(),
                new_messages: vec![],
                summary: "Clarify skipped: already clarified".into(),
            });
        }

        let prompt = format!(
            r#"You are the Requirements Clarifier for a task about to be decomposed and executed by an agent team.

## Task (original request from the user)
{}

## What exploration found
{}

Decide whether the task has MATERIAL ambiguity that would change how it should be executed:
- goals or deliverables that could be interpreted in importantly different ways
- constraints (scope, quantity, time window, quality bar, audience) that are missing but matter
- success criteria the user could reasonably disagree about

Do NOT ask about trivia a sensible default covers. If the task is executable as stated, say so.
Output ONLY valid JSON:
{{"need_clarification": true|false,
  "questions": [{{"question": "<one concrete question>", "options": ["<suggested answer 1>", "<suggested answer 2>"]}}]}}"#,
            ctx.state.original_task,
            ctx.state
                .exploration_history
                .last()
                .map(|e| e.findings.join("; "))
                .unwrap_or_else(|| "(no exploration output)".into()),
        );

        let provider = ctx.agent.flash_provider();
        let request = CompletionRequest {
            system: "You are a precise requirements clarifier. Output ONLY valid JSON.".into(),
            messages: vec![Message::user(&prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(4_096),
                ..Default::default()
            },
        };
        let response = provider.complete(&request, cancel).await?;
        let text: String = response
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let plan: ClarifyPlan = match serde_json::from_str(&extract_and_repair(&text)) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "clarify plan parse failed — proceeding without clarification");
                return Ok(StageOutput {
                    updated_state: ctx.state.clone(),
                    new_messages: vec![],
                    summary: "Clarify skipped: parse failure".into(),
                });
            }
        };
        if !plan.need_clarification || plan.questions.is_empty() {
            let mut state = ctx.state.clone();
            state.clarified = true;
            return Ok(StageOutput {
                updated_state: state,
                new_messages: vec![],
                summary: "Clarify: task is executable as stated — no questions".into(),
            });
        }

        // Ask through the interactive channel. A dropped/timeout answer is
        // recorded as an assumption, never a failure.
        let mut state = ctx.state.clone();
        let mut summaries: Vec<String> = Vec::new();
        for q in plan.questions.iter().take(Self::MAX_QUESTIONS) {
            let answer = (hook)(q.question.clone(), q.options.clone()).await;
            let answered = !answer.trim().is_empty();
            if answered {
                summaries.push(format!("Q: {} → A: {}", q.question, answer));
                state
                    .current_task
                    .push_str(&format!("\n[已澄清] {} → {}", q.question, answer));
            } else {
                summaries.push(format!("Q: {} → (no answer; proceeding on stated assumptions)", q.question));
            }
            state.clarifications.push(Clarification {
                question: q.question.clone(),
                answer: answer.clone(),
                answered,
            });
        }
        state.clarified = true;
        let summary = format!(
            "Clarify: {} question(s) asked — {}",
            state.clarifications.len(),
            summaries.join(" | ")
        );
        Ok(StageOutput {
            updated_state: state,
            new_messages: vec![],
            summary,
        })
    }
}
