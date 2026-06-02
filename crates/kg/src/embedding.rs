use crate::graph::KnowledgeGraph;
use crate::schema::{EntityId, RelationType};
use std::collections::HashMap;

/// Simple TransE-style embedding for link prediction scoring.
/// Uses random projection to d-dimensional space for entities and relations.
pub struct KgeModel {
    dim: usize,
    entity_embeddings: HashMap<EntityId, Vec<f64>>,
    relation_embeddings: HashMap<RelationType, Vec<f64>>,
}

impl KgeModel {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            entity_embeddings: HashMap::new(),
            relation_embeddings: HashMap::new(),
        }
    }

    /// Train embeddings on the KG using simple SGD (TransE scoring: ||h + r - t||)
    pub fn train(&mut self, kg: &KnowledgeGraph, epochs: usize, lr: f64) {
        // Initialize embeddings
        for entity in kg.all_entities() {
            if !self.entity_embeddings.contains_key(&entity.id) {
                self.entity_embeddings.insert(entity.id, Self::random_vec(self.dim));
            }
        }

        for relation in kg.all_relations() {
            if !self.relation_embeddings.contains_key(&relation.relation_type) {
                self.relation_embeddings.insert(relation.relation_type.clone(), Self::random_vec(self.dim));
            }
        }

        let relations: Vec<_> = kg.all_relations().to_vec();
        if relations.is_empty() { return; }

        for _ in 0..epochs {
            for rel in &relations {
                // Collect updates first, then apply
                let h = self.entity_embeddings.get(&rel.from_id);
                let r = self.relation_embeddings.get(&rel.relation_type);
                let t = self.entity_embeddings.get(&rel.to_id);

                let updates: Option<(Vec<f64>, Vec<f64>, Vec<f64>)> = if let (Some(h), Some(r), Some(t)) = (h, r, t) {

                    let mut h_update = vec![0.0; self.dim];
                    let mut r_update = vec![0.0; self.dim];
                    let mut t_update = vec![0.0; self.dim];

                    for d in 0..self.dim {
                        let diff = h[d] + r[d] - t[d];
                        let update = lr * diff;
                        h_update[d] = -update;
                        r_update[d] = -update;
                        t_update[d] = update;
                    }
                    Some((h_update, r_update, t_update))
                } else {
                    None
                };

                if let Some((h_up, r_up, t_up)) = updates {
                    if let Some(h_vec) = self.entity_embeddings.get_mut(&rel.from_id) {
                        for d in 0..self.dim { h_vec[d] += h_up[d]; }
                    }
                    if let Some(r_vec) = self.relation_embeddings.get_mut(&rel.relation_type) {
                        for d in 0..self.dim { r_vec[d] += r_up[d]; }
                    }
                    if let Some(t_vec) = self.entity_embeddings.get_mut(&rel.to_id) {
                        for d in 0..self.dim { t_vec[d] += t_up[d]; }
                    }
                }
            }
        }
    }

    /// TransE distance: ||h + r - t||
    pub fn distance(&self, h: &EntityId, r: &RelationType, t: &EntityId) -> f64 {
        let h_vec = self.entity_embeddings.get(h);
        let r_vec = self.relation_embeddings.get(r);
        let t_vec = self.entity_embeddings.get(t);

        match (h_vec, r_vec, t_vec) {
            (Some(h), Some(r), Some(t)) => {
                let sum: f64 = h.iter()
                    .zip(r.iter())
                    .zip(t.iter())
                    .map(|((hi, ri), ti)| {
                        let diff = hi + ri - ti;
                        diff * diff
                    })
                    .sum();
                sum.sqrt()
            }
            _ => f64::MAX,
        }
    }

    fn random_vec(dim: usize) -> Vec<f64> {
        use std::hash::{DefaultHasher, Hasher};
        let mut state = DefaultHasher::new();
        state.write_u64(rand_u64());
        (0..dim)
            .map(|i| {
                state.write_usize(i);
                let h = state.finish();
                (h as f64 / u64::MAX as f64) * 2.0 - 1.0
            })
            .collect()
    }
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42)
}
