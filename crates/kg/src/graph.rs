use std::collections::{HashMap, HashSet};
use crate::schema::{Entity, EntityId, Relation, RelationType};

pub struct KnowledgeGraph {
    entities: HashMap<EntityId, Entity>,
    // Adjacency list: entity -> list of (relation, target, confidence)
    outgoing: HashMap<EntityId, Vec<(RelationType, EntityId, f64)>>,
    incoming: HashMap<EntityId, Vec<(RelationType, EntityId, f64)>>,
    relations: Vec<Relation>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            relations: Vec::new(),
        }
    }

    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.insert(entity.id, entity);
    }

    pub fn add_relation(&mut self, relation: Relation) {
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

    pub fn entity_count(&self) -> usize { self.entities.len() }
    pub fn relation_count(&self) -> usize { self.relations.len() }

    pub fn get_entity(&self, id: &EntityId) -> Option<&Entity> {
        self.entities.get(id)
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
