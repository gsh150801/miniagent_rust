use crate::graph::KnowledgeGraph;
use crate::schema::{EntityId, RelationType};
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;

/// Distance norm used by the TransE scoring function `d(h, r, t)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceNorm {
    L1,
    L2,
}

/// Tunable configuration for TransE training.
///
/// Implements the textbook formulation (Bordes et al. 2013): margin-based
/// ranking loss `max(0, γ + d(pos) − d(neg))` with uniform negative sampling,
/// optimized by plain SGD. Entity vectors are L2-normalized after every epoch.
#[derive(Debug, Clone)]
pub struct TrainConfig {
    /// Margin γ for the ranking loss.
    pub margin: f64,
    /// Number of negative samples generated per positive triple per epoch.
    pub num_negatives: usize,
    /// Per-epoch learning-rate multiplier (`lr *= decay` each epoch).
    pub lr_decay: f64,
    /// Distance norm (L1 or L2).
    pub norm: DistanceNorm,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            margin: 1.0,
            num_negatives: 5,
            lr_decay: 1.0,
            norm: DistanceNorm::L2,
        }
    }
}

/// TransE-style embedding for link prediction scoring.
///
/// Trained with margin-based ranking loss and negative sampling. Entity vectors
/// are L2-normalized after each epoch so that distance comparisons are scale-free.
pub struct KgeModel {
    dim: usize,
    norm: DistanceNorm,
    entity_embeddings: HashMap<EntityId, Vec<f64>>,
    relation_embeddings: HashMap<RelationType, Vec<f64>>,
}

impl KgeModel {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            norm: DistanceNorm::L2,
            entity_embeddings: HashMap::new(),
            relation_embeddings: HashMap::new(),
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Train embeddings on the KG with the default `TrainConfig`.
    pub fn train(&mut self, kg: &KnowledgeGraph, epochs: usize, lr: f64) {
        self.train_with(kg, epochs, lr, &TrainConfig::default());
    }

    /// Train embeddings using margin-based ranking loss with negative sampling.
    pub fn train_with(&mut self, kg: &KnowledgeGraph, epochs: usize, mut lr: f64, cfg: &TrainConfig) {
        self.norm = cfg.norm;

        let bound = 6.0 / (self.dim as f64).sqrt();
        for entity in kg.all_entities() {
            self.entity_embeddings
                .entry(entity.id)
                .or_insert_with(|| Self::uniform_vec(self.dim, bound));
        }
        for relation in kg.all_relations() {
            self.relation_embeddings
                .entry(relation.relation_type.clone())
                .or_insert_with(|| Self::uniform_vec(self.dim, bound));
        }

        let triples: Vec<(EntityId, RelationType, EntityId)> = kg
            .all_relations()
            .iter()
            .map(|r| (r.from_id, r.relation_type.clone(), r.to_id))
            .collect();
        if triples.is_empty() {
            return;
        }

        let entity_pool: Vec<EntityId> = self.entity_embeddings.keys().copied().collect();
        if entity_pool.len() < 2 {
            // Need at least two entities for meaningful negative sampling.
            return;
        }
        let mut rng = rand::thread_rng();

        for _epoch in 0..epochs {
            let mut order = (0..triples.len()).collect::<Vec<_>>();
            order.shuffle(&mut rng);

            for &idx in &order {
                let (h, r, t) = &triples[idx];
                for _ in 0..cfg.num_negatives {
                    let corrupt_head: bool = rng.gen_bool(0.5);
                    let neg_id = entity_pool.choose(&mut rng).copied().unwrap();
                    // Skip degenerate corruptions that reproduce the positive triple.
                    if corrupt_head && neg_id == *h {
                        continue;
                    }
                    if !corrupt_head && neg_id == *t {
                        continue;
                    }

                    let (neg_h, neg_t) = if corrupt_head { (neg_id, *t) } else { (*h, neg_id) };

                    let (diff_p, d_pos) = self.diff_and_distance(h, r, t);
                    let (diff_n, d_neg) = self.diff_and_distance(&neg_h, r, &neg_t);
                    if d_pos.is_infinite() || d_neg.is_infinite() {
                        continue;
                    }

                    let loss = cfg.margin + d_pos - d_neg;
                    if loss > 0.0 {
                        self.apply_ranking_gradient(
                            h, r, t, &diff_p, d_pos, &neg_h, &neg_t, &diff_n, d_neg, cfg.norm, lr,
                        );
                    }
                }
            }

            // Normalize entity & relation vectors after each epoch (standard TransE).
            self.normalize_all();
            lr *= cfg.lr_decay;
        }
    }

    /// Compute element-wise `h + r - t` and its norm distance.
    fn diff_and_distance(
        &self,
        h: &EntityId,
        r: &RelationType,
        t: &EntityId,
    ) -> (Vec<f64>, f64) {
        match (
            self.entity_embeddings.get(h),
            self.relation_embeddings.get(r),
            self.entity_embeddings.get(t),
        ) {
            (Some(hv), Some(rv), Some(tv)) => {
                let diff: Vec<f64> = (0..self.dim)
                    .map(|d| hv[d] + rv[d] - tv[d])
                    .collect();
                let dist = match self.norm {
                    DistanceNorm::L1 => diff.iter().map(|x| x.abs()).sum(),
                    DistanceNorm::L2 => diff.iter().map(|x| x * x).sum::<f64>().sqrt(),
                };
                (diff, dist)
            }
            _ => (Vec::new(), f64::INFINITY),
        }
    }

    /// Apply one margin-ranking gradient step.
    /// Positive triple is pulled together (d_pos ↓); negative pushed apart (d_neg ↑).
    #[allow(clippy::too_many_arguments)]
    fn apply_ranking_gradient(
        &mut self,
        h: &EntityId,
        r: &RelationType,
        t: &EntityId,
        diff_p: &[f64],
        d_pos: f64,
        neg_h: &EntityId,
        neg_t: &EntityId,
        diff_n: &[f64],
        d_neg: f64,
        norm: DistanceNorm,
        lr: f64,
    ) {
        if d_pos <= 0.0 || d_neg <= 0.0 {
            return;
        }
        // Per-dimension update direction. For L2: dir = diff/d (gradient of ||.||_2).
        // For L1: dir = sign(diff).
        let (g_pos, g_neg) = match norm {
            DistanceNorm::L2 => (lr / d_pos, lr / d_neg),
            DistanceNorm::L1 => (lr, lr),
        };

        // Positive triple: minimize d_pos ⇒ h -= g_pos*dir, r -= g_pos*dir, t += g_pos*dir
        if let Some(v) = self.entity_embeddings.get_mut(h) {
            for d in 0..self.dim {
                v[d] -= g_pos * Self::dir(diff_p[d], d_pos, norm);
            }
        }
        if let Some(v) = self.relation_embeddings.get_mut(r) {
            for d in 0..self.dim {
                v[d] -= g_pos * Self::dir(diff_p[d], d_pos, norm);
            }
        }
        if let Some(v) = self.entity_embeddings.get_mut(t) {
            for d in 0..self.dim {
                v[d] += g_pos * Self::dir(diff_p[d], d_pos, norm);
            }
        }

        // Negative triple: maximize d_neg ⇒ neg_h += g_neg*dir_n, neg_t -= g_neg*dir_n
        if let Some(v) = self.entity_embeddings.get_mut(neg_h) {
            for d in 0..self.dim {
                v[d] += g_neg * Self::dir(diff_n[d], d_neg, norm);
            }
        }
        if let Some(v) = self.entity_embeddings.get_mut(neg_t) {
            for d in 0..self.dim {
                v[d] -= g_neg * Self::dir(diff_n[d], d_neg, norm);
            }
        }
    }

    /// Gradient direction of the distance w.r.t. a coordinate.
    #[inline]
    fn dir(x: f64, d: f64, norm: DistanceNorm) -> f64 {
        match norm {
            DistanceNorm::L2 => {
                if d > 0.0 {
                    x / d
                } else {
                    0.0
                }
            }
            DistanceNorm::L1 => x.signum(),
        }
    }

    /// L2-normalize entity *and* relation vectors after each epoch.
    ///
    /// Entities are normalized per the original TransE formulation (Bordes et
    /// al. 2013). Relations are normalized too: this prevents the relation
    /// vectors from blowing up under SGD (which would let the margin loss be
    /// satisfied trivially without learning structure). Bounding both to the
    /// unit sphere keeps distances in a stable range — the convention used by
    /// production implementations such as OpenKE.
    fn normalize_all(&mut self) {
        for v in self.entity_embeddings.values_mut() {
            Self::l2_normalize(v);
        }
        for v in self.relation_embeddings.values_mut() {
            Self::l2_normalize(v);
        }
    }

    fn l2_normalize(v: &mut [f64]) {
        let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }

    /// TransE distance `||h + r - t||` (uses the model's norm, L2 by default).
    pub fn distance(&self, h: &EntityId, r: &RelationType, t: &EntityId) -> f64 {
        let (_, d) = self.diff_and_distance(h, r, t);
        d
    }

    /// Hold-out evaluation: train on `(1 - test_frac)` of the edges, then
    /// rank each held-out triple against every entity as a corrupted tail
    /// (raw — unfiltered — ranking; good enough as a printed diagnostic).
    ///
    /// Returns `(mrr, hits_at_10, n_test)`. Same test edges are used for
    /// every call with the same KG so numbers are comparable across runs of
    /// the pipeline.
    pub fn holdout_evaluate(
        dim: usize,
        kg: &KnowledgeGraph,
        epochs: usize,
        lr: f64,
        test_frac: f64,
    ) -> (f64, usize, usize) {
        let relations = kg.all_relations();
        if relations.len() < 10 {
            return (0.0, 0, 0); // too small to evaluate meaningfully
        }
        let n_test = ((relations.len() as f64) * test_frac.clamp(0.05, 0.3)) as usize;
        let n_test = n_test.max(1);

        // Deterministic split: stride sampling keeps it stable and cheap.
        let stride = relations.len() / n_test.max(1);
        let test: Vec<&crate::schema::Relation> = (0..n_test)
            .map(|i| &relations[(i * stride).min(relations.len() - 1)])
            .collect();
        let test_keys: std::collections::HashSet<(usize, usize)> = test
            .iter()
            .map(|r| (r.from_id.0.as_u128() as usize, r.to_id.0.as_u128() as usize))
            .collect();

        let mut train_kg = KnowledgeGraph::new();
        for e in kg.all_entities() {
            train_kg.add_entity(e.clone());
        }
        for r in relations {
            let key = (r.from_id.0.as_u128() as usize, r.to_id.0.as_u128() as usize);
            if !test_keys.contains(&key) {
                train_kg.add_relation(r.clone());
            }
        }

        let mut model = KgeModel::new(dim);
        model.train(&train_kg, epochs, lr);

        let all_entity_ids: Vec<crate::schema::EntityId> =
            kg.all_entities().map(|e| e.id).collect();
        let mut reciprocal_ranks = Vec::with_capacity(test.len());
        let mut hits = 0usize;
        for rel in &test {
            // Rank the true tail against all entities.
            let mut scored: Vec<(f64, bool)> = all_entity_ids
                .iter()
                .map(|cand| {
                    let d = model.distance(&rel.from_id, &rel.relation_type, cand);
                    (d, *cand == rel.to_id)
                })
                .collect();
            scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if let Some(rank0) = scored.iter().position(|(_, truth)| *truth) {
                let rank = rank0 + 1;
                reciprocal_ranks.push(1.0 / rank as f64);
                if rank <= 10 {
                    hits += 1;
                }
            }
        }
        let mrr = if reciprocal_ranks.is_empty() {
            0.0
        } else {
            reciprocal_ranks.iter().sum::<f64>() / reciprocal_ranks.len() as f64
        };
        (mrr, hits, test.len())
    }

    fn uniform_vec(dim: usize, bound: f64) -> Vec<f64> {
        let mut rng = rand::thread_rng();
        (0..dim)
            .map(|_| rng.gen_range(-bound..=bound))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeGraph;
    use crate::schema::{Entity, EntityType, Relation, RelationType};

    fn build_small_kg() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::new();
        let genes = ["BRCA1", "BRCA2", "TP53", "MYC", "EGFR", "KRAS"];
        let diseases = ["breast cancer", "ovarian cancer", "lung cancer"];
        let gene_ids: Vec<_> = genes
            .iter()
            .map(|name| {
                let e = Entity {
                    id: EntityId::new(),
                    name: (*name).to_string(),
                    entity_type: EntityType::Gene,
                    aliases: vec![],
                    metadata: serde_json::Value::Null,
                };
                let id = e.id;
                kg.add_entity(e);
                id
            })
            .collect();
        let disease_ids: Vec<_> = diseases
            .iter()
            .map(|name| {
                let e = Entity {
                    id: EntityId::new(),
                    name: (*name).to_string(),
                    entity_type: EntityType::Disease,
                    aliases: vec![],
                    metadata: serde_json::Value::Null,
                };
                let id = e.id;
                kg.add_entity(e);
                id
            })
            .collect();

        // 5 known associated-with triples.
        let links = [(0, 0), (1, 0), (1, 1), (4, 2), (5, 2)];
        for (gi, di) in links {
            kg.add_relation(Relation {
                id: crate::schema::RelationId::new(),
                from_id: gene_ids[gi],
                to_id: disease_ids[di],
                relation_type: RelationType::AssociatedWith,
                confidence: 0.9,
                evidence: "test".into(),
                source_paper_id: None,
                support_count: 1,
                supporting_papers: vec![],
            });
        }
        kg
    }

    #[test]
    fn train_reduces_positive_distance() {
        let kg = build_small_kg();
        let rels = kg.all_relations();
        let (h, r, t) = (rels[0].from_id, rels[0].relation_type.clone(), rels[0].to_id);

        let mut model = KgeModel::new(16);
        let before = {
            model.train_with(&kg, 0, 0.01, &TrainConfig::default());
            model.distance(&h, &r, &t)
        };

        let mut model2 = KgeModel::new(16);
        model2.train_with(&kg, 300, 0.05, &TrainConfig::default());
        let after = model2.distance(&h, &r, &t);

        assert!(
            after < before,
            "training should reduce positive triple distance: before={before:.4} after={after:.4}"
        );
    }

    #[test]
    fn training_separates_positives_from_corruptions() {
        let kg = build_small_kg();
        let positives: Vec<(EntityId, RelationType, EntityId)> = kg
            .all_relations()
            .iter()
            .map(|r| (r.from_id, r.relation_type.clone(), r.to_id))
            .collect();
        let all_entities: Vec<EntityId> = kg.all_entities().map(|e| e.id).collect();

        let mut model = KgeModel::new(24);
        model.train_with(&kg, 800, 0.05, &TrainConfig::default());

        // Aggregate property (the one that matters for link prediction):
        // mean positive distance < mean corruption distance.
        let pos_mean = positives
            .iter()
            .map(|(h, r, t)| model.distance(h, r, t))
            .sum::<f64>()
            / positives.len() as f64;
        let mut corr_sum = 0.0;
        let mut corr_n = 0;
        for (h, r, t) in &positives {
            for cid in &all_entities {
                if cid == t {
                    continue;
                }
                corr_sum += model.distance(h, r, cid);
                corr_n += 1;
            }
        }
        let corr_mean = corr_sum / corr_n as f64;

        assert!(
            pos_mean < corr_mean,
            "positives should be closer than corruptions on average: pos_mean={pos_mean:.4} corr_mean={corr_mean:.4}"
        );
    }

    #[test]
    fn l2_normalization_keeps_unit_length() {
        let kg = build_small_kg();
        let mut model = KgeModel::new(20);
        model.train_with(&kg, 50, 0.05, &TrainConfig::default());

        for v in model.entity_embeddings.values() {
            let n: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            assert!((n - 1.0).abs() < 1e-6, "entity vector not unit length: {n:.6}");
        }
        for v in model.relation_embeddings.values() {
            let n: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            assert!((n - 1.0).abs() < 1e-6, "relation vector not unit length: {n:.6}");
        }
    }

    #[test]
    fn train_with_l1_norm_does_not_panic() {
        let kg = build_small_kg();
        let mut model = KgeModel::new(16);
        let cfg = TrainConfig {
            margin: 0.5,
            num_negatives: 3,
            lr_decay: 0.98,
            norm: DistanceNorm::L1,
        };
        model.train_with(&kg, 100, 0.05, &cfg);
        // Spot-check: distance is finite for a known triple.
        let rels = kg.all_relations();
        let d = model.distance(&rels[0].from_id, &rels[0].relation_type, &rels[0].to_id);
        assert!(d.is_finite(), "L1 distance should be finite after training");
    }

    #[test]
    fn train_with_lr_decay_converges() {
        let kg = build_small_kg();
        let rels = kg.all_relations();
        let (h, r, t) = (rels[0].from_id, rels[0].relation_type.clone(), rels[0].to_id);

        let before = {
            let mut m = KgeModel::new(16);
            m.train_with(&kg, 0, 0.1, &TrainConfig::default());
            m.distance(&h, &r, &t)
        };

        let mut model = KgeModel::new(16);
        let cfg = TrainConfig {
            margin: 1.0,
            num_negatives: 5,
            lr_decay: 0.95,
            norm: DistanceNorm::L2,
        };
        model.train_with(&kg, 400, 0.1, &cfg);
        let after = model.distance(&h, &r, &t);
        assert!(
            after < before,
            "with decay training should still reduce positive distance: before={before:.4} after={after:.4}"
        );
    }
}
