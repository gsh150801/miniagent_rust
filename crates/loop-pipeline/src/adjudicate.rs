//! Three-way adjudication (advocate → challenger → arbiter).
//!
//! Debate-style completion review: one model role argues the work is done
//! (advocate), a second role actively looks for gaps, errors, and off-topic
//! content (challenger), and a third weighs both sides and rules. Used by
//! the loop pipeline's Evaluate stage and by the loop-orchestrated research
//! phases, so "task finished" is never a single model's opinion.
//!
//! Fully generic: the caller supplies the goal, a description of the work,
//! and the evidence (artifact listings / summaries). No domain constants.

use miniagent_core::config::InferenceConfig;
use miniagent_core::error::AgentError;
use miniagent_core::event::ContentBlock;
use miniagent_core::message::Message;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use miniagent_core::json_util::extract_and_repair;

/// Verdict of the arbiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjudicationVerdict {
    /// Work satisfies the goal; submit.
    Complete,
    /// Work has fixable gaps; run one repair round with the suggestions.
    NeedsRepair,
    /// Neither side convincing (evidence insufficient) — treat as
    /// needs-repair with the arbiter's notes.
    Unclear,
}

/// Structured result of one adjudication round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adjudication {
    pub verdict: AdjudicationVerdict,
    /// The advocate's case for completion.
    pub advocate: String,
    /// The challenger's case against completion.
    pub challenger: String,
    /// Concrete unmet items (from the challenger, endorsed by the arbiter).
    pub unmet: Vec<String>,
    /// Repair suggestions (empty when complete).
    pub suggestions: Vec<String>,
    pub summary: String,
}

/// Run one three-way adjudication.
///
/// `providers` are tried in order for each role (cross-family fallbacks); if
/// every provider fails, returns `Err` and the caller keeps its previous
/// decision path (adjudication is a quality gate, not the only signal).
pub async fn adjudicate(
    goal: &str,
    work_description: &str,
    evidence: &str,
    providers: &[std::sync::Arc<dyn LlmProvider>],
    cancel: CancellationToken,
) -> Result<Adjudication, AgentError> {
    if providers.is_empty() {
        return Err(AgentError::invalid_config(
            "adjudication requires at least one provider",
        ));
    }
    let advocate = one_role(
        providers,
        "You are the Advocate in a completion review. Argue, honestly and with \
         evidence from the material, why the work satisfies the goal.",
        &format!(
            "## Goal\n{goal}\n\n## Work performed\n{work_description}\n\n## Evidence\n{evidence}\n\n\
             Argue the case for completion. Output ONLY a short paragraph (<=120 words)."
        ),
        &cancel,
    )
    .await
    .ok_or_else(|| AgentError::internal("adjudication: advocate role unavailable"))?;
    let challenger = one_role(
        providers,
        "You are the Challenger in a completion review. Actively look for gaps, \
         errors, missing deliverables, off-topic content, and unsupported claims. \
         Do not accept surface plausibility.",
        &format!(
            "## Goal\n{goal}\n\n## Work performed\n{work_description}\n\n## Evidence\n{evidence}\n\n\
             List every concrete deficiency. Output ONLY valid JSON: \
             {{\"unmet\": [\"...\"], \"suggestions\": [\"...\"]}}"
        ),
        &cancel,
    )
    .await
    .unwrap_or_else(|| {
        serde_json::json!({ "unmet": [], "suggestions": [] }).to_string()
    });
    let (unmet, suggestions) = parse_list_json(&challenger);

    let arbiter_prompt = format!(
        "## Goal\n{goal}\n\n## Work performed\n{work_description}\n\n## Evidence\n{evidence}\n\n\
         ## Advocate's case\n{advocate}\n\n## Challenger's findings\nunmet: {unmet:?}\nsuggestions: {suggestions:?}\n\n\
         Rule on whether the work satisfies the goal. Output ONLY valid JSON: \
         {{\"verdict\": \"complete\"|\"needs_repair\"|\"unclear\", \"summary\": \"<one short paragraph>\"}}",
    );
    let arbiter_raw = one_role(
        providers,
        "You are the Arbiter in a completion review. Weigh the advocate's case \
         against the challenger's findings and rule strictly on the evidence.",
        &arbiter_prompt,
        &cancel,
    )
    .await
    .ok_or_else(|| AgentError::internal("adjudication: arbiter role unavailable"))?;
    let (verdict, summary) = parse_verdict(&arbiter_raw);

    Ok(Adjudication {
        verdict,
        advocate,
        challenger,
        unmet,
        suggestions,
        summary,
    })
}

/// One LLM role call with cross-provider fallback. Returns None when every
/// provider fails (caller decides how to degrade).
async fn one_role(
    providers: &[std::sync::Arc<dyn LlmProvider>],
    system: &str,
    prompt: &str,
    cancel: &CancellationToken,
) -> Option<String> {
    for provider in providers {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![Message::user(prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.2),
                max_tokens: Some(4_096),
                ..Default::default()
            },
        };
        if let Ok(resp) = provider.complete(&request, cancel.child_token()).await {
            let text: String = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let text = text.trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn parse_list_json(raw: &str) -> (Vec<String>, Vec<String>) {
    let repaired = extract_and_repair(raw);
    #[derive(serde::Deserialize)]
    struct Lists {
        #[serde(default)]
        unmet: Vec<String>,
        #[serde(default)]
        suggestions: Vec<String>,
    }
    match serde_json::from_str::<Lists>(&repaired) {
        Ok(l) => (l.unmet, l.suggestions),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

fn parse_verdict(raw: &str) -> (AdjudicationVerdict, String) {
    let repaired = extract_and_repair(raw);
    #[derive(serde::Deserialize)]
    struct V {
        #[serde(default)]
        verdict: String,
        #[serde(default)]
        summary: String,
    }
    match serde_json::from_str::<V>(&repaired) {
        Ok(v) => {
            let verdict = match v.verdict.as_str() {
                "complete" => AdjudicationVerdict::Complete,
                "needs_repair" => AdjudicationVerdict::NeedsRepair,
                _ => AdjudicationVerdict::Unclear,
            };
            (verdict, v.summary)
        }
        Err(_) => (AdjudicationVerdict::Unclear, raw.chars().take(200).collect()),
    }
}
