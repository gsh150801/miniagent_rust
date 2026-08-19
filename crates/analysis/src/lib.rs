//! End-to-end executable data-analysis track for validation plans.
//!
//! Given a [`DataAnalysisTask`](miniagent_hypothesis::DataAnalysisTask), the
//! [`AnalysisRunner`] generates a reproducible script, provisions a conda
//! environment, executes it, and records a full [`ProvenanceRecord`] so every
//! result is auditable and re-runnable. Notebook execution is also supported
//! via [`execute_notebook`](notebook::execute_notebook).

pub mod geo;
pub mod notebook;
pub mod notebook_gen;
pub mod provenance;
pub mod runner;

pub use geo::{clean_series_matrix, download_geo_series_matrix, geo_bucket};

pub use notebook::{execute_notebook, jupyter_available, NotebookResult};
pub use notebook_gen::{build_notebook, split_code_into_cells, write_notebook};
pub use provenance::{
    current_git_commit, preview, record_dir_shallow, record_file, sha256_hex, FileRecord,
    ProvenanceRecord,
};
pub use runner::{AnalysisResult, AnalysisRunner, RunOpts};
