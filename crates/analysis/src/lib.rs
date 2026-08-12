//! End-to-end executable data-analysis track for validation plans.
//!
//! Given a [`DataAnalysisTask`](miniagent_hypothesis::DataAnalysisTask), the
//! [`AnalysisRunner`] generates a reproducible script, provisions a conda
//! environment, executes it, and records a full [`ProvenanceRecord`] so every
//! result is auditable and re-runnable. Notebook execution is also supported
//! via [`execute_notebook`](notebook::execute_notebook).

pub mod notebook;
pub mod provenance;
pub mod runner;

pub use notebook::{execute_notebook, jupyter_available, NotebookResult};
pub use provenance::{
    current_git_commit, fnv1a_hex, preview, record_dir_shallow, record_file, FileRecord,
    ProvenanceRecord,
};
pub use runner::{AnalysisResult, AnalysisRunner, RunOpts};
