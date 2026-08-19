use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    /// GIVE-style semantic-neighborhood overlap score (0 if no KGE/known tails).
    #[serde(default)]
    pub give_score: f64,
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

/// Multi-signal link-prediction scorer that fuses three normalized signals:
///
/// 1. **KGE** — TransE distance `1/(1+d(h,r,t))` (structural plausibility).
/// 2. **Path** — count of multi-hop paths `h → t` (topological evidence).
/// 3. **GIVE** — Jaccard overlap between the candidate's neighborhood and the
///    neighborhood of known tails of `(h, r)` (semantic / veracity extrapolation).
///
/// Weights are normalized to sum to 1.0 so the composite score lives in `[0, 1]`.
pub struct LinkPredictionScorer {
    kge_model: Option<KgeModel>,
    kge_weight: f64,
    path_weight: f64,
    give_weight: f64,
}

impl LinkPredictionScorer {
    pub fn new() -> Self {
        Self::with_raw_weights(0.40, 0.35, 0.25)
    }

    pub fn with_kge(mut self, kge: KgeModel) -> Self {
        self.kge_model = Some(kge);
        self
    }

    /// Configure the three signal weights (they are normalized to sum to 1.0).
    pub fn with_weights(kge: f64, path: f64, give: f64) -> Self {
        Self::with_raw_weights(kge, path, give)
    }

    fn with_raw_weights(kge: f64, path: f64, give: f64) -> Self {
        let total = (kge + path + give).max(1e-9);
        Self {
            kge_model: None,
            kge_weight: kge / total,
            path_weight: path / total,
            give_weight: give / total,
        }
    }

    /// Score all potential `(h, r, ?)` tails in the KG.
    pub fn predict_tails(
        &self,
        head: &EntityId,
        rel_type: &RelationType,
        kg: &KnowledgeGraph,
        max_results: usize,
    ) -> Vec<HypothesisCandidate> {
        let known_tails: HashSet<EntityId> = kg
            .query_tails(head, rel_type)
            .into_iter()
            .copied()
            .collect();

        // Pre-compute the union of known tails' neighborhoods for GIVE scoring.
        let known_neighborhood: HashSet<EntityId> = known_tails
            .iter()
            .flat_map(|t| kg.neighborhood(t).into_iter().map(|(_, id, _)| id))
            .filter(|id| !known_tails.contains(id) && *id != *head)
            .collect();

        let mut candidates = Vec::new();

        for entity in kg.all_entities() {
            if known_tails.contains(&entity.id) || entity.id == *head {
                continue;
            }

            let paths = kg.find_paths(head, &entity.id, 3);
            let has_kge = self.kge_model.is_some();

            // Skip candidates with no structural signal at all.
            if paths.is_empty() && !has_kge && known_neighborhood.is_empty() {
                continue;
            }

            let s_kge = self
                .kge_model
                .as_ref()
                .map(|kge| {
                    let d = kge.distance(head, rel_type, &entity.id);
                    if d.is_finite() {
                        1.0 / (1.0 + d)
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);

            let s_path = if !paths.is_empty() {
                (paths.len() as f64 / 5.0).min(1.0)
            } else {
                0.0
            };

            let s_give = self.give_overlap(&entity.id, &known_neighborhood, kg);

            let score = s_kge * self.kge_weight
                + s_path * self.path_weight
                + s_give * self.give_weight;

            if score > 0.1 {
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
                        give_score: s_give,
                        supporting_paths: paths,
                        novelty,
                    },
                });
            }
        }

        candidates.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(max_results);
        candidates
    }

    /// GIVE-style veracity extrapolation:
    /// Given `(h, r)`, take all known tails, and surface entities in their
    /// semantic neighborhood as candidates. Emphasizes the GIVE signal.
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

        if known_tails.is_empty() {
            // No known tails — fall back to structural prediction.
            return self.predict_tails(head, rel_type, kg, max_results);
        }

        let known_set: HashSet<EntityId> = known_tails.iter().copied().collect();
        let known_neighborhood: HashSet<EntityId> = known_tails
            .iter()
            .flat_map(|t| kg.neighborhood(t).into_iter().map(|(_, id, _)| id))
            .filter(|id| !known_set.contains(id) && *id != *head)
            .collect();

        let mut candidates = Vec::new();
        for neighbor_id in &known_neighborhood {
            let paths = kg.find_paths(head, neighbor_id, 3);

            let s_kge = self
                .kge_model
                .as_ref()
                .map(|kge| {
                    let d = kge.distance(head, rel_type, neighbor_id);
                    if d.is_finite() {
                        1.0 / (1.0 + d)
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);

            let s_path = if !paths.is_empty() {
                (paths.len() as f64 / 3.0).min(1.0)
            } else {
                0.0
            };

            // A GIVE-extrapolated candidate is, by construction, in the known
            // neighborhood, so it gets the full GIVE signal.
            let s_give = 1.0;

            let score = s_kge * self.kge_weight
                + s_path * self.path_weight
                + s_give * self.give_weight;

            candidates.push(HypothesisCandidate {
                head: *head,
                relation: rel_type.clone(),
                tail: *neighbor_id,
                score,
                evidence: HypothesisEvidence {
                    kge_score: s_kge,
                    path_score: s_path,
                    give_score: s_give,
                    supporting_paths: paths,
                    novelty: HypothesisNovelty::Novel,
                },
            });
        }

        candidates.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(max_results);
        candidates
    }

    /// Jaccard-style overlap between a candidate's direct neighborhood and the
    /// union neighborhood of known tails of `(h, r)`. Higher overlap ⇒ the
    /// candidate sits in a similar semantic region as verified tails.
    fn give_overlap(
        &self,
        candidate: &EntityId,
        known_neighborhood: &HashSet<EntityId>,
        kg: &KnowledgeGraph,
    ) -> f64 {
        if known_neighborhood.is_empty() {
            return 0.0;
        }
        let cand_neighbors: HashSet<EntityId> = kg
            .neighborhood(candidate)
            .into_iter()
            .map(|(_, id, _)| id)
            .collect();
        if cand_neighbors.is_empty() {
            return 0.0;
        }
        let intersection = cand_neighbors
            .intersection(known_neighborhood)
            .count() as f64;
        let union = cand_neighbors.union(known_neighborhood).count() as f64;
        if union > 0.0 {
            intersection / union
        } else {
            0.0
        }
    }
}

impl Default for LinkPredictionScorer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::KgeModel;
    use crate::graph::KnowledgeGraph;
    use crate::schema::{Entity, EntityType, Relation, RelationId};

    fn build_kg() -> KnowledgeGraph {
        // head: G1; known tails under AssociatedWith: D1, D2.
        // D2 shares neighbor X with candidate D3 ⇒ GIVE signal for D3.
        let mut kg = KnowledgeGraph::new();
        let mk = |name: &str, t: EntityType| -> Entity {
            Entity {
                id: EntityId::new(),
                name: name.into(),
                entity_type: t,
                aliases: vec![],
                metadata: serde_json::Value::Null,
            }
        };
        let g1 = mk("G1", EntityType::Gene);
        let d1 = mk("D1", EntityType::Disease);
        let d2 = mk("D2", EntityType::Disease);
        let d3 = mk("D3", EntityType::Disease);
        let x = mk("X", EntityType::Gene);
        let ids = [g1, d1, d2, d3, x]
            .into_iter()
            .map(|e| {
                let id = e.id;
                kg.add_entity(e);
                id
            })
            .collect::<Vec<_>>();
        let [g1, d1, d2, d3, x] = [ids[0], ids[1], ids[2], ids[3], ids[4]];

        let rel = |from, to| Relation {
            id: RelationId::new(),
            from_id: from,
            to_id: to,
            relation_type: RelationType::AssociatedWith,
            confidence: 0.9,
            evidence: "test".into(),
            source_paper_id: None,
            support_count: 1,
            supporting_papers: vec![],
        };
        kg.add_relation(rel(g1, d1));
        kg.add_relation(rel(g1, d2));
        // D2–X and D3–X: shared neighbor ⇒ GIVE overlap between D2 (known) and D3 (candidate).
        kg.add_relation(rel(d2, x));
        kg.add_relation(rel(d3, x));
        kg
    }

    #[test]
    fn weights_normalized_to_one() {
        let s = LinkPredictionScorer::with_weights(2.0, 2.0, 1.0);
        let sum = s.kge_weight + s.path_weight + s.give_weight;
        assert!((sum - 1.0).abs() < 1e-9, "weights must sum to 1.0, got {sum}");
        assert!((s.kge_weight - 0.4).abs() < 1e-9);
        assert!((s.path_weight - 0.4).abs() < 1e-9);
        assert!((s.give_weight - 0.2).abs() < 1e-9);
    }

    #[test]
    fn default_weights_sum_to_one() {
        let s = LinkPredictionScorer::new();
        let sum = s.kge_weight + s.path_weight + s.give_weight;
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn composite_score_in_unit_interval() {
        let kg = build_kg();
        let head = kg.find_entity_by_name("G1").unwrap().id;
        let scorer = LinkPredictionScorer::new();
        let cands = scorer.predict_tails(&head, &RelationType::AssociatedWith, &kg, 10);
        for c in &cands {
            assert!(c.score >= 0.0 && c.score <= 1.0, "score out of [0,1]: {}", c.score);
        }
    }

    #[test]
    fn give_signal_favors_shared_neighborhood() {
        let kg = build_kg();
        let head = kg.find_entity_by_name("G1").unwrap().id;
        // Pure-GIVE scorer (no KGE) — D3 shares neighbor X with known tail D2.
        let scorer = LinkPredictionScorer::with_weights(0.0, 0.0, 1.0);
        let cands = scorer.predict_tails(&head, &RelationType::AssociatedWith, &kg, 10);
        let d3_id = kg.find_entity_by_name("D3").unwrap().id;
        let d3 = cands.iter().find(|c| c.tail == d3_id);
        assert!(d3.is_some(), "D3 should surface as a candidate");
        let d3 = d3.unwrap();
        assert!(d3.evidence.give_score > 0.0, "D3 should have positive GIVE score");
    }

    #[test]
    fn give_extrapolation_surfaces_neighborhood() {
        let kg = build_kg();
        let head = kg.find_entity_by_name("G1").unwrap().id;
        let scorer = LinkPredictionScorer::new();
        let cands = scorer.give_extrapolation(&head, &RelationType::AssociatedWith, &kg, 10);
        // X is a neighbor of known tail D2, so it should surface.
        let x_id = kg.find_entity_by_name("X").unwrap().id;
        assert!(cands.iter().any(|c| c.tail == x_id), "X should be extrapolated");
    }

    #[test]
    fn kge_signal_contributes_when_model_present() {
        let kg = build_kg();
        let head = kg.find_entity_by_name("G1").unwrap().id;
        let mut kge = KgeModel::new(8);
        kge.train(&kg, 50, 0.05);
        let scorer = LinkPredictionScorer::with_weights(1.0, 0.0, 0.0).with_kge(kge);
        let cands = scorer.predict_tails(&head, &RelationType::AssociatedWith, &kg, 10);
        // With pure-KGE weighting, surfaced candidates must have positive kge_score.
        for c in &cands {
            assert!(c.evidence.kge_score > 0.0);
        }
    }
}
