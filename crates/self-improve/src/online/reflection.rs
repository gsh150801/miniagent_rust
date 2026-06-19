use miniagent_core::message::Message;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use crate::integrator::AgentDelta;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Step-level reflection: diagnose each step right after execution.
/// Inspired by Agent-R and GUI-Reflection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReflection {
    pub step_id: uuid::Uuid,
    pub self_score: f64,
    pub error_detected: bool,
    pub error_root_cause: Option<String>,
    pub should_retry: bool,
    pub retry_suggestion: Option<String>,
    pub quality_tags: Vec<String>,
}

pub struct StepReflector {
    flash_provider: Option<Box<dyn LlmProvider>>,
    score_threshold: f64,
}

impl StepReflector {
    pub fn new() -> Self {
        Self {
            flash_provider: None,
            score_threshold: 0.5,
        }
    }

    pub fn with_provider(mut self, provider: Box<dyn LlmProvider>) -> Self {
        self.flash_provider = Some(provider);
        self
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.score_threshold = threshold;
        self
    }

    /// Reflect on a single agent step
    pub async fn reflect(
        &self,
        history: &[Message],
        delta: &AgentDelta,
        cancel: CancellationToken,
    ) -> StepReflection {
        // Fast heuristic evaluation (no LLM call needed for basic cases)
        let heuristic_score = self.heuristic_eval(history, delta);

        if heuristic_score > self.score_threshold {
            return StepReflection {
                step_id: uuid::Uuid::new_v4(),
                self_score: heuristic_score,
                error_detected: false,
                error_root_cause: None,
                should_retry: false,
                retry_suggestion: None,
                quality_tags: vec![],
            };
        }

        // Low heuristic score — use LLM for deeper reflection
        if let Some(ref provider) = self.flash_provider {
            self.llm_reflection(provider.as_ref(), history, delta, cancel).await
        } else {
            StepReflection {
                step_id: uuid::Uuid::new_v4(),
                self_score: heuristic_score,
                error_detected: heuristic_score < 0.3,
                error_root_cause: if heuristic_score < 0.3 {
                    Some("Low confidence (heuristic)".into())
                } else {
                    None
                },
                should_retry: heuristic_score < 0.3,
                retry_suggestion: None,
                quality_tags: vec![],
            }
        }
    }

    fn heuristic_eval(&self, history: &[Message], delta: &AgentDelta) -> f64 {
        let mut score: f64 = 0.7; // baseline

        // Check for empty responses
        if delta.new_messages.is_empty()
            || delta.new_messages.iter().all(|m| m.text_content().trim().is_empty())
        {
            score -= 0.4;
        }

        // Check for very short responses (potential truncation)
        let total_len: usize = delta
            .new_messages
            .iter()
            .map(|m| m.text_content().len())
            .sum();
        if total_len < 20 && !delta.new_messages.is_empty() {
            score -= 0.2;
        }

        // Check for repetition (last 3 messages)
        if history.len() >= 2 {
            let last_content = delta.new_messages.last().map(|m| m.text_content()).unwrap_or_default();
            let prev_content = history.last().map(|m| m.text_content()).unwrap_or_default();
            if last_content == prev_content {
                score -= 0.5; // exact repetition
            }
        }

        score.clamp(0.0, 1.0)
    }

    async fn llm_reflection(
        &self,
        provider: &dyn LlmProvider,
        history: &[Message],
        delta: &AgentDelta,
        cancel: CancellationToken,
    ) -> StepReflection {
        let context = history
            .iter()
            .rev()
            .take(2)
            .map(|m| format!("[{}]: {}", format!("{:?}", m.role).to_lowercase(), m.text_content()))
            .collect::<Vec<_>>()
            .join("\n");

        let response_text = delta
            .new_messages
            .iter()
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Rate the quality of this AI response on a scale of 0.0-1.0.\n\n\
             Context:\n{context}\n\n\
             Response:\n{response_text}\n\n\
             Output JSON: {{\"score\": 0.0-1.0, \"error\": true/false, \"reason\": \"...\", \"retry\": true/false, \"suggestion\": \"...\"}}"
        );

        let request = CompletionRequest {
            system: "You are a quality evaluator. Output ONLY valid JSON.".into(),
            messages: vec![Message::user(&prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(200),
                ..Default::default()
            },
        };

        match provider.complete(&request, cancel).await {
            Ok(resp) => {
                let text = resp.content.iter()
                    .filter_map(|b| match b {
                        miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>().join("");

                let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                StepReflection {
                    step_id: uuid::Uuid::new_v4(),
                    self_score: parsed["score"].as_f64().unwrap_or(0.5),
                    error_detected: parsed["error"].as_bool().unwrap_or(false),
                    error_root_cause: parsed["reason"].as_str().map(|s| s.to_string()),
                    should_retry: parsed["retry"].as_bool().unwrap_or(false),
                    retry_suggestion: parsed["suggestion"].as_str().map(|s| s.to_string()),
                    quality_tags: vec![],
                }
            }
            Err(_) => StepReflection {
                step_id: uuid::Uuid::new_v4(),
                self_score: 0.5,
                error_detected: false,
                error_root_cause: None,
                should_retry: false,
                retry_suggestion: None,
                quality_tags: vec![],
            },
        }
    }
}

impl Default for StepReflector {
    fn default() -> Self {
        Self::new()
    }
}
