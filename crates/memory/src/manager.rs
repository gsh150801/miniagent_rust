use chrono::Utc;
use uuid::Uuid;

use crate::decay::MemoryDecay;
use crate::episodic::EpisodicMemory;
use crate::procedural::ProceduralMemory;
use crate::semantic::SemanticMemory;
use crate::types::{
    AssembledContext, ConsolidationLevel, EpisodicRecord, Relation,
    RelationType, SearchConfig, SearchResult, StructuredSummary,
};
use crate::working::WorkingMemory;

pub struct MemoryManager {
    working: WorkingMemory,
    episodic: EpisodicMemory,
    semantic: SemanticMemory,
    procedural: ProceduralMemory,
}

impl MemoryManager {
    pub fn new(episodic_db_path: &str) -> Result<Self, rusqlite::Error> {
        let episodic = EpisodicMemory::new(episodic_db_path)?;
        Ok(Self {
            working: WorkingMemory::new(128_000),
            episodic,
            semantic: SemanticMemory::new(),
            procedural: ProceduralMemory::new(),
        })
    }

    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let episodic = EpisodicMemory::new_in_memory()?;
        Ok(Self {
            working: WorkingMemory::new(128_000),
            episodic,
            semantic: SemanticMemory::new(),
            procedural: ProceduralMemory::new(),
        })
    }

    pub fn working(&self) -> &WorkingMemory { &self.working }
    pub fn episodic(&self) -> &EpisodicMemory { &self.episodic }
    pub fn procedural(&self) -> &ProceduralMemory { &self.procedural }

    // ── Store ─────────────────────────────────────────────────

    pub fn store(&self, record: &EpisodicRecord) -> Result<(), rusqlite::Error> {
        self.episodic.store(record)?;
        Ok(())
    }

    pub fn store_paper_summary(
        &self,
        title: &str,
        summary: &StructuredSummary,
        tags: &[String],
        source: Option<&str>,
    ) -> Result<Uuid, rusqlite::Error> {
        let now = Utc::now();
        let record = EpisodicRecord {
            id: Uuid::new_v4(),
            title: title.to_string(),
            content: summary.clone(),
            tags: tags.to_vec(),
            source: source.map(|s| s.to_string()),
            importance: 0.7,
            created_at: now,
            last_accessed: now,
            access_count: 0,
            decay_rate: MemoryDecay::default_rate("factual"),
            retention_floor: MemoryDecay::default_floor("factual"),
            current_strength: 1.0,
        };
        self.episodic.store(&record)?;
        Ok(record.id)
    }

    // ── Search ────────────────────────────────────────────────

    pub fn search(&self, config: &SearchConfig) -> Result<Vec<SearchResult>, rusqlite::Error> {
        let mut results = self.episodic.search(config)?;
        let vec_results = self.semantic.search(config);
        results.extend(vec_results);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(config.max_results);
        Ok(results)
    }

    // ── Relations ─────────────────────────────────────────────

    pub fn link_relations(
        &self,
        from: &Uuid,
        to: &Uuid,
        rel_type: RelationType,
        evidence: &str,
    ) -> Result<(), rusqlite::Error> {
        let relation = Relation {
            from_id: *from,
            to_id: *to,
            relation_type: rel_type,
            strength: 1.0,
            evidence: evidence.to_string(),
            created_at: Utc::now(),
        };
        self.episodic.link_relation(&relation)
    }

    pub fn query_relations(
        &self,
        id: &Uuid,
        rel_type: Option<RelationType>,
        depth: usize,
    ) -> Result<Vec<Vec<Relation>>, rusqlite::Error> {
        self.episodic.query_relations(id, rel_type, depth)
    }

    // ── Context Assembly ──────────────────────────────────────

    pub fn assemble_context(
        &self,
        task_description: &str,
        max_records: usize,
    ) -> AssembledContext {
        let mut context = String::new();
        let mut total_tokens = 0;

        // Search relevant memories
        let config = SearchConfig {
            query: task_description.to_string(),
            max_results: max_records,
            ..Default::default()
        };

        let search_results = self.episodic.search(&config).unwrap_or_default();

        // Add procedural skills
        let skills = self.procedural.find_relevant(task_description);
        if !skills.is_empty() {
            context.push_str("## Relevant Skills\n\n");
            for skill in skills {
                let skill_text = format!("**{}**: {}\n", skill.name, skill.content);
                context.push_str(&skill_text);
                total_tokens += skill_text.len() / 4;
            }
            context.push('\n');
        }

        // Recent episodic records
        if !search_results.is_empty() {
            context.push_str("## Relevant Context\n\n");
            for result in &search_results {
                let entry = format!("- **{}**: {}\n", result.title, result.snippet);
                context.push_str(&entry);
                total_tokens += entry.len() / 4;
            }
            context.push('\n');
        }

        let recent_records: Vec<EpisodicRecord> = search_results
            .iter()
            .filter_map(|r| self.episodic.get(&r.record_id).unwrap_or(None))
            .collect();

        AssembledContext {
            system_prompt: String::new(),
            memory_context: context,
            recent_records,
            search_results,
            total_tokens,
        }
    }

    // ── Consolidation ─────────────────────────────────────────

    pub async fn consolidate(&self, level: ConsolidationLevel) {
        match level {
            ConsolidationLevel::Inline => {
                // Quick: no-op, operations done inline
            }
            ConsolidationLevel::EpisodeEnd => {
                // Decay all memories
                if let Err(e) = self.episodic.apply_decay() {
                    tracing::warn!("Decay apply error: {e}");
                }
            }
            ConsolidationLevel::Sleeptime => {
                // Deep: decay all memories and rebuild indices
                if let Err(e) = self.episodic.apply_decay() {
                    tracing::warn!("Sleeptime decay error: {e}");
                }
            }
        }
    }

    pub fn record_access(&self, id: &Uuid) -> Result<(), rusqlite::Error> {
        self.episodic.record_access(id)
    }
}
