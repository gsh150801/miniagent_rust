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
    /// the report a researcher / clinician opens first. Sections, in order:
    /// TL;DR → research question → literature coverage (PMID-indexed) →
    /// knowledge-graph overview (entity-type breakdown + top relation chains) →
    /// hypothesis cards (statement, mechanism, novelty, confidence, supporting
    /// evidence, counter-evidence, experimental design with controls &
    /// expected outcomes, feasibility) → debate verdict (per-hypothesis
    /// supporting/contradicting points, confidence shift, refinement notes,
    /// strongest hypothesis, cross-hypothesis contradictions and merge
    /// suggestions) → validation plans (data-analysis tasks + wet-lab
    /// protocols with reagents/steps/expected outcomes) → data-analysis
    /// delivery (grouped by hypothesis, with backend, notebook & provenance
    /// links) → citation index → audit pointers.
    ///
    /// The function reuses whatever is on disk (KG, hypotheses, debate,
    /// plans, analyses) so it stays correct after resume / partial runs.
    pub fn write_user_report(&self, brief: &str) -> Result<PathBuf> {
        let path = self.dir.join(format!("{brief}.md"));
        let mut md = String::new();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");

        // ── Load everything once ─────────────────────────────────────────
        let papers_json = std::fs::read_to_string(self.dir.join("papers.json")).ok();
        let kg_json = std::fs::read_to_string(self.dir.join("kg.json")).ok();
        let debate_json = std::fs::read_to_string(self.dir.join("debate_report.json")).ok();

        // Hypotheses: prefer the *refined* set, fall back to the raw set.
        let refined_path = self.dir.join("hypotheses_refined_full.json");
        let raw_path = self.dir.join("hypotheses_full.json");
        let refined_src = std::fs::read_to_string(&refined_path)
            .or_else(|_| std::fs::read_to_string(&raw_path))
            .ok();
        let full_hyps: Vec<serde_json::Value> = refined_src
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let full_by_id: std::collections::HashMap<String, &serde_json::Value> = full_hyps
            .iter()
            .filter_map(|v| v.get("id").and_then(|s| s.as_str()).map(|id| (id.to_string(), v)))
            .collect();

        // ── 0. Header ─────────────────────────────────────────────────────
        md.push_str(&format!("# {} · 最终研究报告\n\n", self.query.trim()));
        md.push_str(&format!(
            "> 运行 ID `{id}` · 生成于 {now} · 项目目录 `{dir}`\n\n",
            id = self.id,
            now = now,
            dir = self.dir.display(),
        ));

        // TL;DR needs numbers that are only known after sections 2–7 are
        // rendered; a placeholder is spliced in here and replaced with the
        // real summary just before the file is written.
        md.push_str("<<TLDR_PLACEHOLDER>>\n\n");

        // ── 1. Research question ─────────────────────────────────────────
        md.push_str("## 1. 研究问题\n\n");
        md.push_str(&format!("> {}\n\n", self.query.trim()));

        // ── 2. Literature coverage ───────────────────────────────────────
        let papers_value = papers_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(s).ok());
        if let Some(ref papers) = papers_value {
            md.push_str(&format!("## 2. 文献概览（共 {} 篇）\n\n", papers.len()));
            // Try to extract a (pmid, title, year?) triple from each paper.
            // The legacy papers.json stores `[pmid, text]` arrays, but newer
            // runs also support object form. We handle both.
            md.push_str("| # | PMID | 标题 | 年份 |\n|---|---|---|---|\n");
            for (i, p) in papers.iter().enumerate().take(50) {
                let (pmid, title, year) = extract_paper_meta(p);
                let title_short = truncate_chars(&title, 110);
                let pmid_link = if pmid != "?" {
                    format!("[{pmid}](https://pubmed.ncbi.nlm.nih.gov/{pmid}/)")
                } else {
                    "?".to_string()
                };
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    i + 1,
                    pmid_link,
                    title_short,
                    year.unwrap_or_else(|| "—".to_string()),
                ));
            }
            if papers.len() > 50 {
                md.push_str(&format!(
                    "\n> 其余 {} 篇见 `papers.json`。\n",
                    papers.len() - 50
                ));
            }
            md.push('\n');
        } else {
            md.push_str("## 2. 文献概览\n\n（未找到 `papers.json`。）\n\n");
        }

        // ── 3. Knowledge-graph overview ──────────────────────────────────
        md.push_str("## 3. 知识图谱概要\n\n");
        let kg_value = kg_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        let mut top_relations: Vec<(String, String, String, f64)> = Vec::new(); // head → rel → tail → conf
        if let Some(kg) = kg_value.as_ref() {
            if let Some(entities) = kg.get("entities").and_then(|v| v.as_array()) {
                md.push_str(&format!(
                    "**实体总数**：{}\n\n",
                    entities.len()
                ));
                let mut by_type: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                let mut id_to_name: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for e in entities {
                    let id = e.get("id").and_then(|s| s.as_str()).unwrap_or("").to_string();
                    let t = e.get("entity_type").and_then(|s| s.as_str()).unwrap_or("Other").to_string();
                    let n = e.get("name").and_then(|s| s.as_str()).unwrap_or("?").to_string();
                    if !id.is_empty() {
                        id_to_name.insert(id, n.clone());
                    }
                    by_type.entry(t).or_default().push(n);
                }
                md.push_str("**实体类型分布**：\n\n");
                for (t, names) in &by_type {
                    let shown = if names.len() > 8 {
                        format!(
                            "{}（+{} 更多）",
                            names.iter().take(8).cloned().collect::<Vec<_>>().join("、"),
                            names.len() - 8
                        )
                    } else {
                        names.join("、")
                    };
                    md.push_str(&format!("- **{}**（{}）：{}\n", t, names.len(), shown));
                }
                md.push('\n');

                if let Some(relations) = kg.get("relations").and_then(|v| v.as_array()) {
                    md.push_str(&format!("**关系总数**：{}\n\n", relations.len()));
                    // Collect relation records; we'll show the top-N highest
                    // confidence below.
                    for r in relations {
                        let from = r.get("from_id").and_then(|s| s.as_str()).unwrap_or("");
                        let to = r.get("to_id").and_then(|s| s.as_str()).unwrap_or("");
                        let rel = r.get("relation_type").and_then(|s| s.as_str()).unwrap_or("?");
                        let conf = r.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        if !from.is_empty() && !to.is_empty() {
                            top_relations.push((
                                id_to_name.get(from).cloned().unwrap_or_else(|| short(from)),
                                rel.to_string(),
                                id_to_name.get(to).cloned().unwrap_or_else(|| short(to)),
                                conf,
                            ));
                        }
                    }
                    top_relations.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
                }
            } else {
                md.push_str("（`kg.json` 中未找到 `entities`。）\n\n");
            }
        } else {
            md.push_str("（未找到 `kg.json`。）\n\n");
        }
        if !top_relations.is_empty() {
            md.push_str("**置信度最高的 10 条关系**：\n\n");
            md.push_str("| 头实体 | 关系 | 尾实体 | 置信度 |\n|---|---|---|---|\n");
            for (h, r, t, c) in top_relations.iter().take(10) {
                md.push_str(&format!("| {} | `{}` | {} | {:.2} |\n", h, r, t, c));
            }
            md.push('\n');
        }

        // ── 4. Hypotheses ───────────────────────────────────────────────
        md.push_str("## 4. 致病机理假说\n\n");
        let hyp_ids: Vec<String> = self.hypotheses.iter().map(|h| h.id.to_string()).collect();
        if hyp_ids.is_empty() {
            md.push_str("（未生成假说；详见 `project.json` 的失败事件。）\n\n");
        } else {
            md.push_str("### 4.1 概览\n\n");
            md.push_str("| # | 状态 | 假说 ID | 新颖度 | 置信度 | 核心陈述 |\n|---|---|---|---|---|---|\n");
            for (idx, id) in hyp_ids.iter().enumerate() {
                let body = full_by_id.get(id);
                let stmt = body
                    .and_then(|v| v.get("statement").and_then(|s| s.as_str()))
                    .unwrap_or("(未找到完整陈述)");
                let novelty = body
                    .and_then(|v| v.get("novelty").and_then(|s| s.as_str()))
                    .unwrap_or("—");
                let confidence = body
                    .and_then(|v| v.get("confidence").and_then(|c| c.as_f64()))
                    .unwrap_or(0.0);
                let badge = if self.hypotheses[idx].refined { "✨ 精炼" } else { "候选" };
                let stmt_short = truncate_chars(stmt, 110);
                md.push_str(&format!(
                    "| {} | {} | `{}` | {} | {:.2} | {} |\n",
                    idx + 1,
                    badge,
                    short(id),
                    novelty,
                    confidence,
                    stmt_short,
                ));
            }
            md.push('\n');

            md.push_str("### 4.2 假说详情\n\n");
            for (idx, id) in hyp_ids.iter().enumerate() {
                let body = full_by_id.get(id).copied();
                let badge = if self.hypotheses[idx].refined { "✨ 精炼" } else { "候选" };
                md.push_str(&format!(
                    "<details>\n<summary><strong>假说 #{} ({badge}) · <code>{id_short}</code></strong></summary>\n\n",
                    idx + 1,
                    badge = badge,
                    id_short = short(id),
                ));
                if let Some(body) = body {
                    let name = body.get("name").and_then(|s| s.as_str()).unwrap_or("");
                    let stmt = body.get("statement").and_then(|s| s.as_str()).unwrap_or("");
                    let novelty = body.get("novelty").and_then(|s| s.as_str()).unwrap_or("—");
                    let confidence = body
                        .get("confidence")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let mechanism = body.get("mechanism").and_then(|s| s.as_str()).unwrap_or("");

                    md.push_str(&format!("**名称**：{}\n\n", if name.is_empty() { "—" } else { name }));
                    md.push_str(&format!("**新颖度**：{}  ·  **置信度**：{:.2}\n\n", novelty, confidence));
                    md.push_str(&format!("**陈述**：\n\n{}\n\n", stmt));
                    if !mechanism.is_empty() {
                        md.push_str(&format!("**机制说明**：\n\n{}\n\n", mechanism));
                    }
                    push_evidence(&mut md, "**支持证据**", body.get("supporting_evidence"));
                    push_evidence(&mut md, "**反证 / 局限**", body.get("counter_evidence"));
                    push_experimental_design(&mut md, body.get("experimental_design"));
                    push_source_candidate(&mut md, body.get("source_candidate"), &full_by_id);
                } else {
                    md.push_str("（未在 `hypotheses_refined_full.json` / `hypotheses_full.json` 中找到完整定义，仅保留 `project.json` 中的索引信息。）\n\n");
                }
                md.push_str("</details>\n\n");
            }
        }

        // ── 5. Debate verdict ───────────────────────────────────────────
        let debate_value = debate_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        md.push_str("## 5. 假说辩论与裁决\n\n");
        if let Some(ref report) = debate_value {
            if let Some(rounds) = report.get("rounds").and_then(|v| v.as_array()) {
                md.push_str(&format!("共 **{}** 轮交叉质询。\n\n", rounds.len()));
            }
            if let Some(per) = report.get("per_hypothesis").and_then(|v| v.as_array()) {
                md.push_str("### 5.1 各假说裁决\n\n");
                md.push_str("| 假说 ID | 裁决 | 置信度 | 排名变化 |\n|---|---|---|---|\n");
                for ph in per {
                    let id = ph.get("hypothesis_id").and_then(|s| s.as_str()).unwrap_or("?");
                    let verdict = ph.get("verdict").and_then(|s| s.as_str()).unwrap_or("?");
                    let conf = ph.get("confidence_after").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let rank = ph.get("rank_after").and_then(|v| v.as_u64());
                    md.push_str(&format!(
                        "| `{}` | `{}` | {:.2} | {} |\n",
                        short(id),
                        verdict,
                        conf,
                        rank.map(|r| r.to_string()).unwrap_or_else(|| "—".into()),
                    ));
                }
                md.push('\n');
                // Detail per hypothesis: supporting / contradicting / refinement notes.
                md.push_str("### 5.2 各假说论据细节\n\n");
                for ph in per {
                    let id = ph.get("hypothesis_id").and_then(|s| s.as_str()).unwrap_or("?");
                    let verdict = ph.get("verdict").and_then(|s| s.as_str()).unwrap_or("?");
                    md.push_str(&format!(
                        "<details>\n<summary><strong><code>{}</code> · 裁决 <code>{verdict}</code></strong></summary>\n\n",
                        short(id),
                        verdict = verdict,
                    ));
                    push_bullets(&mut md, "**支持要点**", ph.get("supporting_points"));
                    push_bullets(&mut md, "**反对要点**", ph.get("contradicting_points"));
                    if let Some(notes) = ph.get("refinement_notes").and_then(|v| v.as_str()) {
                        if !notes.is_empty() {
                            md.push_str(&format!("**精炼说明**：\n\n{}\n\n", notes));
                        }
                    }
                    if let Some(opp) = ph.get("opponent_recommendation").and_then(|v| v.as_str()) {
                        if !opp.is_empty() {
                            md.push_str(&format!("**反方建议**：\n\n{}\n\n", opp));
                        }
                    }
                    md.push_str("</details>\n\n");
                }
            }

            // Cross-hypothesis contradictions & merge suggestions.
            if let Some(comp) = report.get("comparison").and_then(|v| v.as_object()) {
                if let Some(contra) = comp.get("contradictions_between").and_then(|v| v.as_array()) {
                    if !contra.is_empty() {
                        md.push_str("### 5.3 假说之间的矛盾\n\n");
                        for c in contra {
                            let a = c.get("a").and_then(|s| s.as_str()).unwrap_or("?");
                            let b = c.get("b").and_then(|s| s.as_str()).unwrap_or("?");
                            let reason = c.get("reason").and_then(|s| s.as_str()).unwrap_or("");
                            md.push_str(&format!(
                                "- <code>{}</code> ↔ <code>{}</code>：{}\n",
                                short(a),
                                short(b),
                                reason,
                            ));
                        }
                        md.push('\n');
                    }
                }
                if let Some(merges) = comp.get("merge_suggestions").and_then(|v| v.as_array()) {
                    if !merges.is_empty() {
                        md.push_str("### 5.4 合并建议\n\n");
                        for m in merges {
                            let text = m.as_str().unwrap_or("").to_string();
                            if !text.is_empty() {
                                md.push_str(&format!("- {}\n", text));
                            }
                        }
                        md.push('\n');
                    }
                }
                if let Some(strongest) = comp.get("strongest_hypothesis").and_then(|s| s.as_str()) {
                    md.push_str(&format!(
                        "**最强假说**：<code>{}</code>\n\n",
                        short(strongest),
                    ));
                }
                if let Some(rationale) = comp.get("ranking_rationale").and_then(|s| s.as_str()) {
                    if !rationale.is_empty() {
                        md.push_str(&format!("**排序理由**：\n\n{}\n\n", rationale));
                    }
                }
                if let Some(summary) = comp.get("summary").and_then(|s| s.as_str()) {
                    if !summary.is_empty() {
                        md.push_str(&format!("**裁判总结**：\n\n{}\n\n", summary));
                    }
                }
            }
        } else {
            md.push_str("（未找到 `debate_report.json`，可能未启用辩论阶段。）\n\n");
        }

        // ── 6. Validation plans ──────────────────────────────────────────
        md.push_str("## 6. 验证计划\n\n");
        // First executable action across all plans — quoted by the TL;DR.
        let mut first_action: Option<String> = None;
        if self.validation_plans.is_empty() {
            md.push_str("（未生成验证计划。可使用 `--validate` 启用。）\n\n");
        } else {
            for (i, p) in self.validation_plans.iter().enumerate() {
                // Relative plan paths (e.g. from a hand-restored manifest)
                // resolve against the project dir, never the process CWD.
                let plan_path = if p.is_absolute() {
                    p.clone()
                } else {
                    self.dir.join(p)
                };
                let plan: serde_json::Value = std::fs::read_to_string(&plan_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
                if first_action.is_none() {
                    let from_tasks = plan
                        .get("data_analysis_tasks")
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first())
                        .and_then(|t| t.get("objective"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let from_protocols = plan
                        .get("wet_lab_protocols")
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first())
                        .and_then(|t| t.get("objective"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    first_action = from_tasks
                        .or(from_protocols)
                        .map(|s| s.to_string());
                }
                let plan_file = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                let hyp_ref = plan.get("hypothesis_id").and_then(|s| s.as_str());
                md.push_str(&format!(
                    "### 6.{} 计划：`{plan_file}`{}\n\n",
                    i + 1,
                    match hyp_ref {
                        Some(id) => format!(" · 对应假说 `{}`", short(id)),
                        None => String::new(),
                    },
                    plan_file = plan_file,
                ));
                let rationale = plan.get("rationale").and_then(|s| s.as_str()).unwrap_or("");
                if !rationale.is_empty() {
                    md.push_str(&format!("**设计理由**：\n\n{}\n\n", rationale));
                }

                let tasks = plan.get("data_analysis_tasks").and_then(|v| v.as_array());
                if let Some(t) = tasks.filter(|t| !t.is_empty()) {
                    md.push_str(&format!("**数据分析任务**（{} 项）：\n\n", t.len()));
                    md.push_str("| ID | 数据集 | 方法 | 优先级 | 目标 |\n|---|---|---|---|---|\n");
                    for task in t {
                        let id = task.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                        let acc = task.get("dataset_accession").and_then(|s| s.as_str()).unwrap_or("(本地)");
                        let method = task.get("statistical_method").and_then(|s| s.as_str()).unwrap_or("");
                        let priority = task.get("priority").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let obj = task.get("objective").and_then(|s| s.as_str()).unwrap_or("");
                        md.push_str(&format!(
                            "| {} | `{}` | {} | {:.2} | {} |\n",
                            id,
                            acc,
                            truncate_chars(method, 60),
                            priority,
                            truncate_chars(obj, 90),
                        ));
                    }
                    md.push('\n');
                }

                let protocols = plan.get("wet_lab_protocols").and_then(|v| v.as_array());
                if let Some(protos) = protocols.filter(|p| !p.is_empty()) {
                    md.push_str(&format!("**湿实验方案**（{} 项）：\n\n", protos.len()));
                    for proto in protos {
                        let id = proto.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                        let obj = proto.get("objective").and_then(|s| s.as_str()).unwrap_or("");
                        let reagents = proto.get("reagents").and_then(|v| v.as_array());
                        let steps = proto.get("steps").and_then(|v| v.as_array());
                        let controls = proto.get("controls").and_then(|v| v.as_array());
                        let expected = proto.get("expected_outcome").and_then(|s| s.as_str()).unwrap_or("");
                        let timeline = proto.get("timeline_days").and_then(|v| v.as_u64());
                        let feasibility = proto.get("feasibility").and_then(|v| v.as_f64());

                        md.push_str(&format!(
                            "<details>\n<summary><strong>湿实验 {}</strong>：{}</summary>\n\n",
                            id,
                            truncate_chars(obj, 120),
                        ));
                        md.push_str(&format!("**目标**：\n\n{}\n\n", obj));
                        if let Some(rs) = reagents {
                            if !rs.is_empty() {
                                md.push_str("**试剂**：\n\n");
                                for r in rs {
                                    let s = r.as_str().unwrap_or("").to_string();
                                    if !s.is_empty() {
                                        md.push_str(&format!("- {}\n", s));
                                    }
                                }
                                md.push('\n');
                            }
                        }
                        if let Some(st) = steps {
                            if !st.is_empty() {
                                md.push_str("**步骤**：\n\n");
                                for (n, step) in st.iter().enumerate() {
                                    let s = step.as_str().unwrap_or("").to_string();
                                    if !s.is_empty() {
                                        md.push_str(&format!("{}. {}\n", n + 1, s));
                                    }
                                }
                                md.push('\n');
                            }
                        }
                        if let Some(cs) = controls {
                            if !cs.is_empty() {
                                md.push_str("**对照**：\n\n");
                                for c in cs {
                                    let s = c.as_str().unwrap_or("").to_string();
                                    if !s.is_empty() {
                                        md.push_str(&format!("- {}\n", s));
                                    }
                                }
                                md.push('\n');
                            }
                        }
                        if !expected.is_empty() {
                            md.push_str(&format!("**预期结果**：\n\n{}\n\n", expected));
                        }
                        let meta_bits: Vec<String> = [
                            timeline.map(|d| format!("周期 ≈ {} 天", d)),
                            feasibility.map(|f| format!("可行性 {:.2}", f)),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        if !meta_bits.is_empty() {
                            md.push_str(&format!("**元数据**：{}\n\n", meta_bits.join(" · ")));
                        }
                        md.push_str("</details>\n\n");
                    }
                }
            }
        }

        // ── 7. Data-analysis delivery ───────────────────────────────────
        md.push_str("## 7. 数据分析交付\n\n");
        if self.analyses.is_empty() {
            md.push_str("（未运行任何数据分析任务。可使用 `--analyze` 启用。）\n\n");
        } else {
            // Group analyses by hypothesis_id so a researcher can audit each
            // hypothesis' evidence together.
            use std::collections::BTreeMap;
            let mut by_hyp: BTreeMap<String, Vec<&AnalysisRef>> = BTreeMap::new();
            for a in &self.analyses {
                let key = a
                    .hypothesis_id
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "(未关联假说)".into());
                by_hyp.entry(key).or_default().push(a);
            }
            md.push_str(&format!(
                "共 **{}** 项数据分析任务，覆盖 **{}** 个假说。\n\n",
                self.analyses.len(),
                by_hyp.len(),
            ));
            for (pos, (hyp, group)) in by_hyp.iter().enumerate() {
                md.push_str(&format!(
                    "### 7.{} 假说 `{}` 的数据分析\n\n",
                    pos + 1,
                    short(hyp),
                ));
                md.push_str("| 任务 | 后端 | 状态 | Notebook | 溯源 |\n|---|---|---|---|---|\n");
                for a in group {
                    let nb = a
                        .notebook_path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("—");
                    let pv = a
                        .provenance_path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("—");
                    let status = if a.success { "✅ 成功" } else { "❌ 失败" };
                    md.push_str(&format!(
                        "| {} | {} | {} | `{}` | `{}` |\n",
                        a.task_id,
                        if a.execution_backend.is_empty() { "—" } else { &a.execution_backend },
                        status,
                        nb,
                        pv,
                    ));
                }
                md.push('\n');
                md.push_str("每个 `analysis.ipynb` 包含该任务的全部计算步骤、注释与图表，可在 Jupyter 中重放；`provenance.json` 记录脚本哈希、输入输出文件、运行环境与执行时间。\n\n");
            }
        }

        // ── 8. Citation index ────────────────────────────────────────────
        if let Some(ref papers) = papers_value {
            md.push_str(&format!("## 8. 引用索引（{} 篇）\n\n", papers.len()));
            md.push_str("<details><summary>展开 / 折叠引用列表</summary>\n\n");
            md.push_str("| # | PMID | 标题 |\n|---|---|---|\n");
            for (i, p) in papers.iter().enumerate() {
                let (pmid, title, _year) = extract_paper_meta(p);
                let pmid_link = if pmid != "?" {
                    format!("[{pmid}](https://pubmed.ncbi.nlm.nih.gov/{pmid}/)")
                } else {
                    "—".to_string()
                };
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    i + 1,
                    pmid_link,
                    truncate_chars(&title, 140),
                ));
            }
            md.push_str("\n</details>\n\n");
        }

        // ── 9. Audit pointers ────────────────────────────────────────────
        md.push_str("## 9. 审计与复现\n\n");
        md.push_str("- `project.json` — 全流水线阶段状态 + append-only 事件日志（机器可读）\n");
        md.push_str("- `run_report.md` — 阶段时长表 + 事件时间线（运维可读）\n");
        md.push_str("- `kg.json` — 完整知识图谱（实体 + 关系）\n");
        md.push_str("- `papers.json` — 原始文献清单与摘要\n");
        md.push_str("- `hypotheses_full.json` / `hypotheses_refined_full.json` — 假说全集与精炼集\n");
        md.push_str("- `debate_report.json` — 辩论每轮论据 / 反驳 / 裁决 / 合并建议\n");
        md.push_str("- `plans/validation_plan_*.json` — 验证计划全文（含变量、统计方法、湿实验步骤）\n");
        md.push_str("- `analysis/plan_*/analysis/*/analysis.ipynb` — 可重放的分析 notebook\n");
        md.push_str("- `analysis/plan_*/analysis/*/provenance.json` — 输入 / 输出 / 脚本哈希 / 环境溯源\n\n");
        md.push_str("---\n\n");
        md.push_str("*本报告由 miniagent research 流水线自动生成。所有原始数据可在项目目录内复现与重跑。*\n");

        // Splice in the TL;DR now that every section's numbers are known.
        let tldr = build_tldr(
            self,
            papers_value.as_ref(),
            kg_value.as_ref(),
            &full_by_id,
            debate_value.as_ref(),
            first_action.as_deref(),
        );
        md = md.replace("<<TLDR_PLACEHOLDER>>", tldr.trim_end());

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

// ───────────────────────────── helpers ─────────────────────────────
//
// These are pure formatting helpers used by `write_user_report`. They live
// outside the `impl` so the body of that method stays compact and they can
// be unit-tested individually.

/// Render the report's TL;DR section.
///
/// Pulls the headline numbers (papers, KG size, hypothesis count, executed
/// analyses) plus the debate's strongest hypothesis into 5–8 lines a reader
/// can act on without scrolling. Degrades gracefully when a stage produced
/// nothing: each line simply drops its missing part.
#[allow(clippy::too_many_arguments)]
fn build_tldr(
    manifest: &ProjectManifest,
    papers: Option<&Vec<serde_json::Value>>,
    kg: Option<&serde_json::Value>,
    full_by_id: &std::collections::HashMap<String, &serde_json::Value>,
    debate: Option<&serde_json::Value>,
    first_action: Option<&str>,
) -> String {
    let mut md = String::from("## TL;DR\n\n");

    // ── 核心结论：辩论裁决的最强假说，否则置信度最高的假说 ──
    let strongest = debate
        .and_then(|d| d.get("comparison"))
        .and_then(|c| c.get("strongest_hypothesis"))
        .and_then(|s| s.as_str())
        .and_then(|id| full_by_id.get(id).map(|v| (id.to_string(), *v)));
    let fallback = manifest
        .hypotheses
        .iter()
        .filter_map(|h| {
            let id = h.id.to_string();
            let body = full_by_id.get(&id)?;
            let conf = body.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
            Some((conf, id, *body))
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, id, body)| (id, body));
    if let Some((id, body)) = strongest.or(fallback) {
        let stmt = body
            .get("statement")
            .and_then(|v| v.as_str())
            .unwrap_or("(见第 4 节假说详情)");
        let conf = body.get("confidence").and_then(|v| v.as_f64());
        let conf_note = match conf {
            Some(c) => format!("（当前置信度 {:.2}）", c),
            None => String::new(),
        };
        md.push_str(&format!(
            "**核心结论**：在本次文献证据与知识图谱链路预测的支持下，最值得优先验证的致病机理假说是 **假说 `{}`**{}：\n\n> {}\n\n",
            short(&id),
            conf_note,
            truncate_chars(stmt, 200),
        ));
    }

    // ── 关键数字 ──
    let mut bits: Vec<String> = Vec::new();
    if let Some(n) = papers.map(|p| p.len()) {
        bits.push(format!("检索并精读 **{} 篇**文献", n));
    }
    if let Some(kg) = kg {
        let e = kg.get("entities").and_then(|v| v.as_array()).map(|a| a.len());
        let r = kg.get("relations").and_then(|v| v.as_array()).map(|a| a.len());
        if let (Some(e), Some(r)) = (e, r) {
            bits.push(format!("构建 **{} 实体 / {} 关系**的知识图谱", e, r));
        }
    }
    if !manifest.hypotheses.is_empty() {
        bits.push(format!("提出 **{} 个**致病机理假说", manifest.hypotheses.len()));
    }
    if !manifest.validation_plans.is_empty() {
        bits.push(format!("生成 **{} 份**验证计划", manifest.validation_plans.len()));
    }
    if !manifest.analyses.is_empty() {
        let ok = manifest.analyses.iter().filter(|a| a.success).count();
        bits.push(format!(
            "端到端执行 **{}/{} 项**数据分析任务（notebook 可重放）",
            ok,
            manifest.analyses.len()
        ));
    }
    if !bits.is_empty() {
        md.push_str(&format!("- {}\n", bits.join("；")));
    }

    // ── 建议下一步 ──
    if let Some(action) = first_action {
        md.push_str(&format!(
            "- **建议下一步**：{}\n",
            truncate_chars(action, 160)
        ));
    }

    md.push_str("\n完整论据链见下文各节；审计与复现入口见第 9 节。\n\n");
    md
}

/// First 8 characters of an id, falling back to the whole string.
fn short(id: &str) -> String {
    if id.chars().count() >= 8 {
        id.chars().take(8).collect()
    } else {
        id.to_string()
    }
}

/// Truncate a string to `max_chars` Unicode characters and append an
/// ellipsis when truncation occurs.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

/// Extract (pmid, title, year) from one entry in `papers.json`.
/// Supports the legacy `[pmid, text]` tuple form AND the object form
/// `{pmid, title, year, ...}`.
fn extract_paper_meta(p: &serde_json::Value) -> (String, String, Option<String>) {
    if let Some(arr) = p.as_array() {
        let pmid = arr
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let text = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
        // Text format observed in `papers.json` (legacy): "<n>. <journal>... <title>."
        let title = extract_title_from_text(text);
        return (pmid, title, None);
    }
    let pmid = p
        .get("pmid")
        .and_then(|v| v.as_str())
        .or_else(|| p.get("id").and_then(|v| v.as_str()))
        .unwrap_or("?")
        .to_string();
    let title = p
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let year = p
        .get("year")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (pmid, title, year)
}

/// Pull a title out of the `<n>. <journal block>... <title>.` blob format
/// that legacy `papers.json` writes.
fn extract_title_from_text(text: &str) -> String {
    // Strip leading numbering: "12. <text>"
    let trimmed = text.trim_start();
    let after_num = trimmed
        .strip_prefix(|c: char| c.is_ascii_digit() || c == '.')
        .map(|s| s.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' '))
        .unwrap_or(trimmed);
    // Look for the first ". " that ends a title-like sentence. Most titles
    // in PubMed-format blobs end before "[Article in ...]" or "doi:" markers.
    for marker in [" doi:", "[Article in", " PMID:", " PMCID:"] {
        if let Some(idx) = after_num.find(marker) {
            let head = &after_num[..idx];
            // Find the last sentence-terminating ". " before the marker.
            if let Some(last_dot) = head.rfind(". ") {
                return head[..last_dot].to_string();
            }
            return head.to_string();
        }
    }
    after_num.to_string()
}

/// Render an evidence list section (`supporting_evidence` or
/// `counter_evidence`) under a heading. Skips empty arrays.
fn push_evidence(md: &mut String, heading: &str, value: Option<&serde_json::Value>) {
    if let Some(arr) = value.and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        md.push_str(&format!("{heading}：\n\n"));
        for item in arr {
            if let Some(s) = item.as_str() {
                if !s.is_empty() {
                    md.push_str(&format!("- {}\n", s));
                }
            }
        }
        md.push('\n');
    }
}

/// Render a `experimental_design` block (approach / methods /
/// expected_outcomes / controls / feasibility).
fn push_experimental_design(md: &mut String, value: Option<&serde_json::Value>) {
    let Some(design) = value else { return };
    if design.is_null() {
        return;
    }
    md.push_str("**实验设计**：\n\n");
    if let Some(approach) = design.get("approach").and_then(|v| v.as_str()) {
        if !approach.is_empty() {
            md.push_str(&format!("- **整体思路**：{}\n", approach));
        }
    }
    if let Some(methods) = design.get("methods").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        md.push_str("- **方法**：\n");
        for m in methods {
            if let Some(s) = m.as_str() {
                if !s.is_empty() {
                    md.push_str(&format!("  - {}\n", s));
                }
            }
        }
    }
    if let Some(expected) = design.get("expected_outcomes").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        md.push_str("- **预期结果**：\n");
        for e in expected {
            if let Some(s) = e.as_str() {
                if !s.is_empty() {
                    md.push_str(&format!("  - {}\n", s));
                }
            }
        }
    }
    if let Some(controls) = design.get("controls").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        md.push_str("- **对照**：\n");
        for c in controls {
            if let Some(s) = c.as_str() {
                if !s.is_empty() {
                    md.push_str(&format!("  - {}\n", s));
                }
            }
        }
    }
    if let Some(feas) = design.get("feasibility").and_then(|v| v.as_f64()) {
        md.push_str(&format!("- **可行性评分**：{:.2}\n", feas));
    }
    md.push('\n');
}

/// Render the link-prediction source candidate (head / relation / tail /
/// supporting paths) so the report shows *why* the KG surfaced each
/// hypothesis.
fn push_source_candidate(
    md: &mut String,
    value: Option<&serde_json::Value>,
    _full_by_id: &std::collections::HashMap<String, &serde_json::Value>,
) {
    let Some(src) = value else { return };
    let head = src.get("head").and_then(|v| v.as_str()).unwrap_or("?");
    let rel = src.get("relation").and_then(|v| v.as_str()).unwrap_or("?");
    let tail = src.get("tail").and_then(|v| v.as_str()).unwrap_or("?");
    let score = src.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
    md.push_str(&format!(
        "**链路预测来源**：<code>{head}</code> — `{rel}` → <code>{tail}</code> · 综合分 {score:.3}\n\n",
    ));
    if let Some(ev) = src.get("evidence").and_then(|v| v.as_object()) {
        if let Some(novelty) = ev.get("novelty").and_then(|v| v.as_str()) {
            md.push_str(&format!("- 新颖度：{}\n", novelty));
        }
        if let Some(kge) = ev.get("kge_score").and_then(|v| v.as_f64()) {
            md.push_str(&format!("- KGE 分数：{:.3}\n", kge));
        }
        if let Some(path) = ev.get("path_score").and_then(|v| v.as_f64()) {
            md.push_str(&format!("- 路径分数：{:.3}\n", path));
        }
        if let Some(give) = ev.get("give_score").and_then(|v| v.as_f64()) {
            md.push_str(&format!("- GIVE 外推分数：{:.3}\n", give));
        }
    }
    md.push('\n');
}

/// Render a list of strings (`supporting_points`, `contradicting_points`,
/// `reagents`, …) under a heading. Skips empty / missing arrays.
fn push_bullets(md: &mut String, heading: &str, value: Option<&serde_json::Value>) {
    if let Some(arr) = value.and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        md.push_str(&format!("{heading}：\n\n"));
        for item in arr {
            if let Some(s) = item.as_str() {
                if !s.is_empty() {
                    md.push_str(&format!("- {}\n", s));
                }
            }
        }
        md.push('\n');
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

    fn fixture_hypothesis(id: &str, statement: &str, confidence: f64) -> serde_json::Value {
        json!({
            "id": id,
            "name": "test hypothesis",
            "statement": statement,
            "novelty": "medium",
            "confidence": confidence,
            "mechanism": "A regulates B downstream of C",
            "supporting_evidence": ["evidence one", "evidence two"],
            "counter_evidence": ["one contradiction"],
        })
    }

    #[test]
    fn user_report_full_run_renders_all_sections() {
        let dir = std::env::temp_dir().join("miniagent_research_user_report_full");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plans")).unwrap();
        std::fs::write(
            dir.join("papers.json"),
            r#"[{"pmid":"12345","title":"A paper about ALS","year":"2023"}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("kg.json"),
            r#"{"entities":[{"id":"e1","name":"SOD1","entity_type":"Gene"},
                            {"id":"e2","name":"ALS","entity_type":"Disease"}],
                "relations":[{"from_id":"e1","to_id":"e2","relation_type":"associated_with","confidence":0.9}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("hypotheses_refined_full.json"),
            serde_json::to_string(&vec![
                fixture_hypothesis("11111111-1111-1111-1111-111111111111", "SOD1 aggregation drives motor neuron death", 0.8),
                fixture_hypothesis("22222222-2222-2222-2222-222222222222", "Glutamate excitotoxicity cascades", 0.6),
            ])
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("debate_report.json"),
            json!({
                "rounds": [{}],
                "per_hypothesis": [{
                    "hypothesis_id": "11111111-1111-1111-1111-111111111111",
                    "verdict": "supported",
                    "confidence_after": 0.85,
                    "supporting_points": ["s1"],
                    "contradicting_points": ["c1"],
                    "refinement_notes": "tightened"
                }],
                "comparison": {
                    "strongest_hypothesis": "11111111-1111-1111-1111-111111111111",
                    "merge_suggestions": ["merge 2 into 1"],
                    "summary": "judge summary"
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("plans/validation_plan_1.json"),
            json!({
                "hypothesis_id": "11111111-1111-1111-1111-111111111111",
                "rationale": "test rationale",
                "data_analysis_tasks": [{
                    "id": "DA-1", "dataset_accession": "GSE1",
                    "statistical_method": "DESeq2", "priority": 0.9,
                    "objective": "Differential expression of SOD1 targets"
                }],
                "wet_lab_protocols": [{
                    "id": "WL-1", "objective": "Aggregate assay",
                    "reagents": ["SOD1 plasmid"], "steps": ["transfect", "measure"],
                    "controls": ["empty vector"], "expected_outcome": "aggregates",
                    "timeline_days": 14, "feasibility": 0.7
                }]
            })
            .to_string(),
        )
        .unwrap();

        let mut m = ProjectManifest::new("ALS pathogenesis", dir.clone());
        m.record_stage("search", StageStatus::Completed, std::time::Duration::from_secs(1), vec!["papers.json".into()], None);
        m.set_kg_stats(json!({"entities": 2, "relations": 1}));
        m.record_hypotheses(vec![
            HypothesisRef::new(
                Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
                "SOD1 aggregation drives motor neuron death", None,
            ).with_refined(true),
        ]);
        m.record_debate("debate_report.json");
        m.add_validation_plan("plans/validation_plan_1.json");
        m.record_analysis(AnalysisRef {
            task_id: "DA-1".into(),
            hypothesis_id: Some(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()),
            notebook_path: Some("analysis/plan_1/analysis/DA-1/analysis.ipynb".into()),
            provenance_path: Some("analysis/plan_1/analysis/DA-1/provenance.json".into()),
            success: true,
            execution_backend: "jupyter".into(),
        });

        let path = m.write_user_report("brief_test").unwrap();
        let report = std::fs::read_to_string(&path).unwrap();
        for expected in [
            "## TL;DR",
            "核心结论",
            "SOD1 aggregation",
            "## 1. 研究问题",
            "## 2. 文献概览",
            "12345",
            "## 3. 知识图谱概要",
            "## 4. 致病机理假说",
            "## 5. 假说辩论与裁决",
            "## 6. 验证计划",
            "## 7. 数据分析交付",
            "## 8. 引用索引",
            "## 9. 审计与复现",
            "建议下一步",
            "Differential expression",
        ] {
            assert!(report.contains(expected), "report missing {expected:?}");
        }
        assert!(!report.contains("<<TLDR_PLACEHOLDER>>"), "TL;DR placeholder leaked");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn user_report_partial_run_still_renders() {
        // A run directory with NO stage artifacts (e.g. aborted after the
        // key check) must still produce a graceful report — every section
        // degrades to a placeholder instead of failing.
        let dir = std::env::temp_dir().join("miniagent_research_user_report_partial");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let m = ProjectManifest::new("ALS pathogenesis", dir.clone());
        let path = m.write_user_report("brief_partial").unwrap();
        let report = std::fs::read_to_string(&path).unwrap();
        assert!(report.contains("## TL;DR"));
        assert!(report.contains("## 4. 致病机理假说"));
        assert!(report.contains("未找到 `papers.json`"));
        assert!(report.contains("## 9. 审计与复现"));
        assert!(!report.contains("<<TLDR_PLACEHOLDER>>"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
