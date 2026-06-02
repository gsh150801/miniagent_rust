use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use crate::roles::{
    AgentRole, Blackboard, FileContext, RoleOutput,
    persist_output, load_checkpoint, append_event, parse_llm_json,
};
use crate::tournament::{TournamentArena, DebateSession, DebateRubricScores, Verdict};

/// Tournament Master: manages the Elo tournament lifecycle, triggers debate sessions,
/// checks convergence, and orchestrates hypothesis evolution.
pub struct TournamentMasterRole {
    provider: Box<dyn LlmProvider>,
}

impl TournamentMasterRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl AgentRole for TournamentMasterRole {
    fn name(&self) -> &str { "tournament_master" }
    fn description(&self) -> &str {
        "Tournament Master. Manages Elo tournament lifecycle: schedules debates, \
         records results, checks convergence, triggers hypothesis evolution."
    }

    async fn execute(
        &self,
        task: &str,
        blackboard: &mut Blackboard,
        cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        append_event(&blackboard.work_dir, "tournament_master: managing round");

        // Load or initialize arena
        let mut arena: TournamentArena = blackboard.artifacts.get("tournament_arena")
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| TournamentArena::new(5));

        // Load hypotheses from proposer outputs
        if arena.phase == crate::tournament::TournamentPhase::Seeding {
            self.seed_hypotheses(&mut arena, blackboard);
            if arena.elo.len() >= 2 {
                arena.start_tournament().ok();
            }
        }

        if arena.is_finished() {
            let standings = arena.standings();
            let summary = serde_json::to_string_pretty(&serde_json::json!({
                "phase": "converged",
                "standings": standings.iter().map(|r| serde_json::json!({
                    "hypothesis_id": r.hypothesis_id,
                    "rating": r.rating,
                    "wins": r.wins,
                    "losses": r.losses,
                })).collect::<Vec<_>>(),
            })).unwrap_or_default();

            persist_output(&blackboard.work_dir, "tournament_master", "final_standings.json", &summary);
            return Ok(RoleOutput {
                content: summary,
                evidence: vec![],
                confidence: 1.0,
                metadata: std::collections::HashMap::new(),
                output_files: vec!["tournament_master/final_standings.json".into()],
                status: "success".into(),
            });
        }

        // Run debates for current round
        let pairs = arena.round_robin_pairs();
        let mut debate_summaries = Vec::new();

        for (h_a, h_b) in &pairs {
            let h_a_text = self.load_hypothesis_text(blackboard, h_a);
            let h_b_text = self.load_hypothesis_text(blackboard, h_b);

            if let (Some(text_a), Some(text_b)) = (h_a_text, h_b_text) {
                let session = self.run_debate(task, &text_a, &text_b, h_a, h_b, cancel.clone()).await;
                match session {
                    Ok(session) => {
                        let summary = format!(
                            "Debate {} vs {}: {:?} (score_a={:.2}, score_b={:.2})",
                            h_a, h_b,
                            session.verdict,
                            session.rubric_scores_a.as_ref().map(|r| r.weighted_total()).unwrap_or(0.0),
                            session.rubric_scores_b.as_ref().map(|r| r.weighted_total()).unwrap_or(0.0),
                        );
                        debate_summaries.push(summary);
                        arena.record_debate(&session);
                    }
                    Err(e) => {
                        append_event(&blackboard.work_dir, &format!("tournament_master: debate error: {e}"));
                    }
                }
            }
        }

        arena.advance_round();

        // Persist arena state
        let arena_json = serde_json::to_string(&arena).unwrap_or_default();
        blackboard.artifacts.insert("tournament_arena".into(), arena_json.clone());
        persist_output(&blackboard.work_dir, "tournament_master", "arena.json", &arena_json);

        let standings = arena.standings();
        let content = serde_json::to_string_pretty(&serde_json::json!({
            "round": arena.round,
            "phase": format!("{:?}", arena.phase),
            "debates_this_round": debate_summaries,
            "standings": standings.iter().map(|r| serde_json::json!({
                "hypothesis_id": r.hypothesis_id,
                "rating": (r.rating * 100.0).round() / 100.0,
                "wins": r.wins,
                "losses": r.losses,
                "draws": r.draws,
            })).collect::<Vec<_>>(),
        })).unwrap_or_default();

        persist_output(&blackboard.work_dir, "tournament_master", "round_report.json", &content);
        append_event(&blackboard.work_dir, &format!("tournament_master: round {} complete", arena.round));

        Ok(RoleOutput {
            content,
            evidence: vec![],
            confidence: 0.85,
            metadata: std::collections::HashMap::new(),
            output_files: vec!["tournament_master/arena.json".into(), "tournament_master/round_report.json".into()],
            status: "success".into(),
        })
    }
}

#[async_trait]
impl FileContext for TournamentMasterRole {
    fn workspace_name(&self) -> &str { "tournament_master" }
}

impl TournamentMasterRole {
    fn seed_hypotheses(&self, arena: &mut TournamentArena, blackboard: &Blackboard) {
        // Look for hypothesis files in proposer/ subdirectories
        let work_dir = &blackboard.work_dir;
        if let Ok(entries) = std::fs::read_dir(work_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let role_name = path.file_name().unwrap_or_default().to_string_lossy();
                    if role_name.starts_with("proposer") {
                        let hyp_file = path.join("hypothesis.json");
                        if hyp_file.exists()
                            && let Ok(content) = std::fs::read_to_string(&hyp_file)
                                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content)
                                    && let Some(id) = parsed["hypothesis"].as_str() {
                                        let h_id = format!("{}_{}", role_name, id.len());
                                        arena.seed(h_id);
                                    }
                    }
                }
            }
        }
    }

    fn load_hypothesis_text(&self, blackboard: &Blackboard, hypothesis_id: &str) -> Option<String> {
        blackboard.artifacts.get(hypothesis_id).cloned()
            .or_else(|| load_checkpoint(&blackboard.work_dir, "proposer", "hypothesis.json"))
    }

    async fn run_debate(
        &self,
        topic: &str,
        text_a: &str,
        text_b: &str,
        id_a: &str,
        id_b: &str,
        cancel: CancellationToken,
    ) -> Result<DebateSession, AgentError> {
        let prompt = format!(
            r#"Evaluate two competing hypotheses on: {topic}

## Hypothesis A ({id_a})
{text_a}

## Hypothesis B ({id_b})
{text_b}

## Scoring Rubric (0-1 scale each)
- evidence_support (weight 0.30): quality and quantity of supporting evidence
- mechanistic_plausibility (weight 0.25): logical coherence of proposed mechanism
- falsifiability (weight 0.20): can this be experimentally tested?
- novelty (weight 0.15): does it offer new insights?
- consistency (weight 0.10): consistent with established knowledge?

## Output Format (JSON)
{{
  "scores_a": {{
    "evidence_support": 0.0-1.0,
    "mechanistic_plausibility": 0.0-1.0,
    "falsifiability": 0.0-1.0,
    "novelty": 0.0-1.0,
    "consistency": 0.0-1.0
  }},
  "scores_b": {{ ... same structure ... }},
  "verdict": "accept_a" | "accept_b" | "revise_a" | "revise_b" | "draw",
  "critique_a": "specific issues with hypothesis A",
  "critique_b": "specific issues with hypothesis B",
  "reasoning": "overall reasoning for the verdict"
}}
"#
        );

        let response = self.call_llm(
            "You are an impartial scientific judge. Score rigorously. Output valid JSON.",
            &prompt,
            cancel,
        ).await?;

        let parsed = parse_llm_json(&response).map_err(AgentError::Internal)?;

        let scores_a = parsed.get("scores_a").map(DebateRubricScores::from_json);
        let scores_b = parsed.get("scores_b").map(DebateRubricScores::from_json);
        let verdict = parsed["verdict"].as_str().and_then(Verdict::parse);
        let critique_a = parsed["critique_a"].as_str().map(String::from);
        let critique_b = parsed["critique_b"].as_str().map(String::from);
        let reasoning = parsed["reasoning"].as_str().map(String::from);

        let mut session = DebateSession::new(
            format!("debate_{id_a}_vs_{id_b}"),
            id_a, text_a,
            id_b, text_b,
        );
        session.rubric_scores_a = scores_a;
        session.rubric_scores_b = scores_b;
        session.verdict = verdict;
        session.critique_a = critique_a;
        session.critique_b = critique_b;
        session.judge_reasoning = reasoning;
        session.rounds_completed = 1;

        Ok(session)
    }

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
}
