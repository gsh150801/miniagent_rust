use crate::generator::{Hypothesis, HypothesisNovelty};

pub struct HypothesisRanker;

impl HypothesisRanker {
    /// Multi-dimensional ranking of hypotheses.
    /// Score = kge * 0.3 + llm_confidence * 0.3 + novelty * 0.2 + feasibility * 0.2
    pub fn rank(hypotheses: &[Hypothesis]) -> Vec<RankedHypothesis> {
        let mut ranked: Vec<RankedHypothesis> = hypotheses
            .iter()
            .map(|h| {
                let kge = h.source_candidate.score;
                let llm = h.confidence;
                let novelty = Self::novelty_score(&h.novelty);
                let feasibility = h
                    .experimental_design
                    .as_ref()
                    .map(|ed| ed.feasibility)
                    .unwrap_or(0.5);

                let score = kge * 0.3 + llm * 0.3 + novelty * 0.2 + feasibility * 0.2;

                RankedHypothesis {
                    hypothesis: h.clone(),
                    composite_score: score,
                    breakdown: ScoreBreakdown {
                        kge,
                        llm_confidence: llm,
                        novelty,
                        feasibility,
                    },
                }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.composite_score
                .partial_cmp(&a.composite_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        ranked
    }

    fn novelty_score(novelty: &HypothesisNovelty) -> f64 {
        match novelty {
            HypothesisNovelty::Novel => 1.0,
            HypothesisNovelty::Incremental => 0.6,
            HypothesisNovelty::Trivial => 0.2,
            HypothesisNovelty::Unknown => 0.5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RankedHypothesis {
    pub hypothesis: Hypothesis,
    pub composite_score: f64,
    pub breakdown: ScoreBreakdown,
}

#[derive(Debug, Clone)]
pub struct ScoreBreakdown {
    pub kge: f64,
    pub llm_confidence: f64,
    pub novelty: f64,
    pub feasibility: f64,
}
