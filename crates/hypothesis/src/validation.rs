//! Structured validation plans derived from a hypothesis.
//!
//! A [`ValidationPlan`] splits the work to *test* a hypothesis into two
//! deliberately separate tracks:
//!
//! - **Data-analysis tasks** — computational, reproducible analyses over
//!   existing public datasets (GEO / TCGA / ArrayExpress) or local data files.
//!   These are directly executable by the analysis runner (see `miniagent-analysis`).
//! - **Wet-lab protocols** — bench procedures that cannot be automated
//!   (reagents, steps, controls, expected outcomes).
//!
//! Keeping the two tracks structurally distinct is what lets the agent execute
//! the computational track end-to-end while still delivering a complete
//! experimental plan to the researcher.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A complete validation plan for one hypothesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationPlan {
    /// The hypothesis this plan validates.
    pub hypothesis_id: Uuid,
    /// One-paragraph rationale linking the hypothesis to the chosen validations.
    pub rationale: String,
    /// Computational analyses over existing datasets (directly executable).
    pub data_analysis_tasks: Vec<DataAnalysisTask>,
    /// Wet-lab procedures (human-executed, not automated).
    pub wet_lab_protocols: Vec<WetLabProtocol>,
}

impl ValidationPlan {
    pub fn task_count(&self) -> usize {
        self.data_analysis_tasks.len() + self.wet_lab_protocols.len()
    }

    /// Deterministic fallback plan for when LLM generation fails repeatedly
    /// (e.g. a reasoning model returns empty content on every attempt). One
    /// standard differential-expression task over GEO + a placeholder wet-lab
    /// protocol, anchored on the hypothesis statement. The pipeline's GEO
    /// grounding step backfills a concrete accession afterwards.
    ///
    /// Better a generic, executable plan than a silently missing one — the
    /// fallback is recorded in the audit log by the caller.
    pub fn minimal(hypothesis: &super::Hypothesis) -> Self {
        let gene = hypothesis
            .statement
            .split_whitespace()
            .find(|w| w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/'))
            .unwrap_or("target gene")
            .to_string();
        Self {
            hypothesis_id: hypothesis.id,
            rationale: format!(
                "Fallback plan (LLM generation failed): standard differential-expression \
                 validation of the hypothesis '{}' across public Alzheimer's-relevant \
                 expression datasets.",
                hypothesis.statement
            ),
            data_analysis_tasks: vec![DataAnalysisTask {
                id: "DA-1".into(),
                objective: format!(
                    "Test whether {gene}-related expression differs between disease and \
                     control cohorts, as predicted by the hypothesis"
                ),
                dataset_source: DatasetSource::Geo,
                dataset_accession: None,
                cohort_definition: "disease vs control".into(),
                variables: AnalysisVariables {
                    independent: vec![format!("{gene} expression")],
                    dependent: vec!["disease status".into()],
                    covariates: vec!["age".into(), "sex".into()],
                },
                statistical_method: "differential expression (t-test with BH FDR correction)"
                    .into(),
                expected_outcome: "significant expression difference between cohorts in the \
                                   direction predicted by the hypothesis"
                    .into(),
                deliverable: "DE table CSV + volcano plot PNG".into(),
                priority: 0.5,
            }],
            wet_lab_protocols: vec![WetLabProtocol {
                id: "WL-1".into(),
                objective: "Confirm the expression change by qPCR on an independent cohort"
                    .into(),
                reagents: vec!["RNA extraction kit".into(), "reverse transcription kit".into()],
                steps: vec![
                    "Extract total RNA from cohort samples".into(),
                    "Reverse-transcribe to cDNA".into(),
                    "Run qPCR with gene-specific primers and housekeeping controls".into(),
                ],
                controls: vec!["housekeeping gene (GAPDH)".into(), "no-template control".into()],
                expected_outcome: "qPCR fold-change agrees with the in-silico direction".into(),
                timeline_days: Some(5),
                feasibility: 0.9,
            }],
        }
    }
}

/// One executable computational analysis task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataAnalysisTask {
    /// Stable task identifier, e.g. `"DA-1"`.
    pub id: String,
    /// What this analysis is meant to establish.
    pub objective: String,
    /// Where the data comes from.
    pub dataset_source: DatasetSource,
    /// Concrete accession / path / URL when known (e.g. `"GSE12345"`).
    #[serde(default)]
    pub dataset_accession: Option<String>,
    /// How to define the cohorts / comparison groups.
    pub cohort_definition: String,
    /// Variables involved in the analysis.
    pub variables: AnalysisVariables,
    /// Statistical method to apply (e.g. `"limma DE"`, `"Cox regression"`).
    pub statistical_method: String,
    /// Outcome that would support (or refute) the hypothesis.
    pub expected_outcome: String,
    /// Concrete deliverable (e.g. `"volcano plot + DE gene table CSV"`).
    pub deliverable: String,
    /// Priority in `[0, 1]` (higher = more central to the hypothesis).
    #[serde(default = "default_priority")]
    pub priority: f64,
}

/// Variables for a data-analysis task.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisVariables {
    /// Independent / predictor variables (e.g. gene expression).
    #[serde(default)]
    pub independent: Vec<String>,
    /// Dependent / outcome variables (e.g. disease status, survival).
    #[serde(default)]
    pub dependent: Vec<String>,
    /// Covariates to control for (e.g. age, sex, batch).
    #[serde(default)]
    pub covariates: Vec<String>,
}

/// Where a data-analysis task sources its data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DatasetSource {
    /// NCBI Gene Expression Omnibus.
    Geo,
    /// The Cancer Genome Atlas (via GDC).
    Tcga,
    /// EBI ArrayExpress.
    ArrayExpress,
    /// A local file the user supplied (path relative to the working directory).
    Local(String),
    /// A custom downloadable URL.
    CustomUrl(String),
}

impl DatasetSource {
    /// True when the data is immediately available without a network download
    /// the runner must perform itself (local files, or accessions resolved by
    /// the caller before execution).
    pub fn is_locally_available(&self) -> bool {
        matches!(self, DatasetSource::Local(_))
    }
}

/// A wet-lab protocol (bench work, not automated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WetLabProtocol {
    /// Stable protocol identifier, e.g. `"WL-1"`.
    pub id: String,
    /// Scientific objective of the experiment.
    pub objective: String,
    /// Required reagents / antibodies / cell lines / kits.
    #[serde(default)]
    pub reagents: Vec<String>,
    /// Ordered experimental steps.
    #[serde(default)]
    pub steps: Vec<String>,
    /// Controls (positive / negative / vehicle).
    #[serde(default)]
    pub controls: Vec<String>,
    /// Outcome expected if the hypothesis holds.
    pub expected_outcome: String,
    /// Approximate duration in days, if estimable.
    #[serde(default)]
    pub timeline_days: Option<u32>,
    /// Feasibility in `[0, 1]` (cost / difficulty / time).
    #[serde(default = "default_priority")]
    pub feasibility: f64,
}

fn default_priority() -> f64 {
    0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_plan_roundtrips_json() {
        let plan = ValidationPlan {
            hypothesis_id: Uuid::new_v4(),
            rationale: "Test BRCA1 DNA-repair role in breast cancer.".into(),
            data_analysis_tasks: vec![DataAnalysisTask {
                id: "DA-1".into(),
                objective: "Quantify BRCA1 differential expression".into(),
                dataset_source: DatasetSource::Geo,
                dataset_accession: Some("GSE12345".into()),
                cohort_definition: "tumor vs adjacent normal".into(),
                variables: AnalysisVariables {
                    independent: vec!["BRCA1 expression".into()],
                    dependent: vec!["tumor/normal status".into()],
                    covariates: vec!["age".into(), "batch".into()],
                },
                statistical_method: "limma DE".into(),
                expected_outcome: "BRCA1 downregulated in tumor".into(),
                deliverable: "volcano plot + DE table".into(),
                priority: 0.9,
            }],
            wet_lab_protocols: vec![WetLabProtocol {
                id: "WL-1".into(),
                objective: "Confirm by western blot".into(),
                reagents: vec!["anti-BRCA1 antibody".into()],
                steps: vec!["lyse cells".into(), "run SDS-PAGE".into()],
                controls: vec!["GAPDH loading control".into()],
                expected_outcome: "Reduced BRCA1 band in tumor".into(),
                timeline_days: Some(3),
                feasibility: 0.8,
            }],
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: ValidationPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_count(), 2);
        assert_eq!(back.data_analysis_tasks[0].id, "DA-1");
        assert_eq!(back.wet_lab_protocols[0].timeline_days, Some(3));
    }

    #[test]
    fn dataset_source_local_availability() {
        assert!(DatasetSource::Local("data.csv".into()).is_locally_available());
        assert!(!DatasetSource::Geo.is_locally_available());
        assert!(!DatasetSource::CustomUrl("https://x/y".into()).is_locally_available());
    }

    #[test]
    fn dataset_source_enum_tagged_serialization() {
        let s = serde_json::to_string(&DatasetSource::Local("foo.csv".into())).unwrap();
        assert!(s.contains("\"kind\":\"local\""));
        assert!(s.contains("\"value\":\"foo.csv\""));
        let g = serde_json::to_string(&DatasetSource::Geo).unwrap();
        assert!(g.contains("\"kind\":\"geo\""));
    }
}
