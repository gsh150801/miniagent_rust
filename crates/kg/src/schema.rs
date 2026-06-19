use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationId(pub Uuid);

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}

impl Default for RelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl RelationId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub name: String,
    pub entity_type: EntityType,
    pub aliases: Vec<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityType {
    // Research objects
    Gene,
    Protein,
    Pathway,
    Disease,
    Phenotype,
    CellLine,
    Drug,
    Compound,
    // Methods
    Method,
    Assay,
    Model,
    // Concepts
    Hypothesis,
    Theory,
    Mechanism,
    Biomarker,
    // Literature
    Paper,
    Author,
    Institution,
    // Data
    Dataset,
    Metric,
    // Generic
    Concept,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub id: RelationId,
    pub from_id: EntityId,
    pub to_id: EntityId,
    pub relation_type: RelationType,
    pub confidence: f64,
    pub evidence: String,
    pub source_paper_id: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationType {
    // Causal/mechanistic
    Activates,
    Inhibits,
    Regulates,
    BindsTo,
    Phosphorylates,
    InteractsWith,
    Catalyzes,
    Transports,
    // Association
    AssociatedWith,
    CorrelatedWith,
    // Method/evidence
    UsesMethod,
    MeasuredBy,
    EvidencedBy,
    // Semantic
    IsA,
    PartOf,
    LocatedIn,
    // Literature
    Cites,
    Contradicts,
    Supports,
    Extends,
    // Hypothesis specific
    Hypothesizes,
    Predicts,
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl RelationType {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_lowercase().as_str() {
            "activates" => Self::Activates,
            "inhibits" => Self::Inhibits,
            "regulates" => Self::Regulates,
            "binds_to" | "binds" => Self::BindsTo,
            "phosphorylates" => Self::Phosphorylates,
            "interacts_with" | "interacts" => Self::InteractsWith,
            "catalyzes" => Self::Catalyzes,
            "transports" => Self::Transports,
            "associated_with" | "associated" => Self::AssociatedWith,
            "correlated_with" | "correlated" => Self::CorrelatedWith,
            "uses_method" | "uses" => Self::UsesMethod,
            "measured_by" | "measured" => Self::MeasuredBy,
            "evidenced_by" | "evidenced" => Self::EvidencedBy,
            "is_a" => Self::IsA,
            "part_of" | "part" => Self::PartOf,
            "located_in" | "located" => Self::LocatedIn,
            "cites" => Self::Cites,
            "contradicts" => Self::Contradicts,
            "supports" => Self::Supports,
            "extends" => Self::Extends,
            "hypothesizes" => Self::Hypothesizes,
            "predicts" => Self::Predicts,
            _ => return None,
        })
    }
}
