use std::collections::{HashMap, HashSet};
use crate::schema::{Entity, EntityId, Relation, RelationType};

type EdgeKey = (EntityId, RelationType, EntityId);

pub struct KnowledgeGraph {
    entities: HashMap<EntityId, Entity>,
    // Adjacency list: entity -> list of (relation, target, confidence)
    outgoing: HashMap<EntityId, Vec<(RelationType, EntityId, f64)>>,
    incoming: HashMap<EntityId, Vec<(RelationType, EntityId, f64)>>,
    relations: Vec<Relation>,
    /// (head, relation, tail) → position in `relations`. Backs O(1) triple
    /// aggregation: re-extracting a known triple increments support instead of
    /// appending a duplicate edge.
    edge_index: HashMap<EdgeKey, usize>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            relations: Vec::new(),
            edge_index: HashMap::new(),
        }
    }

    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.insert(entity.id, entity);
    }

    /// Add a relation, or aggregate support if the triple already exists.
    ///
    /// The same `(head, relation, tail)` extracted from multiple papers must
    /// not produce parallel duplicate edges: it increments `support_count`,
    /// merges evidence and raises confidence via
    /// [`Relation::confidence_from_support`]. An explicitly supplied higher
    /// confidence (external KG scores) is kept.
    pub fn add_relation(&mut self, relation: Relation) {
        let key = (relation.from_id, relation.relation_type.clone(), relation.to_id);
        if let Some(&idx) = self.edge_index.get(&key) {
            let existing = &mut self.relations[idx];
            let support = match relation.source_paper_id {
                Some(pid) => {
                    if !existing.supporting_papers.contains(&pid) {
                        existing.supporting_papers.push(pid);
                    }
                    existing.support_count.max(existing.supporting_papers.len())
                }
                // External merge without paper attribution: external.rs
                // pre-deduplicates, so reaching here means a new source.
                None => existing.support_count + 1,
            };
            existing.support_count = support;
            let aggregated = Relation::confidence_from_support(support);
            existing.confidence = existing.confidence.max(aggregated);
            if !relation.evidence.is_empty()
                && !existing.evidence.contains(&relation.evidence)
                && existing.evidence.len() < 2000
            {
                existing.evidence.push_str(" | ");
                existing.evidence.push_str(&relation.evidence);
            }
            let confidence = existing.confidence;
            let (from, to, rt) = (existing.from_id, existing.to_id, existing.relation_type.clone());
            self.update_adjacency_confidence(from, to, &rt, confidence);
            return;
        }

        let mut relation = relation;
        if relation.support_count == 0 {
            relation.support_count = 1;
        }
        if let Some(pid) = relation.source_paper_id {
            if !relation.supporting_papers.contains(&pid) {
                relation.supporting_papers.push(pid);
            }
        }
        self.edge_index.insert(key, self.relations.len());
        self.outgoing
            .entry(relation.from_id)
            .or_default()
            .push((relation.relation_type.clone(), relation.to_id, relation.confidence));
        self.incoming
            .entry(relation.to_id)
            .or_default()
            .push((relation.relation_type.clone(), relation.from_id, relation.confidence));
        self.relations.push(relation);
    }

    fn update_adjacency_confidence(
        &mut self,
        from: EntityId,
        to: EntityId,
        rel_type: &RelationType,
        confidence: f64,
    ) {
        if let Some(edges) = self.outgoing.get_mut(&from) {
            for (r, t, c) in edges.iter_mut() {
                if r == rel_type && *t == to {
                    *c = confidence;
                }
            }
        }
        if let Some(edges) = self.incoming.get_mut(&to) {
            for (r, f, c) in edges.iter_mut() {
                if r == rel_type && *f == from {
                    *c = confidence;
                }
            }
        }
    }

    /// Number of distinct supporting sources for a triple (0 if absent).
    pub fn edge_support(&self, head: &EntityId, rel_type: &RelationType, tail: &EntityId) -> usize {
        self.edge_index
            .get(&(*head, rel_type.clone(), *tail))
            .map(|&i| self.relations[i].support_count)
            .unwrap_or(0)
    }

    /// Cross-source-safe merge for one triple: unlike [`add_relation`],
    /// support is combined with `max` instead of `+` so merging the same
    /// corpus twice (e.g. from the persistent store) cannot inflate the
    /// count. New edges are inserted as-is.
    pub fn merge_relation(&mut self, relation: Relation) {
        let key = (relation.from_id, relation.relation_type.clone(), relation.to_id);
        let Some(&idx) = self.edge_index.get(&key) else {
            self.add_relation(relation);
            return;
        };
        let existing = &mut self.relations[idx];
        existing.support_count = existing.support_count.max(relation.support_count.max(1));
        existing.confidence = existing
            .confidence
            .max(relation.confidence)
            .max(Relation::confidence_from_support(existing.support_count));
        if !relation.evidence.is_empty()
            && !existing.evidence.contains(&relation.evidence)
            && existing.evidence.len() < 2000
        {
            existing.evidence.push_str(" | ");
            existing.evidence.push_str(&relation.evidence);
        }
        let confidence = existing.confidence;
        let (from, to, rt) = (existing.from_id, existing.to_id, existing.relation_type.clone());
        self.update_adjacency_confidence(from, to, &rt, confidence);
    }

    pub fn entity_count(&self) -> usize { self.entities.len() }
    pub fn relation_count(&self) -> usize { self.relations.len() }

    pub fn get_entity(&self, id: &EntityId) -> Option<&Entity> {
        self.entities.get(id)
    }

    pub fn get_entity_mut(&mut self, id: &EntityId) -> Option<&mut Entity> {
        self.entities.get_mut(id)
    }

    pub fn find_entity_by_name(&self, name: &str) -> Option<&Entity> {
        let lower = name.to_lowercase();
        self.entities.values().find(|e| {
            e.name.to_lowercase() == lower
                || e.aliases.iter().any(|a| a.to_lowercase() == lower)
        })
    }

    /// Query all tails for (head, relation_type)
    pub fn query_tails(&self, head: &EntityId, rel_type: &RelationType) -> Vec<&EntityId> {
        self.outgoing
            .get(head)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|(r, _, _)| r == rel_type)
                    .map(|(_, target, _)| target)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Query all heads for (relation_type, tail)
    pub fn query_heads(&self, rel_type: &RelationType, tail: &EntityId) -> Vec<&EntityId> {
        self.incoming
            .get(tail)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|(r, _, _)| r == rel_type)
                    .map(|(_, source, _)| source)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn contains_edge(&self, head: &EntityId, rel_type: &RelationType, tail: &EntityId) -> bool {
        self.outgoing
            .get(head)
            .is_some_and(|edges| edges.iter().any(|(r, t, _)| r == rel_type && t == tail))
    }

    /// Find all paths between two entities (BFS, max depth)
    pub fn find_paths(
        &self,
        from: &EntityId,
        to: &EntityId,
        max_depth: usize,
    ) -> Vec<Vec<(EntityId, RelationType, EntityId)>> {
        let mut all_paths = Vec::new();
        let mut visited = HashSet::new();
        let mut current_path = Vec::new();

        self.dfs_paths(*from, *to, max_depth, &mut visited, &mut current_path, &mut all_paths);
        all_paths
    }

    fn dfs_paths(
        &self,
        current: EntityId,
        target: EntityId,
        max_depth: usize,
        visited: &mut HashSet<EntityId>,
        path: &mut Vec<(EntityId, RelationType, EntityId)>,
        all_paths: &mut Vec<Vec<(EntityId, RelationType, EntityId)>>,
    ) {
        if current == target && !path.is_empty() {
            all_paths.push(path.clone());
            return;
        }
        if path.len() >= max_depth { return; }

        visited.insert(current);

        if let Some(edges) = self.outgoing.get(&current) {
            for (rel_type, next, _) in edges {
                if !visited.contains(next) || *next == target {
                    path.push((current, rel_type.clone(), *next));
                    self.dfs_paths(*next, target, max_depth, visited, path, all_paths);
                    path.pop();
                }
            }
        }

        visited.remove(&current);
    }

    pub fn all_entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    pub fn all_relations(&self) -> &[Relation] {
        &self.relations
    }

    /// Get neighborhood of an entity (1 hop)
    pub fn neighborhood(&self, id: &EntityId) -> Vec<(RelationType, EntityId, f64)> {
        let mut neighbors = Vec::new();
        if let Some(out) = self.outgoing.get(id) {
            neighbors.extend(out.iter().cloned());
        }
        if let Some(in_edges) = self.incoming.get(id) {
            neighbors.extend(in_edges.iter().cloned());
        }
        neighbors
    }
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}
