use serde::{Deserialize, Serialize};

use super::convergence::NashEquilibriumDetector;
use super::debate::DebateSession;
use super::elo::{EloEngine, MatchOutcome};

/// Phase of a hypothesis tournament.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TournamentPhase {
    Seeding,
    RoundRobin,
    Elimination,
    Converged,
}

/// Record of a single debate result within the tournament.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateResult {
    pub session_id: String,
    pub hypothesis_a_id: String,
    pub hypothesis_b_id: String,
    pub outcome: MatchOutcome,
    pub rubric_total_a: f64,
    pub rubric_total_b: f64,
    pub verdict: String,
    pub rounds_completed: usize,
    pub evidence_cited: Vec<String>,
}

/// The tournament arena manages the full lifecycle of hypothesis debates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TournamentArena {
    pub elo: EloEngine,
    pub phase: TournamentPhase,
    pub round: usize,
    pub max_rounds: usize,
    pub results: Vec<DebateResult>,
    pub convergence: NashEquilibriumDetector,
}

impl TournamentArena {
    pub fn new(max_rounds: usize) -> Self {
        Self {
            elo: EloEngine::default(),
            phase: TournamentPhase::Seeding,
            round: 0,
            max_rounds,
            results: vec![],
            convergence: NashEquilibriumDetector::default(),
        }
    }

    pub fn with_convergence_threshold(mut self, threshold: f64, window: usize) -> Self {
        self.convergence = NashEquilibriumDetector::new(threshold, window);
        self
    }

    /// Register a hypothesis for the tournament.
    pub fn seed(&mut self, hypothesis_id: impl Into<String>) {
        self.elo.register(hypothesis_id);
    }

    /// Transition from Seeding to RoundRobin.
    pub fn start_tournament(&mut self) -> Result<usize, String> {
        if self.phase != TournamentPhase::Seeding {
            return Err(format!("Cannot start tournament from phase {:?}", self.phase));
        }
        if self.elo.len() < 2 {
            return Err("Need at least 2 hypotheses to start tournament".into());
        }
        self.phase = TournamentPhase::RoundRobin;
        self.round = 1;
        Ok(self.elo.len())
    }

    /// Generate all pairwise matchings for round-robin.
    /// Returns pairs of (hypothesis_a_id, hypothesis_b_id).
    pub fn round_robin_pairs(&self) -> Vec<(String, String)> {
        let ids: Vec<String> = self.elo.ratings.keys().cloned().collect();
        let mut pairs = Vec::new();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                pairs.push((ids[i].clone(), ids[j].clone()));
            }
        }
        pairs
    }

    /// Record a completed debate session into the tournament.
    pub fn record_debate(&mut self, session: &DebateSession) {
        let outcome = session.to_match_outcome();

        self.elo.update_after_match(
            &session.hypothesis_a_id,
            &session.hypothesis_b_id,
            outcome,
        );

        let rubric_a = session.rubric_scores_a.as_ref().map(|r| r.weighted_total()).unwrap_or(0.0);
        let rubric_b = session.rubric_scores_b.as_ref().map(|r| r.weighted_total()).unwrap_or(0.0);
        let verdict_str = session.verdict
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "Unresolved".into());

        self.results.push(DebateResult {
            session_id: session.id.clone(),
            hypothesis_a_id: session.hypothesis_a_id.clone(),
            hypothesis_b_id: session.hypothesis_b_id.clone(),
            outcome,
            rubric_total_a: rubric_a,
            rubric_total_b: rubric_b,
            verdict: verdict_str,
            rounds_completed: session.rounds_completed,
            evidence_cited: session.evidence_cited.clone(),
        });
    }

    /// Advance to the next round. Returns false if tournament should stop.
    pub fn advance_round(&mut self) -> bool {
        if self.round >= self.max_rounds {
            self.phase = TournamentPhase::Converged;
            return false;
        }

        // Check convergence (Nash equilibrium)
        let variance = self.elo.rating_variance_top_k(3);
        if self.convergence.check(variance) {
            self.phase = TournamentPhase::Converged;
            return false;
        }

        self.round += 1;
        true
    }

    /// Check if the tournament has finished (converged or max rounds reached).
    pub fn is_finished(&self) -> bool {
        self.phase == TournamentPhase::Converged || self.round >= self.max_rounds
    }

    /// Get the current tournament standings.
    pub fn standings(&self) -> Vec<super::elo::PlayerRating> {
        let mut ratings: Vec<_> = self.elo.ratings.values().cloned().collect();
        ratings.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
        ratings
    }

    /// Get hypotheses that need revision (based on debate verdicts).
    pub fn hypotheses_needing_revision(&self) -> Vec<String> {
        let mut revise_set = std::collections::HashSet::new();
        for result in &self.results {
            let verdict = super::debate::Verdict::parse(&result.verdict);
            if let Some(v) = verdict
                && let Some(idx) = v.needs_revision() {
                    let id = if idx == 0 { &result.hypothesis_a_id } else { &result.hypothesis_b_id };
                    revise_set.insert(id.clone());
                }
        }
        revise_set.into_iter().collect()
    }

    /// Number of debates completed in current round.
    pub fn debates_this_round(&self) -> usize {
        let total_debates_per_round = self.round_robin_pairs().len();
        let debates_in_rounds_before = (self.round - 1) * total_debates_per_round;
        self.results.len().saturating_sub(debates_in_rounds_before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tournament::debate::Verdict;

    #[test]
    fn test_arena_seeding() {
        let mut arena = TournamentArena::new(5);
        arena.seed("h1");
        arena.seed("h2");
        arena.seed("h3");
        assert_eq!(arena.elo.len(), 3);
        assert_eq!(arena.phase, TournamentPhase::Seeding);
    }

    #[test]
    fn test_arena_start_requires_minimum() {
        let mut arena = TournamentArena::new(5);
        arena.seed("only_one");
        assert!(arena.start_tournament().is_err());
    }

    #[test]
    fn test_round_robin_pairs() {
        let mut arena = TournamentArena::new(5);
        arena.seed("a");
        arena.seed("b");
        arena.seed("c");
        arena.start_tournament().unwrap();

        let pairs = arena.round_robin_pairs();
        assert_eq!(pairs.len(), 3); // C(3,2) = 3
    }

    #[test]
    fn test_full_tournament_cycle() {
        let mut arena = TournamentArena::new(3);

        arena.seed("h1");
        arena.seed("h2");
        arena.start_tournament().unwrap();

        // Simulate debate
        let mut session = DebateSession::new("d1", "h1", "text1", "h2", "text2");
        session.verdict = Some(Verdict::AcceptA);
        session.rounds_completed = 1;
        arena.record_debate(&session);

        assert_eq!(arena.results.len(), 1);
        assert!(arena.elo.rating_of("h1") > 1000.0);
        assert!(arena.elo.rating_of("h2") < 1000.0);

        // Advance round
        let continued = arena.advance_round();
        assert!(continued);
        assert_eq!(arena.round, 2);

        // Another debate
        let mut session2 = DebateSession::new("d2", "h1", "text1", "h2", "text2");
        session2.verdict = Some(Verdict::AcceptA);
        session2.rounds_completed = 1;
        arena.record_debate(&session2);

        // Advance to round 3 (max)
        let continued = arena.advance_round();
        assert!(continued);
        assert_eq!(arena.round, 3);

        // Round 3 should exhaust max_rounds
        let mut session3 = DebateSession::new("d3", "h1", "text1", "h2", "text2");
        session3.verdict = Some(Verdict::Draw);
        session3.rounds_completed = 1;
        arena.record_debate(&session3);

        assert!(!arena.advance_round());
        assert!(arena.is_finished());
    }

    #[test]
    fn test_standings_ordering() {
        let mut arena = TournamentArena::new(5);
        arena.seed("weak");
        arena.seed("strong");
        arena.start_tournament().unwrap();

        for i in 0..3 {
            let mut session = DebateSession::new(
                format!("d{i}"), "strong", "text", "weak", "text",
            );
            session.verdict = Some(Verdict::AcceptA);
            session.rounds_completed = 1;
            arena.record_debate(&session);
        }

        let standings = arena.standings();
        assert_eq!(standings[0].hypothesis_id, "strong");
        assert_eq!(standings[1].hypothesis_id, "weak");
    }

    #[test]
    fn test_hypotheses_needing_revision() {
        let mut arena = TournamentArena::new(5);
        arena.seed("h1");
        arena.seed("h2");
        arena.start_tournament().unwrap();

        let mut session = DebateSession::new("d1", "h1", "text", "h2", "text");
        session.verdict = Some(Verdict::ReviseA);
        session.rounds_completed = 1;
        arena.record_debate(&session);

        let revise = arena.hypotheses_needing_revision();
        assert!(revise.contains(&"h1".to_string()));
        assert!(!revise.contains(&"h2".to_string()));
    }
}
