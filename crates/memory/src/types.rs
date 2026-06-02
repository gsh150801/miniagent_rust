use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryLayer {
    Working,
    Episodic,
    Semantic,
    Procedural,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationType {
    Contradicts,
    Extends,
    Supports,
    CitesAsEvidence,
    UsesMethod,
    IsA,
    PartOf,
    AssociatedWith,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicRecord {
    pub id: uuid::Uuid,
    pub title: String,
    pub content: StructuredSummary,
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub importance: f64,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub access_count: u64,
    pub decay_rate: f64,
    pub retention_floor: f64,
    pub current_strength: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct StructuredSummary {
    pub background: String,
    pub method: String,
    pub key_findings: Vec<String>,
    pub limitations: Vec<String>,
    pub contributions: Vec<String>,
    pub raw_summary: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub from_id: uuid::Uuid,
    pub to_id: uuid::Uuid,
    pub relation_type: RelationType,
    pub strength: f64,
    pub evidence: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub query: String,
    pub max_results: usize,
    pub importance_threshold: f64,
    pub strength_threshold: f64,
    pub tags: Vec<String>,
    pub use_fts: bool,
    pub use_vector: bool,
    pub use_graph: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            query: String::new(),
            max_results: 10,
            importance_threshold: 0.1,
            strength_threshold: 0.1,
            tags: vec![],
            use_fts: true,
            use_vector: false,
            use_graph: true,
        }
    }
}

impl SearchConfig {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub record_id: uuid::Uuid,
    pub title: String,
    pub snippet: String,
    pub score: f64,
    pub layer: MemoryLayer,
}

#[derive(Debug, Clone)]
pub struct AssembledContext {
    pub system_prompt: String,
    pub memory_context: String,
    pub recent_records: Vec<EpisodicRecord>,
    pub search_results: Vec<SearchResult>,
    pub total_tokens: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ConsolidationLevel {
    Inline,
    EpisodeEnd,
    Sleeptime,
}
