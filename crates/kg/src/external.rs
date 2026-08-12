//! External biomedical knowledge-graph enrichment.
//!
//! Merges triples from external sources (DisGeNET gene–disease exports, STRING
//! protein–protein interactions, OMIM, or any local TSV) into the in-memory
//! [`KnowledgeGraph`]. This broadens link prediction beyond the small graph
//! extracted from PubMed abstracts, giving the hypothesis generator a richer
//! substrate of known biology to reason over.
//!
//! Design: parsing/merging is pure (no I/O side effects beyond the graph
//! mutation); the only network call is the STRING API fetcher, kept here for
//! cohesion. All HTTP is `#[ignore]`-tested so the suite stays offline.

use crate::graph::KnowledgeGraph;
use crate::schema::{Entity, EntityType, Relation, RelationId, RelationType};
use serde::{Deserialize, Serialize};

/// A triple sourced from an external biomedical KG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTriple {
    pub head_name: String,
    pub head_type: EntityType,
    pub relation: RelationType,
    pub tail_name: String,
    pub tail_type: EntityType,
    /// Association confidence in `[0, 1]` from the source (used as edge weight).
    pub confidence: f64,
    /// Provenance label (e.g. `"DisGeNET"`, `"STRING"`, file path).
    pub source: String,
}

/// Result of merging external triples into a graph.
#[derive(Debug, Clone, Default)]
pub struct MergeStats {
    pub triples_in: usize,
    pub edges_added: usize,
    pub entities_created: usize,
    pub edges_skipped_duplicate: usize,
}

impl MergeStats {
    fn note_created(&mut self) {
        self.entities_created += 1;
    }
}

// ── TSV / CSV loading ──────────────────────────────────────────────────────

/// Load triples from a delimited file where each record is
/// `head <delim> relation <delim> tail [<delim> score]`.
///
/// `relation` is parsed per-row via [`RelationType::parse`]; rows with an
/// unparseable relation are skipped (with a `tracing::warn!`). `score` is
/// optional and clamped to `[0, 1]`.
pub fn load_relation_tsv(
    path: &str,
    delimiter: char,
    head_type: EntityType,
    tail_type: EntityType,
    source: &str,
) -> Result<Vec<ExternalTriple>, std::io::Error> {
    let body = std::fs::read_to_string(path)?;
    Ok(parse_relation_records(
        &body,
        delimiter,
        head_type,
        tail_type,
        source,
    ))
}

/// Load triples from a delimited file where each record is
/// `head <delim> tail [<delim> score]` and the relation is fixed for all rows.
pub fn load_fixed_relation_tsv(
    path: &str,
    delimiter: char,
    head_type: EntityType,
    relation: RelationType,
    tail_type: EntityType,
    source: &str,
) -> Result<Vec<ExternalTriple>, std::io::Error> {
    let body = std::fs::read_to_string(path)?;
    Ok(parse_fixed_relation_records(
        &body,
        delimiter,
        head_type,
        relation,
        tail_type,
        source,
    ))
}

fn parse_relation_records(
    body: &str,
    delimiter: char,
    head_type: EntityType,
    tail_type: EntityType,
    source: &str,
) -> Vec<ExternalTriple> {
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split(delimiter).map(|c| c.trim()).collect();
        // Skip an obvious header row on the first line.
        if i == 0 && cols.len() >= 3 && cols.iter().take(3).all(|c| looks_like_header(c)) {
            continue;
        }
        if cols.len() < 3 {
            tracing::warn!(line = i, "skipping short record");
            continue;
        }
        let Some(rel) = RelationType::parse(cols[1]) else {
            tracing::warn!(line = i, relation = cols[1], "unparseable relation, skipping");
            continue;
        };
        out.push(ExternalTriple {
            head_name: cols[0].to_string(),
            head_type: head_type.clone(),
            relation: rel,
            tail_name: cols[2].to_string(),
            tail_type: tail_type.clone(),
            confidence: parse_score(cols.get(3).copied()),
            source: source.to_string(),
        });
    }
    out
}

fn parse_fixed_relation_records(
    body: &str,
    delimiter: char,
    head_type: EntityType,
    relation: RelationType,
    tail_type: EntityType,
    source: &str,
) -> Vec<ExternalTriple> {
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split(delimiter).map(|c| c.trim()).collect();
        if i == 0 && cols.len() >= 2 && cols.iter().take(2).all(|c| looks_like_header(c)) {
            continue;
        }
        if cols.len() < 2 {
            tracing::warn!(line = i, "skipping short record");
            continue;
        }
        out.push(ExternalTriple {
            head_name: cols[0].to_string(),
            head_type: head_type.clone(),
            relation: relation.clone(),
            tail_name: cols[1].to_string(),
            tail_type: tail_type.clone(),
            confidence: parse_score(cols.get(2).copied()),
            source: source.to_string(),
        });
    }
    out
}

fn looks_like_header(s: &str) -> bool {
    let lower = s.to_lowercase();
    matches!(
        lower.as_str(),
        "gene" | "genesymbol" | "gene_symbol" | "disease" | "diseasename" | "protein"
            | "head" | "tail" | "relation" | "score" | "confidence" | "source" | "a" | "b"
    )
}

fn parse_score(raw: Option<&str>) -> f64 {
    let Some(s) = raw else { return 0.5 };
    let v: f64 = s.parse().unwrap_or(0.5);
    v.clamp(0.0, 1.0)
}

// ── STRING API ─────────────────────────────────────────────────────────────

/// Build the STRING `network` endpoint URL for a set of identifiers.
///
/// `species` is the NCBI taxon ID (9606 = human). `required_score` is STRING's
/// combined score threshold (0–1000; 400 = medium confidence, 700 = high).
pub fn string_network_url(genes: &[String], species: u32, required_score: u32) -> String {
    let identifiers = genes.join("%0d"); // STRING accepts newline (%0d) separators
    format!(
        "https://string-db.org/api/tsv/network?identifiers={identifiers}&species={species}&required_score={required_score}"
    )
}

/// Parse a STRING `network` TSV response into interaction triples.
///
/// STRING columns include `preferredName_A`, `preferredName_B`, and `score`
/// (combined score in `[0, 1]`). Unknown column layouts fall back to positional
/// columns 2 (A), 3 (B), 5 (score).
pub fn parse_string_response(body: &str, score_threshold: f64) -> Vec<ExternalTriple> {
    let mut lines = body.lines();
    let header = lines.next().unwrap_or("");
    let cols: Vec<&str> = header.split('\t').collect();

    let (ia, ib, iscore) = locate_string_cols(&cols);

    let mut out = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let (Some(&a_idx), Some(&b_idx)) = (ia.as_ref(), ib.as_ref()) else {
            continue;
        };
        if a_idx >= fields.len() || b_idx >= fields.len() {
            continue;
        }
        let a = fields[a_idx].trim();
        let b = fields[b_idx].trim();
        if a.is_empty() || b.is_empty() {
            continue;
        }
        let score = iscore
            .and_then(|idx| fields.get(idx))
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.5);
        if score < score_threshold {
            continue;
        }
        out.push(ExternalTriple {
            head_name: a.to_string(),
            head_type: EntityType::Protein,
            relation: RelationType::InteractsWith,
            tail_name: b.to_string(),
            tail_type: EntityType::Protein,
            confidence: score,
            source: "STRING".to_string(),
        });
    }
    out
}

fn locate_string_cols(cols: &[&str]) -> (Option<usize>, Option<usize>, Option<usize>) {
    let mut a = None;
    let mut b = None;
    let mut s = None;
    for (i, c) in cols.iter().enumerate() {
        let lower = c.trim().to_lowercase().trim_start_matches('#').to_string();
        match lower.as_str() {
            "preferredname_a" | "stringid_a" | "a" => a = Some(i),
            "preferredname_b" | "stringid_b" | "b" => b = Some(i),
            "score" => s = Some(i),
            _ => {}
        }
    }
    // Fallbacks for the canonical STRING network layout.
    (Some(a.unwrap_or(2)), Some(b.unwrap_or(3)), s.or(Some(5)))
}

/// Fetch protein–protein interactions from STRING for the given gene/protein
/// identifiers and return them as interaction triples.
pub async fn fetch_string_interactions(
    client: &reqwest::Client,
    genes: &[String],
    species: u32,
    required_score: u32,
    score_threshold: f64,
) -> Result<Vec<ExternalTriple>, reqwest::Error> {
    if genes.is_empty() {
        return Ok(Vec::new());
    }
    let url = string_network_url(genes, species, required_score);
    let resp = client.get(&url).send().await?.error_for_status()?;
    let body = resp.text().await?;
    Ok(parse_string_response(&body, score_threshold))
}

// ── Merge into KnowledgeGraph ──────────────────────────────────────────────

/// Merge external triples into `kg`, deduplicating entities by (case-insensitive)
/// name and skipping edges that already exist.
pub fn merge_external(kg: &mut KnowledgeGraph, triples: &[ExternalTriple]) -> MergeStats {
    let mut stats = MergeStats {
        triples_in: triples.len(),
        ..Default::default()
    };

    for t in triples {
        let head_id = intern_entity(kg, &t.head_name, &t.head_type, &mut stats);
        let tail_id = intern_entity(kg, &t.tail_name, &t.tail_type, &mut stats);

        if kg.contains_edge(&head_id, &t.relation, &tail_id) {
            stats.edges_skipped_duplicate += 1;
            continue;
        }
        kg.add_relation(Relation {
            id: RelationId::new(),
            from_id: head_id,
            to_id: tail_id,
            relation_type: t.relation.clone(),
            confidence: t.confidence,
            evidence: format!("external:{}", t.source),
            source_paper_id: None,
        });
        stats.edges_added += 1;
    }

    stats
}

/// Find an existing entity by name (case-insensitive, incl. aliases) or create one.
fn intern_entity(
    kg: &mut KnowledgeGraph,
    name: &str,
    entity_type: &EntityType,
    stats: &mut MergeStats,
) -> crate::schema::EntityId {
    if let Some(existing) = kg.find_entity_by_name(name) {
        return existing.id;
    }
    let entity = Entity {
        id: crate::schema::EntityId::new(),
        name: name.to_string(),
        entity_type: entity_type.clone(),
        aliases: Vec::new(),
        metadata: serde_json::json!({ "source": "external" }),
    };
    let id = entity.id;
    kg.add_entity(entity);
    stats.note_created();
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::EntityId;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("miniagent_kg_external_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn load_fixed_relation_tsv_parses_gene_disease() {
        let path = write_temp(
            "disgenet.tsv",
            "gene\tdisease\tscore\nBRCA1\tbreast cancer\t0.9\nTP53\tLi-Fraumeni syndrome\t0.8\n",
        );
        let path_str = path.to_str().unwrap();
        let triples = load_fixed_relation_tsv(
            path_str,
            '\t',
            EntityType::Gene,
            RelationType::AssociatedWith,
            EntityType::Disease,
            "DisGeNET",
        )
        .unwrap();
        assert_eq!(triples.len(), 2);
        assert_eq!(triples[0].head_name, "BRCA1");
        assert_eq!(triples[0].tail_name, "breast cancer");
        assert_eq!(triples[0].relation, RelationType::AssociatedWith);
        assert!((triples[0].confidence - 0.9).abs() < 1e-9);
        assert_eq!(triples[0].source, "DisGeNET");
        // header row skipped
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_relation_tsv_parses_per_row_relation() {
        let path = write_temp(
            "mixed.tsv",
            "BRCA1\tactivates\tCHEK2\nMDM2\tinhibits\tTP53\n",
        );
        let triples = load_relation_tsv(
            path.to_str().unwrap(),
            '\t',
            EntityType::Gene,
            EntityType::Gene,
            "custom",
        )
        .unwrap();
        assert_eq!(triples.len(), 2);
        assert_eq!(triples[0].relation, RelationType::Activates);
        assert_eq!(triples[1].relation, RelationType::Inhibits);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parse_string_response_extracts_interactions() {
        let body = "#string-protein-id\t#string-protein-id\tpreferredName_A\tpreferredName_B\tncbiTaxonId\tscore\tnscore\n\
                    9606.ENSP00000001\t9606.ENSP00000002\tBRCA1\tBRCA2\t9606\t0.95\t0.9\n\
                    9606.ENSP00000003\t9606.ENSP00000004\tTP53\tMDM2\t9606\t0.30\t0.2\n";
        let triples = parse_string_response(body, 0.5);
        // Only the 0.95 interaction passes the threshold.
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].head_name, "BRCA1");
        assert_eq!(triples[0].tail_name, "BRCA2");
        assert_eq!(triples[0].relation, RelationType::InteractsWith);
        assert!((triples[0].confidence - 0.95).abs() < 1e-9);
    }

    #[test]
    fn merge_external_dedups_entities_and_edges() {
        let mut kg = KnowledgeGraph::new();
        // Pre-existing BRCA1 entity.
        let existing = Entity {
            id: EntityId::new(),
            name: "BRCA1".to_string(),
            entity_type: EntityType::Gene,
            aliases: vec![],
            metadata: serde_json::Value::Null,
        };
        kg.add_entity(existing);

        let triples = vec![
            ExternalTriple {
                head_name: "BRCA1".to_string(),
                head_type: EntityType::Gene,
                relation: RelationType::AssociatedWith,
                tail_name: "breast cancer".to_string(),
                tail_type: EntityType::Disease,
                confidence: 0.9,
                source: "DisGeNET".into(),
            },
            ExternalTriple {
                head_name: "BRCA1".to_string(),
                head_type: EntityType::Gene,
                relation: RelationType::AssociatedWith,
                tail_name: "breast cancer".to_string(),
                tail_type: EntityType::Disease,
                confidence: 0.8,
                source: "DisGeNET".into(),
            },
        ];
        let stats = merge_external(&mut kg, &triples);
        // BRCA1 reused (0 created for it), breast cancer created (1), second edge deduped.
        assert_eq!(stats.triples_in, 2);
        assert_eq!(stats.edges_added, 1);
        assert_eq!(stats.entities_created, 1);
        assert_eq!(stats.edges_skipped_duplicate, 1);
    }

    #[test]
    fn string_network_url_well_formed() {
        let url = string_network_url(
            &["BRCA1".to_string(), "TP53".to_string()],
            9606,
            400,
        );
        assert!(url.contains("identifiers=BRCA1%0dTP53"));
        assert!(url.contains("species=9606"));
        assert!(url.contains("required_score=400"));
    }
}
