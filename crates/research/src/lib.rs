//! Unified, auditable manifest for a miniagent research project.
//!
//! A *research project* is one end-to-end run of the literature → knowledge
//! graph → hypotheses → debate → validation plans → data-analysis pipeline.
//! [`ProjectManifest`] is the single, human-readable `project.json` that ties
//! every stage's status, outputs, and key artifacts together, so the whole run
//! is auditable as one unit and resumable when interrupted.
//!
//! This crate is intentionally **decoupled** from the rest of the workspace: it
//! depends only on serde/chrono/uuid and stores stage/hypothesis/analysis
//! summaries as plain JSON, so it can be used by the CLI (and any other
//! orchestrator) without pulling in the agent/kg/hypothesis crates.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The on-disk filename for a manifest within its project directory.
pub const MANIFEST_FILENAME: &str = "project.json";

/// One recorded stage of the research pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRecord {
    pub name: String,
    pub status: StageStatus,
    pub duration_secs: f64,
    pub output_paths: Vec<PathBuf>,
    /// Free-form JSON summary (counts, key metrics, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StageStatus {
    Running,
    Completed,
    Failed,
    Skipped,
}

/// A reference to a (possibly refined) hypothesis discovered during the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypothesisRef {
    pub id: Uuid,
    /// 1-based rank within the set recorded by this call.
    pub rank: usize,
    /// Whether this entry comes from the post-debate *refined* set.
    pub refined: bool,
    pub statement: String,
    /// Path to the ranked/refined hypothesis JSON, if persisted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_path: Option<PathBuf>,
}

/// A reference to an executed data-analysis task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisRef {
    pub task_id: String,
    pub hypothesis_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notebook_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance_path: Option<PathBuf>,
    pub success: bool,
    /// "jupyter" | "python" | "dry_run".
    #[serde(default)]
    pub execution_backend: String,
}

/// A timestamped audit event on the manifest timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEvent {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub message: String,
}

/// The unified, auditable manifest for a research project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub id: Uuid,
    pub query: String,
    /// The directory that contains `project.json`. Not serialized (implied by
    /// the file's location; restored on [`ProjectManifest::load`]).
    #[serde(skip)]
    pub dir: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stages: Vec<StageRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kg_stats: Option<serde_json::Value>,
    pub hypotheses: Vec<HypothesisRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debate_report: Option<PathBuf>,
    pub validation_plans: Vec<PathBuf>,
    pub analyses: Vec<AnalysisRef>,
    pub event_log: Vec<ManifestEvent>,
}

impl ProjectManifest {
    /// Create a fresh manifest for a query, rooted at `dir`.
    pub fn new(query: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            query: query.into(),
            dir: dir.into(),
            created_at: now,
            updated_at: now,
            stages: Vec::new(),
            kg_stats: None,
            hypotheses: Vec::new(),
            debate_report: None,
            validation_plans: Vec::new(),
            analyses: Vec::new(),
            event_log: Vec::new(),
        }
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Record (or re-record) a pipeline stage. Re-recording an existing name
    /// replaces the prior entry, so a stage can transition Running → Completed.
    pub fn record_stage(
        &mut self,
        name: impl Into<String>,
        status: StageStatus,
        duration: std::time::Duration,
        output_paths: Vec<PathBuf>,
        summary: Option<serde_json::Value>,
    ) {
        let name = name.into();
        let record = StageRecord {
            name: name.clone(),
            status,
            duration_secs: duration.as_secs_f64(),
            output_paths,
            summary,
        };
        if let Some(existing) = self.stages.iter_mut().find(|s| s.name == name) {
            *existing = record;
        } else {
            self.stages.push(record);
        }
        self.log_event(
            match status {
                StageStatus::Completed => "stage_completed",
                StageStatus::Failed => "stage_failed",
                StageStatus::Running => "stage_started",
                StageStatus::Skipped => "stage_skipped",
            },
            name,
        );
        self.touch();
    }

    /// Replace the hypothesis set. Each ref is assigned a 1-based rank by order.
    pub fn record_hypotheses(&mut self, mut refs: Vec<HypothesisRef>) {
        for (i, r) in refs.iter_mut().enumerate() {
            r.rank = i + 1;
        }
        self.hypotheses = refs;
        self.touch();
    }

    pub fn set_kg_stats(&mut self, stats: serde_json::Value) {
        self.kg_stats = Some(stats);
        self.touch();
    }

    pub fn record_debate(&mut self, report: impl Into<PathBuf>) {
        self.debate_report = Some(report.into());
        self.log_event("debate_completed", "hypothesis debate & refinement");
        self.touch();
    }

    pub fn add_validation_plan(&mut self, path: impl Into<PathBuf>) {
        self.validation_plans.push(path.into());
        self.touch();
    }

    pub fn record_analysis(&mut self, analysis: AnalysisRef) {
        let task = analysis.task_id.clone();
        let ok = analysis.success;
        self.analyses.push(analysis);
        self.log_event(
            if ok { "analysis_succeeded" } else { "analysis_failed" },
            task,
        );
        self.touch();
    }

    /// Append a timestamped event to the audit timeline.
    pub fn log_event(&mut self, kind: impl Into<String>, message: impl Into<String>) {
        self.event_log.push(ManifestEvent {
            timestamp: Utc::now(),
            kind: kind.into(),
            message: message.into(),
        });
        self.touch();
    }

    /// Names of stages that have reached a terminal *success* state.
    pub fn completed_stage_names(&self) -> HashSet<String> {
        self.stages
            .iter()
            .filter(|s| s.status == StageStatus::Completed)
            .map(|s| s.name.clone())
            .collect()
    }

    /// True when a stage with `name` has already completed (resume guard).
    pub fn is_stage_done(&self, name: &str) -> bool {
        self.stages
            .iter()
            .any(|s| s.name == name && s.status == StageStatus::Completed)
    }

    /// True once the final analysis stage has completed (the pipeline is done).
    pub fn is_complete(&self) -> bool {
        self.is_stage_done("analysis")
    }

    /// Path to `project.json` within this manifest's directory.
    pub fn path(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILENAME)
    }

    /// Persist the manifest atomically to `<dir>/project.json`.
    pub fn save(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("create project dir {}", self.dir.display()))?;
        let json = serde_json::to_string_pretty(self).context("serialize manifest")?;
        let final_path = self.path();
        // Atomic write: tmp file in the same dir, then rename.
        let tmp_path = self.dir.join(format!("{}.tmp", MANIFEST_FILENAME));
        std::fs::write(&tmp_path, json)
            .with_context(|| format!("write manifest tmp {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &final_path)
            .with_context(|| format!("rename manifest to {}", final_path.display()))?;
        Ok(final_path)
    }

    /// Load a manifest from a directory containing `project.json`.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let path = dir.join(MANIFEST_FILENAME);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        let mut manifest: ProjectManifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse manifest {}", path.display()))?;
        manifest.dir = dir;
        Ok(manifest)
    }
}

impl HypothesisRef {
    pub fn new(id: Uuid, statement: impl Into<String>, json_path: Option<PathBuf>) -> Self {
        Self {
            id,
            rank: 0,
            refined: false,
            statement: statement.into(),
            json_path,
        }
    }

    pub fn with_refined(mut self, refined: bool) -> Self {
        self.refined = refined;
        self
    }
}

impl AnalysisRef {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            hypothesis_id: None,
            notebook_path: None,
            provenance_path: None,
            success: false,
            execution_backend: String::new(),
        }
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_stage_replaces_by_name() {
        let mut m = ProjectManifest::new("q", PathBuf::from("/tmp/x"));
        m.record_stage("search", StageStatus::Running, std::time::Duration::from_secs(1), vec![], None);
        assert_eq!(m.stages.len(), 1);
        assert_eq!(m.stages[0].status, StageStatus::Running);
        m.record_stage("search", StageStatus::Completed, std::time::Duration::from_secs(5), vec!["a.json".into()], Some(json!({"papers": 10})));
        assert_eq!(m.stages.len(), 1, "same-name stage replaced");
        assert_eq!(m.stages[0].status, StageStatus::Completed);
        assert!((m.stages[0].duration_secs - 5.0).abs() < 1e-9);
        assert_eq!(m.stages[0].summary.as_ref().unwrap()["papers"], 10);
    }

    #[test]
    fn completed_stage_names_and_is_stage_done() {
        let mut m = ProjectManifest::new("q", PathBuf::from("/tmp/x"));
        m.record_stage("search", StageStatus::Completed, std::time::Duration::default(), vec![], None);
        m.record_stage("kg", StageStatus::Failed, std::time::Duration::default(), vec![], None);
        assert!(m.is_stage_done("search"));
        assert!(!m.is_stage_done("kg"));
        assert_eq!(m.completed_stage_names().len(), 1);
    }

    #[test]
    fn hypotheses_assigned_ranks() {
        let mut m = ProjectManifest::new("q", PathBuf::from("/tmp/x"));
        let refs = vec![
            HypothesisRef::new(Uuid::new_v4(), "a", None),
            HypothesisRef::new(Uuid::new_v4(), "b", None).with_refined(true),
        ];
        m.record_hypotheses(refs);
        assert_eq!(m.hypotheses.len(), 2);
        assert_eq!(m.hypotheses[0].rank, 1);
        assert_eq!(m.hypotheses[1].rank, 2);
        assert!(m.hypotheses[1].refined);
    }

    #[test]
    fn save_and_load_roundtrips() {
        let dir = std::env::temp_dir().join("miniagent_research_manifest_test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = ProjectManifest::new("APOE and Alzheimer", dir.clone());
        m.record_stage("search", StageStatus::Completed, std::time::Duration::from_secs(2), vec!["papers.json".into()], None);
        m.set_kg_stats(json!({"entities": 120, "relations": 300}));
        m.record_hypotheses(vec![HypothesisRef::new(Uuid::new_v4(), "h1", None)]);
        m.record_debate("debate_report.json");
        m.add_validation_plan("plans/vp1.json");
        let saved = m.save().unwrap();
        assert!(saved.ends_with(MANIFEST_FILENAME));

        let loaded = ProjectManifest::load(&dir).unwrap();
        assert_eq!(loaded.query, "APOE and Alzheimer");
        assert_eq!(loaded.dir, dir);
        assert!(loaded.is_stage_done("search"));
        assert_eq!(
            loaded.kg_stats.as_ref().and_then(|v| v["entities"].as_u64()),
            Some(120)
        );
        assert_eq!(loaded.hypotheses.len(), 1);
        assert_eq!(loaded.debate_report.as_deref(), Some(Path::new("debate_report.json")));
        assert_eq!(loaded.validation_plans.len(), 1);
        assert!(!loaded.is_complete(), "analysis stage not done yet");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_is_atomic_no_tmp_left_behind() {
        let dir = std::env::temp_dir().join("miniagent_research_atomic_test");
        let _ = std::fs::remove_dir_all(&dir);
        let m = ProjectManifest::new("q", dir.clone());
        m.save().unwrap();
        assert!(!dir.join(format!("{MANIFEST_FILENAME}.tmp")).exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
