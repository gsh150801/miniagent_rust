/// L2 Semantic Memory — vector storage and knowledge graph.
/// Currently uses keyword fallback; LanceDB integration planned.
use crate::types::{EpisodicRecord, SearchConfig, SearchResult};
use crate::MemoryLayer;

pub struct SemanticMemory {
    records: Vec<EpisodicRecord>,
}

impl SemanticMemory {
    pub fn new() -> Self {
        Self { records: Vec::new() }
    }

    pub fn index(&mut self, _record: &EpisodicRecord) {
        // Phase 2: store embedding and add to vector index
    }

    pub fn search(&self, config: &SearchConfig) -> Vec<SearchResult> {
        if config.use_vector && !config.query.is_empty() {
            // Phase 2: vector similarity search
            self.records.iter()
                .filter(|r| {
                    r.content.raw_summary.to_lowercase().contains(&config.query.to_lowercase())
                        || r.title.to_lowercase().contains(&config.query.to_lowercase())
                })
                .take(config.max_results)
                .map(|r| SearchResult {
                    record_id: r.id,
                    title: r.title.clone(),
                    snippet: r.content.raw_summary.chars().take(200).collect(),
                    score: r.current_strength * r.importance,
                    layer: MemoryLayer::Semantic,
                })
                .collect()
        } else {
            vec![]
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl Default for SemanticMemory {
    fn default() -> Self {
        Self::new()
    }
}
