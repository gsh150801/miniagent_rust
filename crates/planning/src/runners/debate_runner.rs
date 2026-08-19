//! `DebateRunner` — exposes the Proposer/Opponent/Judge triad through the
//! unified [`StageDriver`] trait.
//!
//! Re-implements the loop that the CLI's `debate_command` currently inlines:
//!   1. Proposer generates a hypothesis (writes `proposer/hypothesis.json`).
//!   2. Opponent critiques it (`opponent/critique.json`).
//!   3. Judge returns ACCEPT / REJECT / REVISE (`judge/verdict.json`).
//!   4. If REVISE, Proposer re-runs with the critique appended.
//!
//! Each role shares a single `Blackboard` so artifacts flow between rounds.

use crate::roles::{AgentRole, Blackboard, JudgeRole, OpponentRole, ProposerRole, RoleOutput};
use miniagent_core::orchestration::{OrchestrationError, SideEffect, StageDriver, StageInput, StageOutcome};
use miniagent_provider::traits::LlmProvider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One round of the debate, useful for callers that want to inspect outcomes
/// incrementally (e.g. streaming UIs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateRound {
    pub round: usize,
    pub proposer: RoleOutput,
    pub opponent: RoleOutput,
    pub judge: RoleOutput,
    pub verdict: String,
}

/// Adapter that drives the Proposer/Opponent/Judge loop and exposes it
/// through [`StageDriver`].
pub struct DebateRunner {
    proposer: ProposerRole,
    opponent: OpponentRole,
    judge: JudgeRole,
    work_dir: PathBuf,
    max_revise_rounds: usize,
}

impl DebateRunner {
    pub fn new(
        proposer_provider: Box<dyn LlmProvider>,
        opponent_provider: Box<dyn LlmProvider>,
        judge_provider: Box<dyn LlmProvider>,
        work_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            proposer: ProposerRole::new(proposer_provider),
            opponent: OpponentRole::new(opponent_provider),
            judge: JudgeRole::new(judge_provider),
            work_dir: work_dir.into(),
            max_revise_rounds: 2,
        }
    }

    pub fn with_max_revise_rounds(mut self, n: usize) -> Self {
        self.max_revise_rounds = n;
        self
    }

    fn extract_query(input: &StageInput) -> Result<String, OrchestrationError> {
        if let Some(s) = input.input.as_str() {
            return Ok(s.to_string());
        }
        if let Some(s) = input.input.get("prompt").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }
        if let Some(s) = input.input.get("query").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }
        Err(OrchestrationError::Plan(
            "DebateRunner input must be a string or {\"prompt\":...}/{\"query\":...}".into(),
        ))
    }

    /// Extract the `VERDICT:` line (or fall back to the metadata `status`).
    fn extract_verdict(judge_output: &RoleOutput) -> String {
        if let Some(v) = judge_output.metadata.get("verdict") {
            return v.clone();
        }
        if judge_output.content.to_uppercase().contains("ACCEPT") {
            "ACCEPT".into()
        } else if judge_output.content.to_uppercase().contains("REJECT") {
            "REJECT".into()
        } else if judge_output.content.to_uppercase().contains("REVISE") {
            "REVISE".into()
        } else {
            judge_output.status.clone()
        }
    }

    fn map_role_err(role: &str, e: miniagent_core::error::AgentError) -> OrchestrationError {
        match e {
            miniagent_core::error::AgentError::Cancelled => OrchestrationError::Cancelled,
            other => OrchestrationError::Stage(format!("{role} failed: {other}")),
        }
    }
}

#[async_trait::async_trait]
impl StageDriver for DebateRunner {
    fn name(&self) -> &str {
        "planning::DebateRunner"
    }

    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError> {
        let query = Self::extract_query(&input)?;
        let cancel = input.cancel.clone();
        let mut blackboard = Blackboard::new(&self.work_dir);

        let mut rounds: Vec<DebateRound> = Vec::new();
        let mut accepted = false;
        let mut last_verdict = String::new();
        let _ = &mut last_verdict;

        // Round 1: Proposer → Opponent → Judge.
        let proposer_out = self
            .proposer
            .execute(&query, &mut blackboard, cancel.clone())
            .await
            .map_err(|e| Self::map_role_err("proposer", e))?;
        let opponent_out = self
            .opponent
            .execute(&query, &mut blackboard, cancel.clone())
            .await
            .map_err(|e| Self::map_role_err("opponent", e))?;
        let judge_out = self
            .judge
            .execute(&query, &mut blackboard, cancel.clone())
            .await
            .map_err(|e| Self::map_role_err("judge", e))?;
        let verdict = Self::extract_verdict(&judge_out);
        last_verdict = verdict.clone();
        rounds.push(DebateRound {
            round: 1,
            proposer: proposer_out,
            opponent: opponent_out,
            judge: judge_out,
            verdict: verdict.clone(),
        });

        // Revise loop: if the judge asked for revision and we still have
        // budget, re-run the Proposer with the critique in blackboard.
        let mut revise = 0;
        while verdict.to_uppercase().contains("REVISE") && revise < self.max_revise_rounds {
            revise += 1;
            let proposer_out = self
                .proposer
                .execute(&query, &mut blackboard, cancel.clone())
                .await
                .map_err(|e| Self::map_role_err("proposer", e))?;
            let opponent_out = self
                .opponent
                .execute(&query, &mut blackboard, cancel.clone())
                .await
                .map_err(|e| Self::map_role_err("opponent", e))?;
            let judge_out = self
                .judge
                .execute(&query, &mut blackboard, cancel.clone())
.await
            .map_err(|e| Self::map_role_err("judge", e))?;
            let v = Self::extract_verdict(&judge_out);
            last_verdict = v.clone();
            rounds.push(DebateRound {
                round: 1 + revise,
                proposer: proposer_out,
                opponent: opponent_out,
                judge: judge_out,
                verdict: v.clone(),
            });
            if v.to_uppercase().contains("ACCEPT") {
                accepted = true;
                break;
            }
        }

        let summary = if accepted {
            format!(
                "debate accepted after {} round(s) (final verdict: {last_verdict})",
                rounds.len()
            )
        } else if last_verdict.to_uppercase().contains("REJECT") {
            format!("debate rejected (verdict: {last_verdict})")
        } else {
            format!(
                "debate stopped after {} round(s); last verdict: {last_verdict}",
                rounds.len()
            )
        };

        let data = serde_json::to_value(&rounds).unwrap_or_default();
        let side_effects: Vec<SideEffect> = rounds
            .iter()
            .flat_map(|r| {
                [
                    SideEffect::ArtifactWritten {
                        key: format!("proposer/round_{}", r.round),
                        path: format!("debate/proposer_{}.json", r.round),
                    },
                    SideEffect::ArtifactWritten {
                        key: format!("opponent/round_{}", r.round),
                        path: format!("debate/opponent_{}.json", r.round),
                    },
                    SideEffect::ArtifactWritten {
                        key: format!("judge/round_{}", r.round),
                        path: format!("debate/judge_{}.json", r.round),
                    },
                ]
            })
            .collect();

        Ok(StageOutcome {
            data,
            summary,
            side_effects,
            mode: "debate".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn extract_query_accepts_string_or_keys() {
        let cancel = CancellationToken::new();
        let a = StageInput::new("d", serde_json::json!("q"), cancel.clone());
        assert_eq!(DebateRunner::extract_query(&a).unwrap(), "q");
        let b = StageInput::new("d", serde_json::json!({"prompt": "p"}), cancel.clone());
        assert_eq!(DebateRunner::extract_query(&b).unwrap(), "p");
        let c = StageInput::new("d", serde_json::json!({"query": "qq"}), cancel);
        assert_eq!(DebateRunner::extract_query(&c).unwrap(), "qq");
    }

    #[test]
    fn extract_verdict_falls_back_to_keywords() {
        let mut out = RoleOutput {
            content: "I vote ACCEPT on this hypothesis".into(),
            evidence: vec![],
            confidence: 0.0,
            metadata: std::collections::HashMap::new(),
            output_files: vec![],
            status: "success".into(),
        };
        assert_eq!(DebateRunner::extract_verdict(&out), "ACCEPT");
        out.content = "REVISE please".into();
        assert_eq!(DebateRunner::extract_verdict(&out), "REVISE");
        out.content = "no clear answer".into();
        out.metadata.insert("verdict".into(), "AMBIGUOUS".into());
        assert_eq!(DebateRunner::extract_verdict(&out), "AMBIGUOUS");
    }

    #[test]
    fn name_is_stable() {
        assert_eq!("planning::DebateRunner", "planning::DebateRunner");
    }
}