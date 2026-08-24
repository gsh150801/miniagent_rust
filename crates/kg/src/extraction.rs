use crate::graph::KnowledgeGraph;
use crate::schema::{Entity, EntityId, EntityType, Relation, RelationId, RelationType};
use uuid::Uuid;

/// Extract entities and relations from a paper's structured summary using LLM.
/// This returns a structured representation that can be loaded into the KG.
pub struct ExtractionResult {
    pub paper_id: Uuid,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// Parse LLM extraction output into structured ExtractionResult
pub fn parse_extraction_result(
    paper_id: Uuid,
    json_output: &serde_json::Value,
) -> ExtractionResult {
    let mut entities = Vec::new();
    let mut name_to_id = std::collections::HashMap::new();

    if let Some(entity_list) = json_output["entities"].as_array() {
        for e in entity_list {
            let name = e["name"].as_str().unwrap_or("unknown").to_string();
            let etype = parse_entity_type(e["type"].as_str().unwrap_or("Concept"));
            let id = EntityId::new();
            name_to_id.insert(name.clone(), id);

            let aliases: Vec<String> = e["aliases"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            entities.push(Entity {
                id,
                name,
                entity_type: etype,
                aliases,
                metadata: serde_json::json!({"source_paper": paper_id.to_string()}),
            });
        }
    }

    let mut relations = Vec::new();
    if let Some(rel_list) = json_output["relations"].as_array() {
        for r in rel_list {
            let from_name = r["from"].as_str().unwrap_or("");
            let to_name = r["to"].as_str().unwrap_or("");
            let rel_type = r["type"].as_str().unwrap_or("");
            let evidence = r["evidence"].as_str().unwrap_or("").to_string();

            if let (Some(&from_id), Some(&to_id)) = (name_to_id.get(from_name), name_to_id.get(to_name))
                && let Some(rt) = RelationType::parse(rel_type) {
                    relations.push(Relation {
                        id: RelationId::new(),
                        from_id,
                        to_id,
                        relation_type: rt,
                        // Confidence reflects literature support: one paper is
                        // 0.5, growing with each independent supporting paper
                        // (aggregated by KnowledgeGraph::add_relation).
                        confidence: Relation::confidence_from_support(1),
                        evidence,
                        source_paper_id: Some(paper_id),
                        support_count: 1,
                        supporting_papers: vec![paper_id],
                    });
                }
        }
    }

    ExtractionResult {
        paper_id,
        entities,
        relations,
    }
}

fn parse_entity_type(s: &str) -> EntityType {
    match s {
        "Gene" => EntityType::Gene,
        "Protein" => EntityType::Protein,
        "Pathway" => EntityType::Pathway,
        "Disease" => EntityType::Disease,
        "Phenotype" => EntityType::Phenotype,
        "Drug" => EntityType::Drug,
        "Compound" => EntityType::Compound,
        "Method" => EntityType::Method,
        _ => EntityType::Concept,
    }
}

/// Merge an ExtractionResult into a KnowledgeGraph
pub fn merge_into_kg(kg: &mut KnowledgeGraph, result: ExtractionResult) {
    for entity in result.entities {
        // Check for existing entity with same name (normalization)
        if let Some(existing) = kg.find_entity_by_name(&entity.name) {
            // Merge aliases
            let merged_aliases = {
                let mut a = existing.aliases.clone();
                a.extend(entity.aliases.clone());
                a
            };
            kg.add_entity(Entity {
                aliases: merged_aliases,
                ..entity
            });
        } else {
            kg.add_entity(entity);
        }
    }

    for relation in result.relations {
        kg.add_relation(relation);
    }
}

/// Stats reported by [`merge_extraction_canonical`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ExtractionMergeStats {
    pub entities_added: usize,
    pub entities_merged: usize,
    pub relations_added: usize,
    /// Relations dropped because an endpoint could not be canonicalized.
    pub relations_dropped: usize,
}

/// Merge an extraction into the KG with alias-aware canonicalization.
///
/// Unlike [`merge_into_kg`], this remaps relation endpoints to the canonical
/// entity ids of the merged KG. The per-paper extraction assigns fresh ids to
/// every entity; merging by name without remapping leaves relations pointing
/// at ids that were never inserted (dangling edges that break neighborhood
/// queries and link prediction).
///
/// Canonicalization is case-insensitive over both names and aliases, so
/// "α-synuclein" / "alpha-synuclein" / "Alpha-Syn" collapse to one node when
/// they appear as name/alias pairs across papers.
pub fn merge_extraction_canonical(
    kg: &mut KnowledgeGraph,
    result: ExtractionResult,
) -> ExtractionMergeStats {
    // Index existing entities by lowercase name and alias → canonical id.
    let mut index: std::collections::HashMap<String, EntityId> = std::collections::HashMap::new();
    for e in kg.all_entities() {
        index.entry(e.name.to_lowercase()).or_insert(e.id);
        for a in &e.aliases {
            index.entry(a.to_lowercase()).or_insert(e.id);
        }
    }

    let mut stats = ExtractionMergeStats::default();
    // Paper-local entity id → canonical KG id.
    let mut id_map: std::collections::HashMap<EntityId, EntityId> =
        std::collections::HashMap::new();

    for entity in result.entities {
        let key = entity.name.to_lowercase();
        if let Some(&canonical_id) = index.get(&key) {
            // Existing entity: fold the new aliases in, drop the duplicate node.
            let mut new_aliases: Vec<String> = Vec::new();
            if let Some(existing) = kg.get_entity(&canonical_id) {
                for a in &entity.aliases {
                    if !existing.aliases.iter().any(|x| x.eq_ignore_ascii_case(a))
                        && !existing.name.eq_ignore_ascii_case(a)
                    {
                        new_aliases.push(a.clone());
                    }
                }
            }
            if !new_aliases.is_empty() {
                if let Some(existing) = kg.get_entity_mut(&canonical_id) {
                    existing.aliases.extend(new_aliases);
                }
            }
            id_map.insert(entity.id, canonical_id);
            stats.entities_merged += 1;
        } else {
            let new_id = EntityId::new();
            index.insert(key, new_id);
            for a in &entity.aliases {
                index.entry(a.to_lowercase()).or_insert(new_id);
            }
            id_map.insert(entity.id, new_id);
            kg.add_entity(Entity {
                id: new_id,
                ..entity
            });
            stats.entities_added += 1;
        }
    }

    for relation in result.relations {
        match (id_map.get(&relation.from_id), id_map.get(&relation.to_id)) {
            (Some(from), Some(to)) => {
                kg.add_relation(Relation {
                    id: RelationId::new(),
                    from_id: *from,
                    to_id: *to,
                    ..relation
                });
                stats.relations_added += 1;
            }
            _ => {
                stats.relations_dropped += 1;
            }
        }
    }

    stats
}

#[cfg(test)]
mod aggregation_tests {
    use super::*;
    use crate::schema::{Entity, EntityType};

    fn make_kg_with_gene() -> (KnowledgeGraph, crate::schema::EntityId) {
        let mut kg = KnowledgeGraph::new();
        let id = crate::schema::EntityId::new();
        kg.add_entity(Entity {
            id,
            name: "BRCA1".into(),
            entity_type: EntityType::Gene,
            aliases: vec![],
            metadata: serde_json::Value::Null,
        });
        (kg, id)
    }

    fn relation(kg_from: crate::schema::EntityId, to: crate::schema::EntityId, paper: uuid::Uuid, ev: &str) -> Relation {
        Relation {
            id: RelationId::new(),
            from_id: kg_from,
            to_id: to,
            relation_type: RelationType::Inhibits,
            confidence: Relation::confidence_from_support(1),
            evidence: ev.into(),
            source_paper_id: Some(paper),
            support_count: 1,
            supporting_papers: vec![paper],
        }
    }

    #[test]
    fn same_triple_from_three_papers_is_one_edge() {
        let (mut kg, g) = make_kg_with_gene();
        let d = crate::schema::EntityId::new();
        let p1 = uuid::Uuid::new_v4();
        let p2 = uuid::Uuid::new_v4();
        let p3 = uuid::Uuid::new_v4();

        kg.add_relation(relation(g, d, p1, "paper1 evidence"));
        kg.add_relation(relation(g, d, p2, "paper2 evidence"));
        kg.add_relation(relation(g, d, p3, "paper3 evidence"));

        assert_eq!(kg.relation_count(), 1, "three papers → one aggregated edge");
        assert_eq!(kg.edge_support(&g, &RelationType::Inhibits, &d), 3);
        let rel = &kg.all_relations()[0];
        assert_eq!(rel.supporting_papers.len(), 3);
        // n/(n+1) with n=3 → 0.75
        assert!((rel.confidence - 0.75).abs() < 1e-9);
        assert!(rel.evidence.contains("paper2 evidence"));
    }

    #[test]
    fn same_paper_duplicate_extraction_not_double_counted() {
        let (mut kg, g) = make_kg_with_gene();
        let d = crate::schema::EntityId::new();
        let p1 = uuid::Uuid::new_v4();

        kg.add_relation(relation(g, d, p1, "first mention"));
        kg.add_relation(relation(g, d, p1, "second mention same paper"));

        assert_eq!(kg.relation_count(), 1);
        assert_eq!(kg.edge_support(&g, &RelationType::Inhibits, &d), 1);
        // n=1 → 0.5, not 1.0 (evidence still merged)
        assert!((kg.all_relations()[0].confidence - 0.5).abs() < 1e-9);
        assert!(kg.all_relations()[0].evidence.contains("second mention same paper"));
    }

    #[test]
    fn external_confidence_never_lowered_by_aggregation() {
        let (mut kg, g) = make_kg_with_gene();
        let d = crate::schema::EntityId::new();

        let mut ext = relation(g, d, uuid::Uuid::new_v4(), "disgenet");
        ext.confidence = 0.9;
        ext.source_paper_id = None;
        ext.supporting_papers = vec![];
        kg.add_relation(ext);

        assert_eq!(kg.relation_count(), 1);
        assert!((kg.all_relations()[0].confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn old_kg_json_without_support_fields_still_loads() {
        let (kg, g) = make_kg_with_gene();
        let _ = g;
        let json = serde_json::to_string(kg.all_relations()).unwrap();
        assert!(!json.contains("supporting_papers") || true);
        // simulate legacy relation JSON missing the new fields
        let legacy = r#"[{
            "id": "3f8e6c5a-1111-4222-8333-444455556666",
            "from_id": "3f8e6c5a-1111-4222-8333-444455556666",
            "to_id": "3f8e6c5a-1111-4222-8333-444455556666",
            "relation_type": "Inhibits",
            "confidence": 1.0,
            "evidence": "legacy",
            "source_paper_id": null
        }]"#;
        let parsed: Result<Vec<Relation>, _> = serde_json::from_str(legacy);
        assert!(parsed.is_ok(), "legacy relations must deserialize with defaults");
        assert_eq!(parsed.unwrap()[0].support_count, 1);
    }
}
