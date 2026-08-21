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

pub mod pipeline;
pub use pipeline::{run_research, ResearchOptions, ResearchProgress};

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

    /// Render the user-facing final report at `<dir>/<brief>.md`.
    ///
    /// Unlike `run_report.md` (the *audit* timeline for engineers), this is
    /// the report a researcher / clinician opens first: research question,
    /// literature coverage, KG overview, refined hypotheses with mechanism
    /// and evidence, debate verdict, validation plans summary, and the
    /// status of each data-analysis task. It reuses whatever is on disk
    /// (KG, hypotheses, debate, plans, analyses) so it stays correct after
    /// resume / partial runs.
    pub fn write_user_report(&self, brief: &str) -> Result<PathBuf> {
        let path = self.dir.join(format!("{brief}.md"));
        let mut md = String::new();

        // ── Header ─────────────────────────────────────────────
        md.push_str(&format!(
            "# {} 致病机理研究 · 最终报告\n\n",
            self.query.trim()
        ));
        md.push_str(&format!(
            "- **运行 ID**: `{}`  \n- **生成时间**: {}  \n- **项目目录**: `{}`\n\n",
            self.id,
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            self.dir.display(),
        ));

        // ── Executive summary ──────────────────────────────────
        let kg_stats = self.kg_stats.clone().unwrap_or_else(|| serde_json::json!({}));
        let n_entities = kg_stats.get("entities").and_then(|v| v.as_u64()).unwrap_or(0);
        let n_relations = kg_stats.get("relations").and_then(|v| v.as_u64()).unwrap_or(0);
        let n_refined = self.hypotheses.iter().filter(|h| h.refined).count();
        let n_plans = self.validation_plans.len();
        let n_analyses = self.analyses.len();
        md.push_str("## 执行摘要\n\n");
        md.push_str(&format!(
            "本报告综合了文献检索、知识图谱构建、致病机理假说生成与精炼、跨文献证据辩论、以及可执行验证计划与端到端数据分析的完整结果。"
        ));
        md.push_str(&format!(
            "共纳入 **{n_entities}** 个知识图谱实体、**{n_relations}** 条关系，"
        ));
        md.push_str(&format!(
            "最终产出 **{n_refined}** 条精炼假说、**{n_plans}** 套验证计划与 **{n_analyses}** 项数据分析交付。\n\n",
        ));

        // ── 1. Research question ────────────────────────────────
        md.push_str(&format!("## 1. 研究问题\n\n{}\n\n", self.query.trim()));

        // ── 2. Literature coverage (if papers.json exists) ─────
        let papers_path = self.dir.join("papers.json");
        if let Ok(text) = std::fs::read_to_string(&papers_path) {
            if let Ok(papers) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
                md.push_str(&format!("## 2. 文献概览（{} 篇）\n\n", papers.len()));
                md.push_str("| PMID | 标题 |\n|---|---|\n");
                for p in papers.iter().take(50) {
                    let pmid = p.get("pmid").and_then(|v| v.as_str()).unwrap_or("?");
                    let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let title_short = if title.chars().count() > 120 {
                        let cut: String = title.chars().take(120).collect();
                        format!("{cut}…")
                    } else {
                        title.to_string()
                    };
                    md.push_str(&format!("| [{pmid}](https://pubmed.ncbi.nlm.nih.gov/{pmid}/) | {title_short} |\n"));
                }
                if papers.len() > 50 {
                    md.push_str(&format!("\n> 其余 {} 篇见 `papers.json`。\n", papers.len() - 50));
                }
                md.push('\n');
            }
        }

        // ── 3. Knowledge graph overview ────────────────────────
        md.push_str("## 3. 知识图谱概要\n\n");
        if let Ok(text) = std::fs::read_to_string(self.dir.join("kg.json")) {
            if let Ok(kg) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(entities) = kg.get("entities").and_then(|v| v.as_array()) {
                    md.push_str(&format!("**{}** 个实体，按类型分布：\n\n", entities.len()));
                    let mut by_type: std::collections::BTreeMap<String, Vec<String>> =
                        std::collections::BTreeMap::new();
                    for e in entities {
                        let t = e.get("entity_type").and_then(|v| v.as_str()).unwrap_or("Other").to_string();
                        let n = e.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                        by_type.entry(t).or_default().push(n);
                    }
                    for (t, names) in &by_type {
                        let shown = if names.len() > 8 {
                            format!("{}（+{} 更多）", names.iter().take(8).cloned().collect::<Vec<_>>().join("、"), names.len() - 8)
                        } else {
                            names.join("、")
                        };
                        md.push_str(&format!("- **{}**（{}）：{}\n", t, names.len(), shown));
                    }
                    md.push('\n');
                }
                if let Some(relations) = kg.get("relations").and_then(|v| v.as_array()) {
                    md.push_str(&format!("**{}** 条关系（详见 `kg.json`）。\n\n", relations.len()));
                }
            }
        }

        // ── 4. Refined hypotheses ──────────────────────────────
        md.push_str("## 4. 致病机理假说（精炼后）\n\n");
        // Try refined set first; fall back to ranked.
        let refined_path = self.dir.join("hypotheses_refined_full.json");
        let refined_src = std::fs::read_to_string(&refined_path)
            .or_else(|_| std::fs::read_to_string(self.dir.join("hypotheses_full.json")))
            .ok();
        let full_hyps: Vec<serde_json::Value> = refined_src
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        // Map refined list (uuid) → full hyp body for statement/mechanism
        let full_by_id: std::collections::HashMap<String, &serde_json::Value> = full_hyps
            .iter()
            .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(|id| (id.to_string(), v)))
            .collect();
        let hyp_ids: Vec<String> = self.hypotheses.iter().map(|h| h.id.to_string()).collect();
        if !hyp_ids.is_empty() {
            md.push_str("| # | 假说 ID | 核心陈述 |\n|---|---|---|\n");
            for (idx, id) in hyp_ids.iter().enumerate() {
                let body = full_by_id.get(id);
                let stmt = body
                    .and_then(|v| v.get("statement").and_then(|s| s.as_str()))
                    .unwrap_or("(未找到完整陈述)");
                let stmt_short = if stmt.chars().count() > 80 {
                    let cut: String = stmt.chars().take(80).collect();
                    format!("{cut}…")
                } else {
                    stmt.to_string()
                };
                let badge = if self.hypotheses[idx].refined { "✨ 精炼" } else { "候选" };
                md.push_str(&format!("| {} | {} · {} | {} |\n", idx + 1, &id[..8.min(id.len())], badge, stmt_short));
            }
            md.push('\n');

            // Detail blocks for the top 3 refined.
            md.push_str("### 4.1 核心假说详述（按优先级 Top 3）\n\n");
            for (idx, id) in hyp_ids.iter().take(3).enumerate() {
                if let Some(body) = full_by_id.get(id) {
                    let stmt = body.get("statement").and_then(|s| s.as_str()).unwrap_or("");
                    let mechanism = body.get("mechanism").and_then(|s| s.as_str()).unwrap_or("");
                    let novelty = body.get("novelty").and_then(|s| s.as_str()).unwrap_or("");
                    let confidence = body.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let evidence = body.get("supporting_evidence").and_then(|v| v.as_array());
                    md.push_str(&format!(
                        "#### 假说 #{}：{}\n\n**新颖度**：{}  ·  **置信度**：{:.2}\n\n**陈述**：{}\n\n",
                        idx + 1,
                        body.get("name").and_then(|s| s.as_str()).unwrap_or(""),
                        novelty,
                        confidence,
                        stmt,
                    ));
                    if !mechanism.is_empty() {
                        md.push_str(&format!("**机制说明**：\n\n{}\n\n", mechanism));
                    }
                    if let Some(ev) = evidence {
                        if !ev.is_empty() {
                            md.push_str("**支持证据**：\n\n");
                            for e in ev.iter().take(5) {
                                if let Some(s) = e.as_str() {
                                    md.push_str(&format!("- {}\n", s));
                                }
                            }
                            md.push('\n');
                        }
                    }
                }
            }
        } else {
            md.push_str("（未生成假说；详见 `project.json` 的失败事件。）\n\n");
        }

        // ── 5. Debate verdict ──────────────────────────────────
        let debate_path = self.dir.join("debate_report.json");
        if let Ok(text) = std::fs::read_to_string(&debate_path) {
            if let Ok(report) = serde_json::from_str::<serde_json::Value>(&text) {
                md.push_str("## 5. 假说辩论与裁决\n\n");
                if let Some(rounds) = report.get("rounds").and_then(|v| v.as_array()) {
                    md.push_str(&format!("共进行 **{}** 轮交叉质询。\n\n", rounds.len()));
                }
                if let Some(per) = report.get("per_hypothesis").and_then(|v| v.as_array()) {
                    md.push_str("| 假说 | 裁决 | 置信度 |\n|---|---|---|\n");
                    for ph in per {
                        let id = ph.get("hypothesis_id").and_then(|s| s.as_str()).unwrap_or("?");
                        let verdict = ph.get("verdict").and_then(|s| s.as_str()).unwrap_or("?");
                        let conf = ph.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let id_short = if id.len() >= 8 { &id[..8] } else { id };
                        md.push_str(&format!("| {} | `{}` | {:.2} |\n", id_short, verdict, conf));
                    }
                    md.push('\n');
                }
                if let Some(strongest) = report.get("strongest_hypothesis").and_then(|s| s.as_str()) {
                    md.push_str(&format!("**最强假说**：`{}`\n\n", &strongest[..8.min(strongest.len())]));
                }
                if let Some(summary) = report.get("summary").and_then(|s| s.as_str()) {
                    if !summary.is_empty() {
                        md.push_str(&format!("**裁判总结**：{}\n\n", summary));
                    }
                }
            }
        }

        // ── 6. Validation plans ────────────────────────────────
        if !self.validation_plans.is_empty() {
            md.push_str("## 6. 验证计划\n\n");
            for (i, p) in self.validation_plans.iter().enumerate() {
                let plan: serde_json::Value = std::fs::read_to_string(p)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
                let plan_file = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let rationale = plan.get("rationale").and_then(|s| s.as_str()).unwrap_or("");
                let tasks = plan.get("data_analysis_tasks").and_then(|v| v.as_array());
                let protocols = plan.get("wet_lab_protocols").and_then(|v| v.as_array());
                md.push_str(&format!(
                    "### 计划 {}：`{}`\n\n",
                    i + 1,
                    plan_file,
                ));
                if !rationale.is_empty() {
                    md.push_str(&format!("**设计理由**：{}\n\n", rationale));
                }
                if let Some(t) = tasks {
                    md.push_str(&format!("**数据分析任务**（{} 项）：\n\n", t.len()));
                    md.push_str("| ID | 数据集 | 目标 |\n|---|---|---|\n");
                    for task in t {
                        let id = task.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                        let acc = task.get("dataset_accession").and_then(|s| s.as_str()).unwrap_or("(本地)");
                        let obj = task.get("objective").and_then(|s| s.as_str()).unwrap_or("");
                        let obj_short = if obj.chars().count() > 90 {
                            let cut: String = obj.chars().take(90).collect();
                            format!("{cut}…")
                        } else {
                            obj.to_string()
                        };
                        md.push_str(&format!("| {} | `{}` | {} |\n", id, acc, obj_short));
                    }
                    md.push('\n');
                }
                if let Some(p) = protocols {
                    md.push_str(&format!("**湿实验方案**（{} 项）：\n\n", p.len()));
                    for proto in p {
                        let id = proto.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                        let obj = proto.get("objective").and_then(|s| s.as_str()).unwrap_or("");
                        let obj_short = if obj.chars().count() > 100 {
                            let cut: String = obj.chars().take(100).collect();
                            format!("{cut}…")
                        } else {
                            obj.to_string()
                        };
                        md.push_str(&format!("- **{}**：{}\n", id, obj_short));
                    }
                    md.push('\n');
                }
            }
        }

        // ── 7. Data analysis delivery ──────────────────────────
        if !self.analyses.is_empty() {
            md.push_str("## 7. 数据分析交付\n\n");
            md.push_str("| 任务 | 数据集 | 后端 | 状态 | Notebook | 溯源 |\n|---|---|---|---|---|---|\n");
            for a in &self.analyses {
                let nb = a.notebook_path.as_ref().map(|p| p.file_name().and_then(|n| n.to_str()).unwrap_or("?")).unwrap_or("—");
                let pv = a.provenance_path.as_ref().map(|p| p.file_name().and_then(|n| n.to_str()).unwrap_or("?")).unwrap_or("—");
                let status = if a.success { "✅ 成功" } else { "❌ 失败" };
                md.push_str(&format!(
                    "| {} | `{}` | {} | {} | `{}` | `{}` |\n",
                    a.task_id,
                    a.task_id, // dataset often encoded in task id by runner; keep generic
                    a.execution_backend,
                    status,
                    nb,
                    pv,
                ));
            }
            md.push('\n');
            md.push_str("每个 `analysis.ipynb` 都包含对应 plan 的全部计算步骤与注释；可在 Jupyter 中重放。\n\n");
        }

        // ── 8. Audit pointers ─────────────────────────────────
        md.push_str("## 8. 审计与复现\n\n");
        md.push_str("- `project.json` — 全流水线阶段状态与 append-only 事件日志（机器可读）\n");
        md.push_str("- `run_report.md` — 阶段时长表与事件时间线（运维可读）\n");
        md.push_str("- `kg.json` — 完整知识图谱（节点 + 关系）\n");
        md.push_str("- `debate_report.json` — 辩论每轮的论据 / 反驳 / 裁决\n");
        md.push_str("- `plans/validation_plan_*.json` — 验证计划全文（含变量定义与统计方法）\n\n");
        md.push_str("---\n\n");
        md.push_str("*本报告由 miniagent research 流水线自动生成，所有数据可在项目目录内复现与重跑。*\n");

        std::fs::write(&path, md)
            .with_context(|| format!("write user report {}", path.display()))?;
        Ok(path)
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

    /// Render the audit timeline as a human-readable `run_report.md` next to
    /// `project.json`: model/config attribution, per-stage table, artifact
    /// inventory, and the full append-only event log (dsh: everything the
    /// run did must be reconstructable from disk).
    pub fn write_run_report(&self) -> Result<PathBuf> {
        let mut md = String::new();
        md.push_str("# Run Report\n\n");
        md.push_str(&format!(
            "- **Query**: {}\n- **Run ID**: {}\n- **Created**: {}\n- **Last update**: {}\n- **Project dir**: `{}`\n\n",
            self.query,
            self.id,
            self.created_at.to_rfc3339(),
            self.updated_at.to_rfc3339(),
            self.dir.display(),
        ));

        md.push_str("## Stages\n\n| Stage | Status | Duration | Outputs |\n|---|---|---|---|\n");
        for s in &self.stages {
            let outputs = if s.output_paths.is_empty() {
                "—".to_string()
            } else {
                s.output_paths
                    .iter()
                    .map(|p| format!("`{}`", p.display()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let status = match s.status {
                StageStatus::Completed => "✅ completed",
                StageStatus::Failed => "❌ failed",
                StageStatus::Running => "🔄 running",
                StageStatus::Skipped => "⏭ skipped",
            };
            md.push_str(&format!(
                "| {} | {} | {:.1}s | {} |\n",
                s.name, status, s.duration_secs, outputs
            ));
        }

        if let Some(stats) = &self.kg_stats {
            md.push_str(&format!("\n**KG stats**: `{stats}`\n"));
        }

        if !self.hypotheses.is_empty() {
            md.push_str("\n## Hypotheses\n\n");
            for h in &self.hypotheses {
                let tag = if h.refined { "refined" } else { "ranked" };
                md.push_str(&format!(
                    "{}. **[{tag}]** {} (`{}`)\n",
                    h.rank,
                    h.statement.replace('\n', " "),
                    h.id
                ));
            }
        }
        if let Some(report) = &self.debate_report {
            md.push_str(&format!("\n**Debate report**: `{}`\n", report.display()));
        }
        if !self.validation_plans.is_empty() {
            md.push_str("\n## Validation Plans\n\n");
            for p in &self.validation_plans {
                md.push_str(&format!("- `{}`\n", p.display()));
            }
        }
        if !self.analyses.is_empty() {
            md.push_str("\n## Data Analyses\n\n| Task | Backend | Success | Notebook | Provenance |\n|---|---|---|---|---|\n");
            for a in &self.analyses {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    a.task_id,
                    if a.execution_backend.is_empty() { "—" } else { &a.execution_backend },
                    if a.success { "✅" } else { "❌" },
                    a.notebook_path.as_ref().map(|p| format!("`{}`", p.display())).unwrap_or_else(|| "—".into()),
                    a.provenance_path.as_ref().map(|p| format!("`{}`", p.display())).unwrap_or_else(|| "—".into()),
                ));
            }
        }

        md.push_str("\n## Event Log (append-only)\n\n| Time | Kind | Message |\n|---|---|---|\n");
        for e in &self.event_log {
            md.push_str(&format!(
                "| {} | {} | {} |\n",
                e.timestamp.to_rfc3339(),
                e.kind,
                e.message.replace('|', "\\|").replace('\n', " ")
            ));
        }

        let path = self.dir.join("run_report.md");
        std::fs::write(&path, md)
            .with_context(|| format!("write run report {}", path.display()))?;
        Ok(path)
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
