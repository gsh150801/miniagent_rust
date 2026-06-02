use miniagent_core::checkpoint::{Checkpoint, CheckpointSummary};
use miniagent_core::message::Message;
use miniagent_core::types::{CheckpointId, ProjectId, RunId, StepId};
use rusqlite::{Connection, params};
use std::sync::Mutex;

pub struct CheckpointStore {
    conn: Mutex<Connection>,
}

impl CheckpointStore {
    pub fn new(db_path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        let store = Self { conn: Mutex::new(conn) };
        store.initialize()?;
        Ok(store)
    }

    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn: Mutex::new(conn) };
        store.initialize()?;
        Ok(store)
    }

    fn conn_guard(&self) -> Result<std::sync::MutexGuard<'_, Connection>, rusqlite::Error> {
        self.conn.lock().map_err(|e| rusqlite::Error::InvalidParameterName(format!("lock poisoned: {e}")))
    }

    fn initialize(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                project_id TEXT,
                timestamp TEXT NOT NULL,
                step_id TEXT NOT NULL,
                iteration INTEGER NOT NULL DEFAULT 0,
                history_json TEXT NOT NULL,
                progress_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_ckpt_run ON checkpoints(run_id);
            CREATE INDEX IF NOT EXISTS idx_ckpt_project ON checkpoints(project_id);
            CREATE INDEX IF NOT EXISTS idx_ckpt_timestamp ON checkpoints(timestamp);"
        )?;
        Ok(())
    }

    pub fn save(&self, checkpoint: &Checkpoint) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        let history_json = serde_json::to_string(&checkpoint.history)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let progress_json = checkpoint.progress.as_ref()
            .map(|p| serde_json::to_string(p).unwrap_or_default());

        conn.execute(
            "INSERT OR REPLACE INTO checkpoints
             (id, run_id, project_id, timestamp, step_id, iteration, history_json, progress_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                checkpoint.id.0.to_string(),
                checkpoint.run_id.0.to_string(),
                checkpoint.project_id.map(|p| p.0.to_string()),
                checkpoint.timestamp.to_rfc3339(),
                checkpoint.step_id.0.to_string(),
                checkpoint.iteration,
                history_json,
                progress_json,
            ],
        )?;
        Ok(())
    }

    pub fn load(&self, id: &CheckpointId) -> Result<Option<Checkpoint>, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, project_id, timestamp, step_id, iteration, history_json, progress_json
             FROM checkpoints WHERE id = ?1"
        )?;

        let result = stmt.query_row(params![id.0.to_string()], |row| {
            let history_json: String = row.get(6)?;
            let history: Vec<Message> = serde_json::from_str(&history_json).unwrap_or_default();
            let progress_json: Option<String> = row.get(7)?;
            let progress = progress_json.and_then(|s| serde_json::from_str(&s).ok());

            Ok(Checkpoint {
                id: CheckpointId(row.get::<_, String>(0)?.parse().unwrap_or_default()),
                run_id: RunId(row.get::<_, String>(1)?.parse().unwrap_or_default()),
                project_id: row.get::<_, Option<String>>(2)?.map(|s| ProjectId(s.parse().unwrap_or_default())),
                timestamp: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                    .map(|dt| dt.to_utc())
                    .unwrap_or_default(),
                step_id: StepId(row.get::<_, String>(4)?.parse().unwrap_or_default()),
                iteration: row.get(5)?,
                history,
                progress,
            })
        });

        match result {
            Ok(ckpt) => Ok(Some(ckpt)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn latest(&self, run_id: &RunId) -> Result<Option<Checkpoint>, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM checkpoints WHERE run_id = ?1
             ORDER BY timestamp DESC LIMIT 1"
        )?;

        let result = stmt.query_row(params![run_id.0.to_string()], |row| {
            let id_str: String = row.get(0)?;
            Ok(CheckpointId(id_str.parse().unwrap_or_default()))
        });

        match result {
            Ok(id) => self.load(&id),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn list(&self, project_id: Option<&ProjectId>) -> Result<Vec<CheckpointSummary>, rusqlite::Error> {
        let conn = self.conn_guard()?;

        let rows = if let Some(pid) = project_id {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, step_id, iteration, history_json
                 FROM checkpoints WHERE project_id = ?1
                 ORDER BY timestamp DESC LIMIT 50"
            )?;
            stmt.query_map(params![pid.0.to_string()], Self::map_summary)?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>()
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, step_id, iteration, history_json
                 FROM checkpoints ORDER BY timestamp DESC LIMIT 50"
            )?;
            stmt.query_map([], Self::map_summary)?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>()
        };

        Ok(rows)
    }

    fn map_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<CheckpointSummary> {
        let history_json: String = row.get(4)?;
        let history: Vec<Message> = serde_json::from_str(&history_json).unwrap_or_default();
        Ok(CheckpointSummary {
            id: CheckpointId(row.get::<_, String>(0)?.parse().unwrap_or_default()),
            timestamp: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                .map(|dt| dt.to_utc())
                .unwrap_or_default(),
            step_id: StepId(row.get::<_, String>(2)?.parse().unwrap_or_default()),
            iteration: row.get(3)?,
            message_count: history.len(),
        })
    }

    pub fn delete(&self, id: &CheckpointId) -> Result<(), rusqlite::Error> {
        let conn = self.conn_guard()?;
        conn.execute("DELETE FROM checkpoints WHERE id = ?1", params![id.0.to_string()])?;
        Ok(())
    }

    pub fn cleanup_old(&self, keep_count: usize) -> Result<usize, rusqlite::Error> {
        let conn = self.conn_guard()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))?;
        if count <= keep_count as i64 { return Ok(0); }
        let deleted = conn.execute(
            "DELETE FROM checkpoints WHERE id IN (
                SELECT id FROM checkpoints ORDER BY timestamp ASC LIMIT ?1
            )",
            params![count - keep_count as i64],
        )?;
        Ok(deleted)
    }
}
