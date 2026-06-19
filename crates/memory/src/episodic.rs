use chrono::Utc;
use rusqlite::{Connection, params};
use std::sync::Mutex;

use crate::decay::MemoryDecay;
use crate::types::{EpisodicRecord, Relation, RelationType, SearchConfig, SearchResult, StructuredSummary};
use crate::MemoryLayer;

pub struct EpisodicMemory {
    conn: Mutex<Connection>,
    __decay: MemoryDecay,
}

impl EpisodicMemory {
    pub fn new(db_path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        let this = Self {
            conn: Mutex::new(conn),
            __decay: MemoryDecay,
        };
        this.initialize_tables()?;
        Ok(this)
    }

    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        let this = Self {
            conn: Mutex::new(conn),
            __decay: MemoryDecay,
        };
        this.initialize_tables()?;
        Ok(this)
    }

    fn conn_guard(&self) -> Result<std::sync::MutexGuard<'_, Connection>, rusqlite::Error> {
        self.conn.lock().map_err(|e| rusqlite::Error::InvalidParameterName(format!("lock poisoned: {e}")))
    }

    fn initialize_tables(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS episodic_records (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                background TEXT DEFAULT '',
                method TEXT DEFAULT '',
                key_findings TEXT DEFAULT '[]',
                limitations TEXT DEFAULT '[]',
                contributions TEXT DEFAULT '[]',
                raw_summary TEXT DEFAULT '',
                tags TEXT DEFAULT '[]',
                source TEXT,
                importance REAL DEFAULT 0.5,
                created_at TEXT NOT NULL,
                last_accessed TEXT NOT NULL,
                access_count INTEGER DEFAULT 0,
                decay_rate REAL DEFAULT 0.01,
                retention_floor REAL DEFAULT 0.01,
                current_strength REAL DEFAULT 1.0
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS episodic_fts USING fts5(
                title, background, method, key_findings, raw_summary, tags,
                content='episodic_records',
                content_rowid='rowid'
            );

            CREATE TABLE IF NOT EXISTS relations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                strength REAL DEFAULT 1.0,
                evidence TEXT DEFAULT '',
                created_at TEXT NOT NULL,
                UNIQUE(from_id, to_id, relation_type)
            );

            CREATE INDEX IF NOT EXISTS idx_relations_from ON relations(from_id);
            CREATE INDEX IF NOT EXISTS idx_relations_to ON relations(to_id);

            CREATE TRIGGER IF NOT EXISTS episodic_ai AFTER INSERT ON episodic_records BEGIN
                INSERT INTO episodic_fts(rowid, title, background, method, key_findings, raw_summary, tags)
                VALUES (new.rowid, new.title, new.background, new.method, new.key_findings, new.raw_summary, new.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS episodic_ad AFTER DELETE ON episodic_records BEGIN
                INSERT INTO episodic_fts(episodic_fts, rowid, title, background, method, key_findings, raw_summary, tags)
                VALUES('delete', old.rowid, old.title, old.background, old.method, old.key_findings, old.raw_summary, old.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS episodic_au AFTER UPDATE ON episodic_records BEGIN
                INSERT INTO episodic_fts(episodic_fts, rowid, title, background, method, key_findings, raw_summary, tags)
                VALUES('delete', old.rowid, old.title, old.background, old.method, old.key_findings, old.raw_summary, old.tags);
                INSERT INTO episodic_fts(rowid, title, background, method, key_findings, raw_summary, tags)
                VALUES (new.rowid, new.title, new.background, new.method, new.key_findings, new.raw_summary, new.tags);
            END;"
        )?;
        Ok(())
    }

    pub fn store(&self, record: &EpisodicRecord) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        conn.execute(
            "INSERT OR REPLACE INTO episodic_records
             (id, title, background, method, key_findings, limitations, contributions,
              raw_summary, tags, source, importance, created_at, last_accessed,
              access_count, decay_rate, retention_floor, current_strength)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                record.id.to_string(),
                record.title,
                record.content.background,
                record.content.method,
                serde_json::to_string(&record.content.key_findings).unwrap_or_default(),
                serde_json::to_string(&record.content.limitations).unwrap_or_default(),
                serde_json::to_string(&record.content.contributions).unwrap_or_default(),
                record.content.raw_summary,
                serde_json::to_string(&record.tags).unwrap_or_default(),
                record.source,
                record.importance,
                record.created_at.to_rfc3339(),
                record.last_accessed.to_rfc3339(),
                record.access_count,
                record.decay_rate,
                record.retention_floor,
                record.current_strength,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &uuid::Uuid) -> Result<Option<EpisodicRecord>, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, background, method, key_findings, limitations, contributions,
                    raw_summary, tags, source, importance, created_at, last_accessed,
                    access_count, decay_rate, retention_floor, current_strength
             FROM episodic_records WHERE id = ?1"
        )?;

        let result = stmt.query_row(params![id.to_string()], |row| {
            Ok(EpisodicRecord {
                id: uuid::Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                title: row.get(1)?,
                content: StructuredSummary {
                    background: row.get(2)?,
                    method: row.get(3)?,
                    key_findings: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                    limitations: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                    contributions: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                    raw_summary: row.get(7)?,
                },
                tags: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
                source: row.get(9)?,
                importance: row.get(10)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(11)?)
                    .map(|dt| dt.to_utc())
                    .unwrap_or_else(|_| chrono::Utc::now()),
                last_accessed: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(12)?)
                    .map(|dt| dt.to_utc())
                    .unwrap_or_else(|_| chrono::Utc::now()),
                access_count: row.get(13)?,
                decay_rate: row.get(14)?,
                retention_floor: row.get(15)?,
                current_strength: row.get(16)?,
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn search(&self, config: &SearchConfig) -> Result<Vec<SearchResult>, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let mut results = Vec::new();

        if config.use_fts && !config.query.is_empty() {
            let query = config.query.replace('\'', "''");
            let fts_sql = "SELECT r.id, r.title, snippet(episodic_fts, 1, '<b>', '</b>', '...', 32) as snippet,
                        r.importance, r.current_strength
                 FROM episodic_fts f
                 JOIN episodic_records r ON f.rowid = r.rowid
                 WHERE episodic_fts MATCH ?1
                 AND r.current_strength >= ?2
                 AND r.importance >= ?3
                 ORDER BY rank
                 LIMIT ?4".to_string();

            let mut stmt = conn.prepare(&fts_sql)?;
            let rows = stmt.query_map(
                params![query, config.strength_threshold, config.importance_threshold, config.max_results],
                |row| {
                    Ok(SearchResult {
                        record_id: uuid::Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                        title: row.get(1)?,
                        snippet: row.get(2)?,
                        score: row.get::<_, f64>(3)? * row.get::<_, f64>(4)?,
                        layer: MemoryLayer::Episodic,
                    })
                },
            )?;

            for row in rows {
                results.push(row?);
            }
        } else if !config.query.is_empty() {
            // Fallback to LIKE search
            let query = format!("%{}%", config.query.replace('%', "\\%"));
            let mut stmt = conn.prepare(
                "SELECT id, title, raw_summary, importance, current_strength
                 FROM episodic_records
                 WHERE (title LIKE ?1 OR raw_summary LIKE ?1)
                 AND current_strength >= ?2
                 AND importance >= ?3
                 ORDER BY current_strength DESC
                 LIMIT ?4"
            )?;
            let rows = stmt.query_map(
                params![query, config.strength_threshold, config.importance_threshold, config.max_results],
                |row| {
                    Ok(SearchResult {
                        record_id: uuid::Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                        title: row.get(1)?,
                        snippet: row.get::<_, String>(2)?.chars().take(200).collect(),
                        score: row.get::<_, f64>(3)? * row.get::<_, f64>(4)?,
                        layer: MemoryLayer::Episodic,
                    })
                },
            )?;
            for row in rows {
                results.push(row?);
            }
        }

        Ok(results)
    }

    pub fn link_relation(&self, relation: &Relation) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        let rel_type = match &relation.relation_type {
            RelationType::Custom(s) => s.clone(),
            _ => format!("{:?}", relation.relation_type).to_lowercase(),
        };
        conn.execute(
            "INSERT OR REPLACE INTO relations (from_id, to_id, relation_type, strength, evidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                relation.from_id.to_string(),
                relation.to_id.to_string(),
                rel_type,
                relation.strength,
                relation.evidence,
                relation.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn query_relations(
        &self,
        id: &uuid::Uuid,
        _rel_type: Option<RelationType>,
        max_depth: usize,
    ) -> Result<Vec<Vec<Relation>>, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let mut all_paths = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut current = vec![*id];

        for _ in 0..=max_depth {
            if current.is_empty() { break; }
            let placeholders: Vec<String> = current.iter().enumerate()
                .map(|(i, _)| format!("?{}", i + 1)).collect();
            let sql = format!(
                "SELECT from_id, to_id, relation_type, strength, evidence, created_at
                 FROM relations WHERE from_id IN ({})",
                placeholders.join(",")
            );

            let ids: Vec<String> = current.iter().map(|u| u.to_string()).collect();
            let params: Vec<&dyn rusqlite::types::ToSql> = ids.iter()
                .map(|s| s as &dyn rusqlite::types::ToSql).collect();

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params.as_slice(), |row| {
                let from = uuid::Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default();
                let to = uuid::Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default();
                let rt: String = row.get(2)?;
                let rel_type = match rt.as_str() {
                    "contradicts" => RelationType::Contradicts,
                    "extends" => RelationType::Extends,
                    _ => RelationType::Custom(rt),
                };
                Ok(Relation {
                    from_id: from,
                    to_id: to,
                    relation_type: rel_type,
                    strength: row.get(3)?,
                    evidence: row.get(4)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .unwrap_or_default().into(),
                })
            })?;

            let mut step_relations = Vec::new();
            let mut next_ids = Vec::new();
            for row in rows {
                let rel = row?;
                if !visited.contains(&rel.to_id) {
                    visited.insert(rel.to_id);
                    next_ids.push(rel.to_id);
                }
                step_relations.push(rel);
            }

            if !step_relations.is_empty() {
                all_paths.push(step_relations);
            }
            current = next_ids;
        }

        Ok(all_paths)
    }

    pub fn apply_decay(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let now = Utc::now();
        let mut affected = 0;

        let mut stmt = conn.prepare(
            "SELECT id, current_strength, decay_rate, retention_floor, last_accessed
             FROM episodic_records"
        )?;

        let updates: Vec<(String, f64)> = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let strength: f64 = row.get(1)?;
            let rate: f64 = row.get(2)?;
            let floor: f64 = row.get(3)?;
            let last: String = row.get(4)?;
            let last_accessed = chrono::DateTime::parse_from_rfc3339(&last)
                .map(|dt| dt.to_utc())
                .unwrap_or(now);
            let days = (now - last_accessed).num_hours() as f64 / 24.0;
            let new_strength = MemoryDecay::calculate(strength, rate, floor, days);
            Ok((id, new_strength))
        })?
        .filter_map(|r| r.ok())
        .collect();

        for (id, new_strength) in &updates {
            conn.execute(
                "UPDATE episodic_records SET current_strength = ?1 WHERE id = ?2",
                params![new_strength, id],
            )?;
            affected += 1;
        }

        Ok(affected)
    }

    pub fn record_access(&self, id: &uuid::Uuid) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        conn.execute(
            "UPDATE episodic_records
             SET last_accessed = ?1, access_count = access_count + 1,
                 current_strength = MIN(1.0, current_strength + 0.05)
             WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id.to_string()],
        )?;
        Ok(())
    }
}
