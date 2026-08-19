//! Persistent cross-project knowledge-graph store.
//!
//! Every research run rebuilds its KG from scratch today; the store lets
//! corpus knowledge accumulate across runs (`kg_store.json` at the workspace
//! root by default, override with `KG_STORE_PATH`). Merging is entity-name
//! canonical (case-insensitive over names + aliases) and uses
//! [`KnowledgeGraph::merge_relation`], whose max-combining keeps repeated
//! merges of the same corpus from inflating support counts.

use crate::graph::KnowledgeGraph;
use crate::schema::{Entity, EntityId, Relation};
use std::collections::HashMap;
use std::path::PathBuf;

/// Serialization DTO (mirrors the CLI's kg.json dump shape).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct StoreDump {
    #[serde(default)]
    entities: Vec<Entity>,
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StoreMergeStats {
    pub entities_added: usize,
    pub entities_merged: usize,
    pub relations_added: usize,
    pub relations_merged: usize,
    /// Relations dropped because an endpoint name could not be interned
    /// (cannot happen for whole-graph merges — kept for audit symmetry).
    pub relations_dropped: usize,
}

pub struct KgStore {
    kg: KnowledgeGraph,
    path: PathBuf,
}

impl KgStore {
    /// Load the store from `path` (empty store when the file is missing or
    /// corrupt — a corrupt store degrades to a fresh one rather than
    /// poisoning runs).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut kg = KnowledgeGraph::new();
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(dump) = serde_json::from_slice::<StoreDump>(&bytes)
        {
            for e in dump.entities {
                kg.add_entity(e);
            }
            for r in dump.relations {
                kg.add_relation(r);
            }
        }
        Self { kg, path }
    }

    pub fn knowledge_graph(&self) -> &KnowledgeGraph {
        &self.kg
    }

    /// Merge a project KG into the store. Entities canonicalize by
    /// lowercase name/alias; relations follow their endpoints.
    pub fn merge(&mut self, other: &KnowledgeGraph) -> StoreMergeStats {
        let mut index: HashMap<String, EntityId> = HashMap::new();
        for e in self.kg.all_entities() {
            index.entry(e.name.to_lowercase()).or_insert(e.id);
            for a in &e.aliases {
                index.entry(a.to_lowercase()).or_insert(e.id);
            }
        }

        let mut stats = StoreMergeStats::default();
        let mut id_map: HashMap<EntityId, EntityId> = HashMap::new();
        for entity in other.all_entities() {
            let key = entity.name.to_lowercase();
            if let Some(&canonical) = index.get(&key) {
                // Fold new aliases into the canonical entity.
                let new_aliases: Vec<String> = entity
                    .aliases
                    .iter()
                    .filter(|a| {
                        !index.contains_key(&a.to_lowercase())
                    })
                    .cloned()
                    .collect();
                if !new_aliases.is_empty() {
                    if let Some(existing) = self.kg.get_entity_mut(&canonical) {
                        for a in new_aliases {
                            index.entry(a.to_lowercase()).or_insert(canonical);
                            existing.aliases.push(a);
                        }
                    }
                }
                id_map.insert(entity.id, canonical);
                stats.entities_merged += 1;
            } else {
                let new_id = EntityId::new();
                index.insert(key, new_id);
                for a in &entity.aliases {
                    index.entry(a.to_lowercase()).or_insert(new_id);
                }
                id_map.insert(entity.id, new_id);
                self.kg.add_entity(Entity {
                    id: new_id,
                    name: entity.name.clone(),
                    entity_type: entity.entity_type.clone(),
                    aliases: entity.aliases.clone(),
                    metadata: entity.metadata.clone(),
                });
                stats.entities_added += 1;
            }
        }

        for rel in other.all_relations() {
            match (id_map.get(&rel.from_id), id_map.get(&rel.to_id)) {
                (Some(from), Some(to)) => {
                    let existed = self.kg.edge_support(from, &rel.relation_type, to) > 0;
                    self.kg.merge_relation(Relation {
                        id: crate::schema::RelationId::new(),
                        from_id: *from,
                        to_id: *to,
                        relation_type: rel.relation_type.clone(),
                        confidence: rel.confidence,
                        evidence: rel.evidence.clone(),
                        source_paper_id: None,
                        // Cross-run paper UUIDs are unstable; carry the count
                        // only (merge_relation combines with max).
                        support_count: rel.support_count,
                        supporting_papers: vec![],
                    });
                    if existed {
                        stats.relations_merged += 1;
                    } else {
                        stats.relations_added += 1;
                    }
                }
                _ => stats.relations_dropped += 1,
            }
        }
        stats
    }

    /// Persist the store atomically (tmp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        let dump = StoreDump {
            entities: self.kg.all_entities().cloned().collect(),
            relations: self.kg.all_relations().to_vec(),
        };
        let json = serde_json::to_vec(&dump)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{EntityType, RelationType};

    fn entity(name: &str) -> Entity {
        Entity {
            id: EntityId::new(),
            name: name.into(),
            entity_type: EntityType::Gene,
            aliases: vec![],
            metadata: serde_json::Value::Null,
        }
    }

    fn kg_with(edge: (String, String), support: usize) -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::new();
        let a = entity(&edge.0);
        let b = entity(&edge.1);
        let (a_id, b_id) = (a.id, b.id);
        kg.add_entity(a);
        kg.add_entity(b);
        kg.add_relation(Relation {
            id: crate::schema::RelationId::new(),
            from_id: a_id,
            to_id: b_id,
            relation_type: RelationType::AssociatedWith,
            confidence: Relation::confidence_from_support(support),
            evidence: "ev".into(),
            source_paper_id: None,
            support_count: support,
            supporting_papers: vec![],
        });
        kg
    }

    #[test]
    fn merge_accumulates_entities_and_edges() {
        let mut store = KgStore { kg: KnowledgeGraph::new(), path: PathBuf::from("/dev/null") };
        let s1 = store.merge(&kg_with(("BRCA1".into(), "Breast cancer".into()), 2));
        assert_eq!((s1.entities_added, s1.relations_added), (2, 1));
        let s2 = store.merge(&kg_with(("BRCA1".into(), "Ovarian cancer".into()), 1));
        assert_eq!((s2.entities_added, s2.entities_merged), (1, 1));
        assert_eq!(store.kg.relation_count(), 2);
        assert_eq!(store.kg.entity_count(), 3);
    }

    #[test]
    fn merging_same_corpus_twice_does_not_inflate_support() {
        let mut store = KgStore { kg: KnowledgeGraph::new(), path: PathBuf::from("/dev/null") };
        let kg = kg_with(("APOE".into(), "Alzheimer".into()), 3);
        store.merge(&kg);
        store.merge(&kg);
        store.merge(&kg);
        assert_eq!(store.kg.relation_count(), 1);
        assert_eq!(store.kg.all_relations()[0].support_count, 3, "max-combined, not summed");
    }

    #[test]
    fn alias_canonicalization_collapses_entities() {
        let mut kg1 = KnowledgeGraph::new();
        let mut e = entity("α-synuclein");
        e.aliases = vec!["SNCA".into(), "Alpha-syn".into()];
        kg1.add_entity(e);
        let mut store = KgStore { kg: KnowledgeGraph::new(), path: PathBuf::from("/dev/null") };
        store.merge(&kg1);

        let mut kg2 = KnowledgeGraph::new();
        kg2.add_entity(entity("SNCA"));
        let s = store.merge(&kg2);
        assert_eq!(s.entities_merged, 1);
        assert_eq!(store.kg.entity_count(), 1);
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mn_kg_store_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        let mut store = KgStore::load(&path);
        store.merge(&kg_with(("TP53".into(), "Glioma".into()), 2));
        store.save().unwrap();
        let reloaded = KgStore::load(&path);
        assert_eq!(reloaded.knowledge_graph().relation_count(), 1);
        assert_eq!(reloaded.knowledge_graph().entity_count(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
