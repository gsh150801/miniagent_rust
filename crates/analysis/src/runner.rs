//! End-to-end execution of a data-analysis task.
//!
//! [`AnalysisRunner`] takes a [`DataAnalysisTask`] (from a validation plan) and:
//! 1. generates a reproducible analysis script via an LLM,
//! 2. ensures a conda environment with the required packages,
//! 3. executes the script,
//! 4. captures a full [`ProvenanceRecord`] (script + I/O hashes + env + seed + git).
//!
//! When conda is unavailable the runner falls back to system `python` and records
//! that in provenance. When no local data is supplied (e.g. a GEO accession not
//! yet downloaded), the runner still produces the script + plan as a dry-run
//! deliverable so the researcher can execute it manually.

use chrono::Utc;
use miniagent_core::error::AgentError;
use miniagent_core::json_util;
use miniagent_hypothesis::{DataAnalysisTask, DatasetSource};
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::provenance::{
    current_git_commit, preview, record_dir_bounded, record_file, sha256_hex, FileRecord,
    ProvenanceRecord,
};

/// Tunable options for one analysis run.
#[derive(Debug, Clone)]
pub struct RunOpts {
    pub seed: u64,
    pub conda_env: String,
    /// Extra Python packages to ensure in the conda env.
    pub extra_packages: Vec<String>,
    /// Path to a local data file (overrides task dataset_source when set).
    pub local_data: Option<PathBuf>,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            seed: 42,
            conda_env: "mn_analysis".to_string(),
            extra_packages: vec![
                "pandas".into(),
                "numpy".into(),
                "scipy".into(),
                "scikit-learn".into(),
                "matplotlib".into(),
                "statsmodels".into(),
            ],
            local_data: None,
        }
    }
}

/// How an analysis task was executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    /// Executed in place as a Jupyter notebook (`.ipynb` carries outputs).
    Jupyter,
    /// The `.py` script ran directly (notebook saved without outputs).
    Python,
    /// No execution: script + notebook generated for manual running.
    DryRun,
}

/// Outcome of running one analysis task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub task_id: String,
    pub success: bool,
    pub dry_run: bool,
    pub script_path: PathBuf,
    /// Always produced: the Jupyter notebook (executed-with-outputs when
    /// `execution_backend == Jupyter`, code-only otherwise).
    pub notebook_path: PathBuf,
    pub notebook_executed: bool,
    pub execution_backend: ExecutionBackend,
    pub output_files: Vec<PathBuf>,
    pub provenance_path: Option<PathBuf>,
    pub provenance: ProvenanceRecord,
    pub error: Option<String>,
}

pub struct AnalysisRunner {
    provider: Box<dyn LlmProvider>,
}

impl AnalysisRunner {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }

    /// Execute a data-analysis task end-to-end.
    pub async fn run(
        &self,
        task: &DataAnalysisTask,
        working_dir: &Path,
        hypothesis_ref: Option<uuid::Uuid>,
        opts: &RunOpts,
        cancel: CancellationToken,
    ) -> Result<AnalysisResult, AgentError> {
        let task_dir = working_dir.join("analysis").join(safe_id(&task.id));
        std::fs::create_dir_all(&task_dir)
            .map_err(|e| AgentError::Checkpoint(format!("create task dir: {e}")))?;

        let script_path = task_dir.join("script.py");
        let notebook_path = task_dir.join("analysis.ipynb");
        let started_at = Utc::now();
        let instant = std::time::Instant::now();

        // Resolve local data path.
        let local_data_path: Option<PathBuf> = opts
            .local_data
            .clone()
            .or_else(|| match &task.dataset_source {
                DatasetSource::Local(p) => Some(working_dir.join(p)),
                _ => None,
            });
        let dry_run = local_data_path.is_none() && !matches!(task.dataset_source, DatasetSource::Local(_));

        // Generate the reproducible analysis script.
        let data_preview = read_data_preview(local_data_path.as_deref());
        let script = self
            .generate_script(task, &script_path, &local_data_path, &task_dir, opts, &data_preview, &cancel)
            .await?;
        std::fs::write(&script_path, &script)
            .map_err(|e| AgentError::Checkpoint(format!("write script: {e}")))?;
        let script_hash = sha256_hex(script.as_bytes());

        // Always build a Jupyter notebook from the generated script + task
        // metadata, so the analysis is viewable/re-runnable as a .ipynb.
        let notebook = crate::notebook_gen::build_notebook(task, hypothesis_ref, &script);
        crate::notebook_gen::write_notebook(&notebook, &notebook_path)?;

        // In dry-run mode we stop here: deliver script + notebook + plan, no execution.
        if dry_run {
            let no_conda: Option<String> = None;
            let provenance = self.finalize_provenance(
                task,
                hypothesis_ref,
                &script_path,
                script_hash,
                &local_data_path,
                &task_dir,
                opts,
                started_at,
                instant.elapsed(),
                None,
                "",
                "",
                &no_conda,
                false,
                Some(&notebook_path),
                false,
                ExecutionBackend::DryRun,
            );
            let provenance_path = self.persist_provenance(&task_dir, &provenance)?;
            return Ok(AnalysisResult {
                task_id: task.id.clone(),
                success: true,
                dry_run: true,
                script_path,
                notebook_path,
                notebook_executed: false,
                execution_backend: ExecutionBackend::DryRun,
                output_files: vec![],
                provenance_path: Some(provenance_path),
                provenance,
                error: Some(format!(
                    "dry-run: no local data available (source={:?}); script + notebook generated for manual execution",
                    task.dataset_source
                )),
            });
        }

        // Ensure conda env (best-effort) — needed for the python fallback and
        // harmless for the jupyter path.
        let conda_bin = detect_conda();
        let conda_used = if let Some(ref bin) = conda_bin {
            ensure_env(bin, &opts.conda_env, &opts.extra_packages).await
        } else {
            tracing::warn!("no conda/mamba/micromamba found; falling back to system python");
            false
        };

        // Execute. Prefer running the notebook in place via Jupyter (so the
        // saved .ipynb carries outputs); fall back to running the .py script
        // directly when Jupyter is missing or the notebook execution fails.
        let exec = self
            .execute_analysis(&conda_bin, &opts.conda_env, &script_path, &notebook_path, working_dir, &cancel)
            .await;
        let (stdout, stderr, exit_code, notebook_executed, backend) = match exec {
            Ok(o) => (o.stdout, o.stderr, o.exit_code, o.notebook_executed, o.backend),
            Err(e) => {
                let stderr = e.to_string();
                let provenance = self.finalize_provenance(
                    task, hypothesis_ref, &script_path, script_hash, &local_data_path,
                    &task_dir, opts, started_at, instant.elapsed(), None, "", &stderr,
                    &conda_bin, conda_used, Some(&notebook_path), false, ExecutionBackend::Python,
                );
                let provenance_path = self.persist_provenance(&task_dir, &provenance)?;
                return Ok(AnalysisResult {
                    task_id: task.id.clone(),
                    success: false,
                    dry_run: false,
                    script_path,
                    notebook_path,
                    notebook_executed: false,
                    execution_backend: ExecutionBackend::Python,
                    output_files: vec![],
                    provenance_path: Some(provenance_path),
                    provenance,
                    error: Some(format!("analysis execution failed: {e}")),
                });
            }
        };

        // Capture outputs and provenance.
        let output_files = collect_outputs(&task_dir, &script_path, &notebook_path);
        let provenance = self.finalize_provenance(
            task, hypothesis_ref, &script_path, script_hash, &local_data_path, &task_dir,
            opts, started_at, instant.elapsed(), exit_code, &stdout, &stderr, &conda_bin,
            conda_used, Some(&notebook_path), notebook_executed, backend,
        );
        let provenance_path = self.persist_provenance(&task_dir, &provenance)?;

        Ok(AnalysisResult {
            task_id: task.id.clone(),
            success: exit_code == Some(0),
            dry_run: false,
            script_path,
            notebook_path,
            notebook_executed,
            execution_backend: backend,
            output_files,
            provenance_path: Some(provenance_path),
            provenance,
            error: if exit_code == Some(0) {
                None
            } else {
                Some(format!("analysis exited with code {exit_code:?}"))
            },
        })
    }

    /// Execute the analysis, preferring an in-place Jupyter run and falling
    /// back to the `.py` script. Returns the captured stdout/stderr, exit
    /// code, whether the notebook itself was executed, and the backend used.
    async fn execute_analysis(
        &self,
        conda_bin: &Option<String>,
        env: &str,
        script: &Path,
        notebook: &Path,
        working_dir: &Path,
        cancel: &CancellationToken,
    ) -> Result<ExecOutcome, AgentError> {
        // Try Jupyter first. Overall wall-clock guard: the per-cell
        // nbconvert timeout handles hanging cells, but nbconvert itself can
        // stall (kernel startup, lock files) — 15 min is the hard ceiling.
        if crate::notebook::jupyter_available() {
            let nb_path = notebook.to_path_buf();
            let nb_exec = tokio::task::spawn_blocking(move || {
                crate::notebook::execute_notebook(&nb_path, Some(&nb_path), 600)
            });
            match tokio::time::timeout(std::time::Duration::from_secs(900), nb_exec).await {
                Ok(Ok(Ok(nb))) if nb.exit_code == 0 => {
                    return Ok(ExecOutcome {
                        stdout: nb.stdout,
                        stderr: nb.stderr,
                        exit_code: Some(nb.exit_code),
                        notebook_executed: true,
                        backend: ExecutionBackend::Jupyter,
                    });
                }
                Ok(Ok(Ok(nb))) => {
                    // Notebook execution failed — fall back to the script so we
                    // still capture a concrete error / partial outputs.
                    tracing::warn!(
                        exit_code = nb.exit_code,
                        "notebook execution failed (exit {}); falling back to script",
                        nb.exit_code
                    );
                }
                Ok(Ok(Err(e))) => {
                    tracing::warn!("jupyter unavailable ({e}); falling back to script execution");
                }
                Ok(Err(e)) => {
                    tracing::warn!("nbconvert task panicked: {e}; falling back to script");
                }
                Err(_) => {
                    tracing::warn!("nbconvert exceeded 15 min wall clock; falling back to script");
                }
            }
        }

        // Fallback: run the .py script directly.
        let o = run_script(conda_bin.as_deref(), env, script, working_dir, cancel).await?;
        Ok(ExecOutcome {
            stdout: o.stdout,
            stderr: o.stderr,
            exit_code: o.exit_code,
            notebook_executed: false,
            backend: ExecutionBackend::Python,
        })
    }

    async fn generate_script(
        &self,
        task: &DataAnalysisTask,
        _script_path: &Path,
        local_data: &Option<PathBuf>,
        output_dir: &Path,
        opts: &RunOpts,
        data_preview: &str,
        cancel: &CancellationToken,
    ) -> Result<String, AgentError> {
        let input_block = match local_data {
            Some(p) => format!(
                "INPUT_DATA_PATH = {}  # relative to working dir\nDATA PREVIEW (first lines):\n{}",
                python_str(&p.to_string_lossy()),
                data_preview
            ),
            None => format!(
                "No local data file. If a dataset needs downloading, include a comment with the \
                 source accession ({:?}) and a TODO so the researcher can fetch it. Do NOT invent \
                 a file path.",
                task.dataset_source
            ),
        };

        let vars_block = format!(
            "independent: {:?}\ndependent: {:?}\ncovariates: {:?}",
            task.variables.independent, task.variables.dependent, task.variables.covariates
        );

        let prompt = format!(
            r#"You are a bioinformatics engineer. Write ONE self-contained, reproducible Python script.

**Analysis objective:** {objective}
**Statistical method:** {method}
**Cohort / comparison:** {cohort}
**Variables:**
{vars}
**Expected outcome (if hypothesis holds):** {expected}
**Deliverable:** {deliverable}

{input}

OUTPUT_DIR = {output_dir}  # write all outputs here (relative to working dir)
SEED = {seed}

Requirements:
1. Set seeds at the top: `import numpy as np, random; np.random.seed({seed}); random.seed({seed})`.
2. Read the input data with pandas. Use robust parsing (delimiters, missing values).
3. Adapt the stated statistical method to the ACTUAL columns shown in the data
   preview. If the planned method is impossible with the available columns
   (e.g. it needs gene-expression rows but the file is a per-sample biomarker
   table), implement the closest valid equivalent on the real columns instead
   of failing. Never reference columns that are not in the preview. Prefer
   scipy / statsmodels / scikit-learn.
4. Write all deliverables (tables as CSV, figures as PNG) into OUTPUT_DIR.
5. Print a final JSON line `RESULT = {{...}}` summarizing key numbers (effect sizes, p-values, CIs).
6. Include brief comments. No external services, no GUI. No `plt.show()`.
7. If the input data does not match expectations, raise a clear `ValueError` with a message.

Output ONLY the Python code, no markdown fences, no explanation."#,
            objective = task.objective,
            method = task.statistical_method,
            cohort = task.cohort_definition,
            vars = vars_block,
            expected = task.expected_outcome,
            deliverable = task.deliverable,
            input = input_block,
            output_dir = python_str(&output_dir.to_string_lossy()),
            seed = opts.seed,
        );

        let request = CompletionRequest {
            system: "You are a precise code generator. Output ONLY raw Python code."
                .into(),
            messages: vec![miniagent_core::message::Message::user(&prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.1),
                max_tokens: Some(4000),
                ..Default::default()
            },
        };

        let resp = self.provider.complete(&request, cancel.clone()).await?;
        let text = resp
            .content
            .iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        // Reasoning models can exhaust the token budget on chain-of-thought
        // and return empty code — observed live: the empty script built a
        // notebook that "succeeded" with zero output. One retry with a
        // doubled budget recovers it (same pattern as validation plans).
        let script = json_util::strip_markdown_fences(&json_util::strip_reasoning_tags(&text))
            .trim()
            .to_string();
        let text = if script.is_empty() {
            tracing::warn!("script generation empty ({}), retrying with larger budget", task.id);
            let retry = CompletionRequest {
                system: request.system.clone(),
                messages: request.messages.clone(),
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.1),
                    max_tokens: Some(16_384),
                    ..Default::default()
                },
            };
            let resp = self.provider.complete(&retry, cancel.clone()).await?;
            resp.content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        } else {
            text
        };

        // Strip reasoning tags + markdown fences if the model added them
        // despite instructions (reasoning models emit <think> inline).
        Ok(json_util::strip_markdown_fences(&json_util::strip_reasoning_tags(&text)).trim().to_string())
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_provenance(
        &self,
        task: &DataAnalysisTask,
        hypothesis_ref: Option<uuid::Uuid>,
        script_path: &Path,
        script_hash: String,
        local_data: &Option<PathBuf>,
        task_dir: &Path,
        opts: &RunOpts,
        started_at: chrono::DateTime<Utc>,
        elapsed: std::time::Duration,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        conda_bin: &Option<String>,
        conda_used: bool,
        notebook_path: Option<&Path>,
        notebook_executed: bool,
        backend: ExecutionBackend,
    ) -> ProvenanceRecord {
        let inputs: Vec<FileRecord> = local_data
            .iter()
            .filter_map(|p| record_file(p))
            .collect();
        let outputs = record_dir_bounded(task_dir, 6)
            .into_iter()
            .filter(|r| r.path != script_path && Some(r.path.as_path()) != notebook_path)
            .collect();

        ProvenanceRecord {
            task_id: task.id.clone(),
            hypothesis_ref,
            script_path: script_path.to_path_buf(),
            script_hash,
            inputs,
            outputs,
            params: serde_json::json!({
                "statistical_method": task.statistical_method,
                "cohort_definition": task.cohort_definition,
                "variables": {
                    "independent": task.variables.independent,
                    "dependent": task.variables.dependent,
                    "covariates": task.variables.covariates,
                },
                "seed": opts.seed,
            }),
            conda_env: opts.conda_env.clone(),
            conda_used,
            package_versions: capture_package_versions(conda_bin.as_deref(), &opts.conda_env, conda_used),
            seed: opts.seed,
            git_commit: current_git_commit(task_dir),
            started_at,
            duration: chrono::Duration::from_std(elapsed).unwrap_or_default(),
            exit_code,
            stdout_hash: sha256_hex(stdout.as_bytes()),
            stderr_hash: sha256_hex(stderr.as_bytes()),
            stdout_preview: preview(stdout.as_bytes(), 2000),
            stderr_preview: preview(stderr.as_bytes(), 2000),
            notebook_path: notebook_path.map(|p| p.to_path_buf()),
            notebook_executed,
            execution_backend: match backend {
                ExecutionBackend::Jupyter => "jupyter",
                ExecutionBackend::Python => "python",
                ExecutionBackend::DryRun => "dry_run",
            }
            .to_string(),
        }
    }

    fn persist_provenance(
        &self,
        task_dir: &Path,
        record: &ProvenanceRecord,
    ) -> Result<PathBuf, AgentError> {
        let path = task_dir.join("provenance.json");
        let json = record
            .to_json_pretty()
            .map_err(|e| AgentError::Checkpoint(format!("provenance serialize: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| AgentError::Checkpoint(format!("write provenance: {e}")))?;
        Ok(path)
    }
}

struct ExecOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
}

/// Snapshot the interpreter's package versions for provenance. Runs
/// `<python> -m pip freeze` in the env that actually executed the script
/// (conda env when used, else system python). Best-effort: empty on any
/// failure (no pip / offline), never blocks the analysis.
fn capture_package_versions(conda_bin: Option<&str>, env: &str, conda_used: bool) -> Vec<String> {
    let mut cmd = if conda_used {
        let Some(bin) = conda_bin else {
            return vec![];
        };
        let mut c = std::process::Command::new(bin);
        c.args(["run", "-n", env, "python", "-m", "pip", "freeze"]);
        c
    } else {
        let mut c = std::process::Command::new("python3");
        c.args(["-m", "pip", "freeze"]);
        c
    };
    let Ok(out) = cmd.output() else {
        return vec![];
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(200)
        .map(str::to_string)
        .collect()
}

/// Resolved execution outcome from `execute_analysis`.
struct ExecOutcome {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    notebook_executed: bool,
    backend: ExecutionBackend,
}

async fn run_script(
    conda_bin: Option<&str>,
    env: &str,
    script: &Path,
    working_dir: &Path,
    cancel: &CancellationToken,
) -> Result<ExecOutput, AgentError> {
    let mut cmd = match conda_bin {
        Some(bin) => {
            let mut c = Command::new(bin);
            c.args(["run", "-n", env, "python", &script.to_string_lossy()]);
            c
        }
        None => {
            let mut c = Command::new("python3");
            c.arg(script);
            c
        }
    };
    cmd.current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // spawn() is synchronous; only the wait needs to be cancellable.
    let child = cmd
        .spawn()
        .map_err(|e| AgentError::tool("analysis.runner", format!("spawn: {e}")))?;
    let output = tokio::select! {
        _ = cancel.cancelled() => return Err(AgentError::Cancelled),
        r = child.wait_with_output() => {
            r.map_err(|e| AgentError::tool("analysis.runner", format!("wait: {e}")))?
        }
    };

    Ok(ExecOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code(),
    })
}

/// Create the conda env if it does not yet exist (best-effort).
async fn ensure_env(conda_bin: &str, env: &str, packages: &[String]) -> bool {
    // Check existence first.
    let exists = Command::new(conda_bin)
        .args(["env", "list"])
        .output()
        .await
        .map(|o| {
            let txt = String::from_utf8_lossy(&o.stdout);
            txt.lines().any(|l| l.split_whitespace().next() == Some(env))
        })
        .unwrap_or(false);
    if exists {
        return true;
    }

    let mut args = vec!["create".to_string(), "-y".to_string(), "-n".to_string(), env.to_string()];
    for p in packages {
        args.push(p.clone());
    }
    let status = Command::new(conda_bin)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    matches!(status, Ok(s) if s.success())
}

/// Detect the first available conda-family binary on PATH.
fn detect_conda() -> Option<String> {
    for bin in &["micromamba", "mamba", "conda"] {
        if std::process::Command::new(bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return Some((*bin).to_string());
        }
    }
    None
}

fn safe_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn python_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(r#""{escaped}""#)
}

fn read_data_preview(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().take(20).collect();
            if lines.is_empty() {
                "(empty file)".to_string()
            } else {
                lines.join("\n")
            }
        }
        Err(e) => format!("(could not read data file for preview: {e})"),
    }
}

fn collect_outputs(task_dir: &Path, script: &Path, notebook: &Path) -> Vec<PathBuf> {
    // Bounded recursive walk: generated scripts legitimately create
    // subdirectories (figures/, tables/), and a script that mis-handles the
    // OUTPUT_DIR hint can scatter files several levels deep — either way the
    // deliverables must still be tracked for provenance.
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, depth + 1, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(task_dir, 0, &mut files);
    files
        .into_iter()
        .filter(|p| p != script && p != notebook)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn safe_id_sanitizes() {
        assert_eq!(safe_id("DA-1"), "DA-1");
        assert_eq!(safe_id("DA 1/2"), "DA_1_2");
    }

    #[test]
    fn python_str_escapes() {
        assert_eq!(python_str(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn read_data_preview_returns_head() {
        let dir = std::env::temp_dir().join("miniagent_runner_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.csv");
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 0..30 {
            writeln!(f, "row{i}").unwrap();
        }
        let p = preview_of(&path);
        assert_eq!(p.lines().count(), 20);
        std::fs::remove_file(path).ok();
    }

    fn preview_of(path: &Path) -> String {
        read_data_preview(Some(path))
    }

    #[test]
    fn read_data_preview_missing_file_is_safe() {
        let s = read_data_preview(Some(Path::new("/nonexistent/x.csv")));
        assert!(s.contains("could not read"));
    }

    /// A stub provider that returns a canned script, so the runner can be
    /// exercised offline (no real LLM). Validates the dry-run path + provenance.
    #[tokio::test]
    async fn dry_run_generates_script_and_provenance() {
        use miniagent_provider::traits::{CompletionResponse, StreamResponse};
        struct Stub;
        #[async_trait::async_trait]
        impl LlmProvider for Stub {
            async fn complete(
                &self,
                _req: &CompletionRequest,
                _cancel: CancellationToken,
            ) -> Result<CompletionResponse, AgentError> {
                use miniagent_core::event::{ContentBlock, StopReason};
                Ok(CompletionResponse {
                    content: vec![ContentBlock::Text {
                        text: "print('hello analysis')".into(),
                    }],
                    stop_reason: StopReason::EndTurn,
                    usage: Default::default(),
                })
            }
            async fn stream(
                &self,
                _req: &CompletionRequest,
                _cancel: CancellationToken,
            ) -> Result<StreamResponse, AgentError> {
                Err(AgentError::internal("stub does not support stream"))
            }
        }

        let dir = std::env::temp_dir().join("miniagent_runner_dryrun");
        std::fs::create_dir_all(&dir).unwrap();
        let runner = AnalysisRunner::new(Box::new(Stub));
        let task = DataAnalysisTask {
            id: "DA-1".into(),
            objective: "test".into(),
            dataset_source: DatasetSource::Geo,
            dataset_accession: Some("GSE1".into()),
            cohort_definition: "x".into(),
            variables: Default::default(),
            statistical_method: "t-test".into(),
            expected_outcome: "e".into(),
            deliverable: "d".into(),
            priority: 0.5,
        };
        let res = runner
            .run(&task, &dir, None, &RunOpts::default(), CancellationToken::new())
            .await
            .unwrap();
        assert!(res.dry_run);
        assert!(res.success);
        assert!(res.script_path.exists());
        assert!(res.provenance_path.is_some());
        let prov = std::fs::read_to_string(res.provenance_path.unwrap()).unwrap();
        assert!(prov.contains("\"task_id\": \"DA-1\""));
        assert!(prov.contains("\"conda_used\": false"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
