//! Notebook execution (best-effort, via `jupyter nbconvert --execute`).
//!
//! Bridges the existing `NotebookEditTool` (which edits `.ipynb` cells but never
//! runs them) to actual execution. When Jupyter is not installed, the call fails
//! with a clear error so callers can fall back (e.g. export a Python script).

use miniagent_core::error::AgentError;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Result of executing a notebook in place.
#[derive(Debug, Clone)]
pub struct NotebookResult {
    pub notebook_path: PathBuf,
    pub exit_code: i32,
    pub duration: Duration,
    pub stdout: String,
    pub stderr: String,
}

/// Check whether `jupyter nbconvert` is actually usable on PATH.
///
/// `jupyter --version` succeeds even when the nbconvert subcommand is not
/// installed (observed on machines with a partial pip install), which used to
/// make every execution attempt fail and fall back to the raw script.
pub fn jupyter_available() -> bool {
    std::process::Command::new("jupyter")
        .args(["nbconvert", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Execute a notebook in place via `jupyter nbconvert --to notebook --execute`.
///
/// Writes the executed notebook back to `output_path` (or over the input when
/// `output_path` is `None`). Returns a non-error `NotebookResult` with a non-zero
/// exit code on execution failure; returns an `AgentError` only when Jupyter is
/// missing or the subprocess cannot be spawned.
pub fn execute_notebook(
    notebook_path: &Path,
    output_path: Option<&Path>,
    timeout_secs: u64,
) -> Result<NotebookResult, AgentError> {
    if !notebook_path.exists() {
        return Err(AgentError::invalid_config(format!(
            "notebook not found: {}",
            notebook_path.display()
        )));
    }
    if !jupyter_available() {
        return Err(AgentError::invalid_config(
            "jupyter is not installed; cannot execute notebook. \
             Install with `pip install jupyter` or export the notebook to a .py script."
                .to_string(),
        ));
    }

    let out = output_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| notebook_path.to_path_buf());

    let started = std::time::Instant::now();
    let output = std::process::Command::new("jupyter")
        .args([
            "nbconvert",
            "--to",
            "notebook",
            "--execute",
            "--output",
            &out.to_string_lossy(),
            &notebook_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| {
            AgentError::tool(
                "analysis.notebook",
                format!("failed to spawn jupyter nbconvert: {e}"),
            )
        })?;
    let duration = started.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);

    let _ = timeout_secs; // honored by nbconvert's own cell timeout in future; reserved.

    Ok(NotebookResult {
        notebook_path: out,
        exit_code,
        duration,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_notebook_errors() {
        let res = execute_notebook(Path::new("/nonexistent/path.ipynb"), None, 120);
        assert!(res.is_err());
    }
}
