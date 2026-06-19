use serde::{Deserialize, Serialize};
use crate::embedding::KgeModel;
use crate::graph::KnowledgeGraph;
use crate::schema::{EntityId, RelationType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypothesisCandidate {
    pub head: EntityId,
    pub relation: RelationType,
    pub tail: EntityId,
    pub score: f64,
    pub evidence: HypothesisEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypothesisEvidence {
    pub kge_score: f64,
    pub path_score: f64,
    pub supporting_paths: Vec<Vec<(EntityId, RelationType, EntityId)>>,
    pub novelty: HypothesisNovelty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HypothesisNovelty {
    Novel,
    Incremental,
    Trivial,
    Unknown,
}

pub struct LinkPredictionScorer {
    kge_model: Option<KgeModel>,
    kge_weight: f64,
    path_weight: f64,
}

impl LinkPredictionScorer {
    pub fn new() -> Self {
        Self {
            kge_model: None,
            kge_weight: 0.35,
            path_weight: 0.30,
        }
    }

    pub fn with_kge(mut self, kge: KgeModel) -> Self {
        self.kge_model = Some(kge);
        self
    }

    /// Score all potential (h, r, ?) tails in the KG
    pub fn predict_tails(
        &self,
        head: &EntityId,
        rel_type: &RelationType,
        kg: &KnowledgeGraph,
        max_results: usize,
    ) -> Vec<HypothesisCandidate> {
        let known_tails: std::collections::HashSet<EntityId> = kg
            .query_tails(head, rel_type)
            .into_iter()
            .copied()
            .collect();

        let mut candidates = Vec::new();

        // For each entity in the KG (except known tails), compute a score
        for entity in kg.all_entities() {
            if known_tails.contains(&entity.id) || entity.id == *head {
                continue;
            }

            let paths = kg.find_paths(head, &entity.id, 3);
            if paths.is_empty() && self.kge_model.is_none() {
                continue; // No structural evidence
            }

            let s_kge = self
                .kge_model
                .as_ref()
                .map(|kge| {
                    let d = kge.distance(head, rel_type, &entity.id);
                    // Convert distance to similarity score: 1/(1+d)
                    1.0 / (1.0 + d)
                })
                .unwrap_or(0.0);

            let s_path = if !paths.is_empty() {
                // Score based on path confidence: more paths = higher score
                (paths.len() as f64 / 5.0).min(1.0)
            } else {
                0.0
            };

            let score = s_kge * self.kge_weight + s_path * self.path_weight;

            if score > 0.1 {
                // Determine novelty
                let novelty = if known_tails.is_empty() && paths.len() <= 1 {
                    HypothesisNovelty::Novel
                } else if paths.len() >= 3 {
                    HypothesisNovelty::Incremental
                } else {
                    HypothesisNovelty::Unknown
                };

                candidates.push(HypothesisCandidate {
                    head: *head,
                    relation: rel_type.clone(),
                    tail: entity.id,
                    score,
                    evidence: HypothesisEvidence {
                        kge_score: s_kge,
                        path_score: s_path,
                        supporting_paths: paths,
                        novelty,
                    },
                });
            }
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(max_results);
        candidates
    }

    /// GIVE-style veracity extrapolation:
    /// Given (h, r), find all known tails, compute their embedding centroid,
    /// and find semantically similar entities as candidates.
    pub fn give_extrapolation(
        &self,
        head: &EntityId,
        rel_type: &RelationType,
        kg: &KnowledgeGraph,
        max_results: usize,
    ) -> Vec<HypothesisCandidate> {
        let known_tails: Vec<EntityId> = kg
            .query_tails(head, rel_type)
            .into_iter()
            .copied()
            .collect();

        let mut candidates = Vec::new();
        let known_set: std::collections::HashSet<EntityId> = known_tails.iter().copied().collect();

        if known_tails.is_empty() {
            // No known tails — fall back to structural prediction
            return self.predict_tails(head, rel_type, kg, max_results);
        }

        // For each known tail, find its neighbors and suggest them as candidates
        for known_tail in &known_tails {
            let neighbors = kg.neighborhood(known_tail);
            for (_rt, neighbor_id, _conf) in neighbors {
                if known_set.contains(&neighbor_id) || neighbor_id == *head {
                    continue;
                }

                // Check if there's a path between head and neighbor
                let paths = kg.find_paths(head, &neighbor_id, 3);

                let s_path = if !paths.is_empty() {
                    (paths.len() as f64 / 3.0).min(1.0)
                } else {
                    0.3 // Weaker evidence but still from semantic neighborhood
                };

                let score = 0.6 + s_path * 0.4; // Higher base score for GIVE extrapolation

                if score > 0.3 {
                    candidates.push(HypothesisCandidate {
                        head: *head,
                        relation: rel_type.clone(),
                        tail: neighbor_id,
                        score,
                        evidence: HypothesisEvidence {
                            kge_score: 0.0,
                            path_score: s_path,
                            supporting_paths: paths,
                            novelty: HypothesisNovelty::Novel,
                        },
                    });
                }
            }
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(max_results);
        candidates
    }
}

impl Default for LinkPredictionScorer {
    fn default() -> Self {
        Self::new()
    }
}
