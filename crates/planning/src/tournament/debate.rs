use serde::{Deserialize, Serialize};

/// Five weighted dimensions for evaluating hypothesis strength in a debate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateRubricScores {
    pub evidence_support: f64,         // weight: 0.30
    pub mechanistic_plausibility: f64, // weight: 0.25
    pub falsifiability: f64,           // weight: 0.20
    pub novelty: f64,                  // weight: 0.15
    pub consistency: f64,              // weight: 0.10
}

impl Default for DebateRubricScores {
    fn default() -> Self {
        Self {
            evidence_support: 0.5,
            mechanistic_plausibility: 0.5,
            falsifiability: 0.5,
            novelty: 0.5,
            consistency: 0.5,
        }
    }
}

impl DebateRubricScores {
    const W_EVIDENCE: f64 = 0.30;
    const W_PLAUSIBILITY: f64 = 0.25;
    const W_FALSIFIABILITY: f64 = 0.20;
    const W_NOVELTY: f64 = 0.15;
    const W_CONSISTENCY: f64 = 0.10;

    pub fn weighted_total(&self) -> f64 {
        Self::W_EVIDENCE * self.evidence_support
            + Self::W_PLAUSIBILITY * self.mechanistic_plausibility
            + Self::W_FALSIFIABILITY * self.falsifiability
            + Self::W_NOVELTY * self.novelty
            + Self::W_CONSISTENCY * self.consistency
    }

    /// Parse from a JSON value (LLM output).
    pub fn from_json(val: &serde_json::Value) -> Self {
        Self {
            evidence_support: val["evidence_support"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
            mechanistic_plausibility: val["mechanistic_plausibility"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
            falsifiability: val["falsifiability"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
            novelty: val["novelty"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
            consistency: val["consistency"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
        }
    }
}

/// Verdict from a debate session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    AcceptA,
    AcceptB,
    ReviseA,
    ReviseB,
    Draw,
}

impl Verdict {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_lowercase().as_str() {
            "accept_a" | "accepta" => Self::AcceptA,
            "accept_b" | "acceptb" => Self::AcceptB,
            "revise_a" | "revisea" => Self::ReviseA,
            "revise_b" | "reviseb" => Self::ReviseB,
            "draw" => Self::Draw,
            _ => return None,
        })
    }

    /// Which hypothesis won, if any (0 = A, 1 = B, None = draw).
    pub fn winner(&self) -> Option<usize> {
        match self {
            Self::AcceptA | Self::ReviseB => Some(0),
            Self::AcceptB | Self::ReviseA => Some(1),
            Self::Draw => None,
        }
    }

    /// Whether a hypothesis needs revision.
    pub fn needs_revision(&self) -> Option<usize> {
        match self {
            Self::ReviseA => Some(0),
            Self::ReviseB => Some(1),
            _ => None,
        }
    }
}

/// A single debate session between two hypotheses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateSession {
    pub id: String,
    pub hypothesis_a_id: String,
    pub hypothesis_a_text: String,
    pub hypothesis_b_id: String,
    pub hypothesis_b_text: String,
    pub rubric_scores_a: Option<DebateRubricScores>,
    pub rubric_scores_b: Option<DebateRubricScores>,
    pub verdict: Option<Verdict>,
    pub critique_a: Option<String>,
    pub critique_b: Option<String>,
    pub judge_reasoning: Option<String>,
    pub evidence_cited: Vec<String>,
    pub rounds_completed: usize,
}

impl DebateSession {
    pub fn new(
        id: impl Into<String>,
        a_id: impl Into<String>,
        a_text: impl Into<String>,
        b_id: impl Into<String>,
        b_text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            hypothesis_a_id: a_id.into(),
            hypothesis_a_text: a_text.into(),
            hypothesis_b_id: b_id.into(),
            hypothesis_b_text: b_text.into(),
            rubric_scores_a: None,
            rubric_scores_b: None,
            verdict: None,
            critique_a: None,
            critique_b: None,
            judge_reasoning: None,
            evidence_cited: vec![],
            rounds_completed: 0,
        }
    }

    /// Resolve the debate to a MatchOutcome for Elo update.
    pub fn to_match_outcome(&self) -> super::MatchOutcome {
        match self.verdict {
            Some(Verdict::AcceptA) | Some(Verdict::ReviseB) => super::MatchOutcome::WinA,
            Some(Verdict::AcceptB) | Some(Verdict::ReviseA) => super::MatchOutcome::WinB,
            Some(Verdict::Draw) | None => super::MatchOutcome::Draw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rubric_weighted_total() {
        let scores = DebateRubricScores {
            evidence_support: 0.8,
            mechanistic_plausibility: 0.7,
            falsifiability: 0.6,
            novelty: 0.9,
            consistency: 0.5,
        };
        let total = scores.weighted_total();
        let expected = 0.30 * 0.8 + 0.25 * 0.7 + 0.20 * 0.6 + 0.15 * 0.9 + 0.10 * 0.5;
        assert!((total - expected).abs() < 0.001, "Expected {expected}, got {total}");
    }

    #[test]
    fn test_rubric_from_json() {
        let json = serde_json::json!({
            "evidence_support": 0.9,
            "mechanistic_plausibility": 0.8,
            "falsifiability": 0.7,
            "novelty": 0.6,
            "consistency": 0.5,
        });
        let scores = DebateRubricScores::from_json(&json);
        assert!((scores.evidence_support - 0.9).abs() < 0.01);
        assert!((scores.novelty - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_verdict_parse() {
        assert_eq!(Verdict::parse("accept_a"), Some(Verdict::AcceptA));
        assert_eq!(Verdict::parse("AcceptB"), Some(Verdict::AcceptB));
        assert_eq!(Verdict::parse("revise_a"), Some(Verdict::ReviseA));
        assert_eq!(Verdict::parse("draw"), Some(Verdict::Draw));
        assert_eq!(Verdict::parse("unknown"), None);
    }

    #[test]
    fn test_verdict_winner() {
        assert_eq!(Verdict::AcceptA.winner(), Some(0));
        assert_eq!(Verdict::AcceptB.winner(), Some(1));
        assert_eq!(Verdict::ReviseA.winner(), Some(1)); // B wins when A needs revision
        assert_eq!(Verdict::ReviseB.winner(), Some(0));
        assert_eq!(Verdict::Draw.winner(), None);
    }

    #[test]
    fn test_verdict_needs_revision() {
        assert_eq!(Verdict::ReviseA.needs_revision(), Some(0));
        assert_eq!(Verdict::ReviseB.needs_revision(), Some(1));
        assert_eq!(Verdict::AcceptA.needs_revision(), None);
    }

    #[test]
    fn test_debate_session_outcome() {
        let mut session = DebateSession::new("d1", "h1", "text1", "h2", "text2");
        session.verdict = Some(Verdict::AcceptA);
        assert_eq!(session.to_match_outcome(), super::super::MatchOutcome::WinA);
    }
}
