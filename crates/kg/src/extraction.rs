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

/// Build a prompt for entity/relation extraction from a paper.
pub fn extraction_prompt(title: &str, abstract_text: &str, full_text: Option<&str>) -> String {
    let text = full_text.unwrap_or(abstract_text);
    format!(
        r#"Extract key entities and their relationships from the following scientific paper.

**Paper Title:** {title}
**Content:** {text}

Output a JSON object with:
1. "entities": list of objects with fields:
   - "name": canonical name
   - "type": one of [Gene, Protein, Pathway, Disease, Phenotype, Drug, Method, Concept]
   - "aliases": list of alternative names
2. "relations": list of objects with fields:
   - "from": entity name (must match an entity above)
   - "to": entity name
   - "type": one of [activates, inhibits, regulates, binds_to, phosphorylates, interacts_with, associated_with, correlated_with, uses_method, measured_by, is_a, part_of, located_in, supports, contradicts, extends]
   - "evidence": short quote or description supporting this relation

Focus on biologically/scientifically meaningful entities and experimentally supported relations.
"#
    )
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
                        confidence: 1.0,
                        evidence,
                        source_paper_id: Some(paper_id),
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
