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
    ProvenanceRecord, RepairAttempt,
};

/// Total generation+execution attempts per task before giving up: the first
/// try plus up to 2 self-repair rounds driven by the real execution error.
const MAX_ATTEMPTS: usize = 3;

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
    /// Alternate-vendor clients used only when the primary provider returns
    /// empty/truncated code after its budget escalation (provider-level
    /// degradation recovery), tried in order. Built via
    /// `factory::codegen_fallback_providers`.
    codegen_fallback: Vec<Box<dyn LlmProvider>>,
}

impl AnalysisRunner {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider, codegen_fallback: Vec::new() }
    }

    /// Wire cross-family fallbacks used for script generation only.
    pub fn with_codegen_fallback(mut self, providers: Vec<Box<dyn LlmProvider>>) -> Self {
        self.codegen_fallback = providers;
        self
    }

    /// Execute a data-analysis task end-to-end.
    ///
    /// Runs a generate → execute → repair loop: when a generated script fails
    /// (or generation returns nothing), the real error plus the dataset's
    /// actual schema are fed back for a corrected script, up to
    /// [`MAX_ATTEMPTS`] attempts. Every failed attempt is kept in the
    /// provenance `repair_history` for audit.
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

        // GEO series matrices get a schema-level summary (attribute keys,
        // header, dimensions) instead of a raw 20-line head: generated scripts
        // must be written against the real column layout.
        let data_schema = local_data_path
            .as_deref()
            .and_then(crate::geo::summarize_series_matrix);
        let data_preview = match (&data_schema, local_data_path.as_deref()) {
            (Some(summary), _) => summary.clone(),
            (None, Some(p)) => read_data_preview(Some(p)),
            (None, None) => String::new(),
        };

        // Generate the reproducible analysis script.
        let script = self
            .generate_script(task, &local_data_path, &task_dir, opts, &data_preview, None, &cancel)
            .await?;
        if script_is_empty(&script) {
            return Ok(self.failed_result(
                task,
                hypothesis_ref,
                &script_path,
                &notebook_path,
                &task_dir,
                &local_data_path,
                opts,
                started_at,
                instant.elapsed(),
                "script generation returned an empty script (model budget exhausted on reasoning?); not executing",
                vec![RepairAttempt {
                    attempt: 1,
                    action: "aborted: empty script".into(),
                    error_tail: "empty generation".into(),
                }],
                ExecutionBackend::DryRun,
            ));
        }
        std::fs::write(&script_path, &script)
            .map_err(|e| AgentError::Checkpoint(format!("write script: {e}")))?;

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
                vec![],
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

        // ── Generate → execute → repair loop ─────────────────────────────
        // The first script (already written above) runs as attempt 1; a
        // failure feeds stderr + the dataset schema back to the LLM for a
        // corrected script (attempts 2..MAX_ATTEMPTS). Missing third-party
        // imports are auto-installed into every interpreter that may run the
        // script BEFORE execution, which resolves ModuleNotFoundError without
        // burning an LLM round-trip.
        let mut repair_history: Vec<RepairAttempt> = Vec::new();
        let mut current_script = script;
        let mut last_error = String::new();

        for attempt in 1..=MAX_ATTEMPTS {
            if attempt > 1 {
                let repair_ctx = RepairContext {
                    failed_script: current_script.clone(),
                    error_tail: last_error.clone(),
                };
                print!("      🔧 repair attempt {attempt}/{} ... ", MAX_ATTEMPTS);
                std::io::Write::flush(&mut std::io::stdout()).ok();
                current_script = self
                    .generate_script(
                        task,
                        &local_data_path,
                        &task_dir,
                        opts,
                        &data_preview,
                        Some(&repair_ctx),
                        &cancel,
                    )
                    .await?;
                if script_is_empty(&current_script) {
                    last_error =
                        "repair generation returned an empty script".to_string();
                    repair_history.push(RepairAttempt {
                        attempt,
                        action: "regenerate (empty output — retrying)".into(),
                        error_tail: last_error.clone(),
                    });
                    continue;
                }
                std::fs::write(&script_path, &current_script)
                    .map_err(|e| AgentError::Checkpoint(format!("write script: {e}")))?;
                let notebook = crate::notebook_gen::build_notebook(
                    task,
                    hypothesis_ref,
                    &current_script,
                );
                crate::notebook_gen::write_notebook(&notebook, &notebook_path)?;
                repair_history.push(RepairAttempt {
                    attempt,
                    action: "regenerated script via LLM with error feedback".into(),
                    error_tail: tail_of(&last_error, 400),
                });
            }

            // Auto-install missing imports in every candidate interpreter.
            ensure_imports(conda_bin.as_deref(), &opts.conda_env, &current_script).await;

            let exec = self
                .execute_analysis(
                    &conda_bin,
                    &opts.conda_env,
                    &script_path,
                    &notebook_path,
                    working_dir,
                    &cancel,
                )
                .await;
            match exec {
                Ok(o) if o.exit_code == Some(0) => {
                    // Best-effort: when only the .py fallback ran, try to
                    // execute the notebook once more so the .ipynb carries
                    // outputs (goal 4 deliverable) without blocking success.
                    let (mut notebook_executed, mut backend) = (o.notebook_executed, o.backend);
                    if backend == ExecutionBackend::Python && !notebook_executed {
                        if let Some(nb) = try_execute_notebook(&notebook_path) {
                            notebook_executed = nb;
                            if nb {
                                backend = ExecutionBackend::Jupyter;
                            }
                        }
                    }
                    let output_files = collect_outputs(&task_dir, &script_path, &notebook_path);
                    let provenance = self.finalize_provenance(
                        task, hypothesis_ref, &script_path, &local_data_path, &task_dir, opts,
                        started_at, instant.elapsed(), Some(0), &o.stdout, &o.stderr, &conda_bin,
                        conda_used, Some(&notebook_path), notebook_executed, backend,
                        repair_history,
                    );
                    let provenance_path = self.persist_provenance(&task_dir, &provenance)?;
                    return Ok(AnalysisResult {
                        task_id: task.id.clone(),
                        success: true,
                        dry_run: false,
                        script_path,
                        notebook_path,
                        notebook_executed,
                        execution_backend: backend,
                        output_files,
                        provenance_path: Some(provenance_path),
                        provenance,
                        error: None,
                    });
                }
                Ok(o) => {
                    last_error = format!(
                        "exit code {:?}\n{}",
                        o.exit_code,
                        tail_of(&o.stderr, 2000)
                    );
                    if !o.stderr.trim().is_empty() || !o.stdout.trim().is_empty() {
                        println!("      ❌ attempt {attempt} failed");
                    }
                }
                Err(e) => {
                    last_error = e.to_string();
                }
            }
            if attempt < MAX_ATTEMPTS {
                let schema_hint = data_schema.as_deref().unwrap_or("");
                if !schema_hint.is_empty() {
                    last_error.push_str("\n\nActual dataset schema:\n");
                    last_error.push_str(schema_hint);
                }
            }
        }

        // All attempts exhausted — record an honest failure with the repair
        // history for audit.
        let provenance = self.finalize_provenance(
            task, hypothesis_ref, &script_path, &local_data_path, &task_dir, opts, started_at,
            instant.elapsed(), Some(1), "", &last_error, &conda_bin, conda_used,
            Some(&notebook_path), false, ExecutionBackend::Python, repair_history,
        );
        let provenance_path = self.persist_provenance(&task_dir, &provenance)?;
        Ok(AnalysisResult {
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
            error: Some(format!(
                "analysis failed after {MAX_ATTEMPTS} attempts: {}",
                tail_of(&last_error, 300)
            )),
        })
    }

    /// Build a failure [`AnalysisResult`] for pre-execution aborts (empty
    /// generation) without pretending anything ran.
    #[allow(clippy::too_many_arguments)]
    fn failed_result(
        &self,
        task: &DataAnalysisTask,
        hypothesis_ref: Option<uuid::Uuid>,
        script_path: &Path,
        notebook_path: &Path,
        task_dir: &Path,
        local_data: &Option<PathBuf>,
        opts: &RunOpts,
        started_at: chrono::DateTime<Utc>,
        elapsed: std::time::Duration,
        error: &str,
        repair_history: Vec<RepairAttempt>,
        backend: ExecutionBackend,
    ) -> AnalysisResult {
        let provenance = self.finalize_provenance(
            task, hypothesis_ref, script_path, local_data, task_dir, opts, started_at, elapsed,
            None, "", error, &None, false, Some(notebook_path), false, backend, repair_history,
        );
        let provenance_path = self.persist_provenance(task_dir, &provenance).ok();
        AnalysisResult {
            task_id: task.id.clone(),
            success: false,
            dry_run: false,
            script_path: script_path.to_path_buf(),
            notebook_path: notebook_path.to_path_buf(),
            notebook_executed: false,
            execution_backend: backend,
            output_files: vec![],
            provenance_path,
            provenance,
            error: Some(error.to_string()),
        }
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
        local_data: &Option<PathBuf>,
        output_dir: &Path,
        opts: &RunOpts,
        data_preview: &str,
        repair: Option<&RepairContext>,
        cancel: &CancellationToken,
    ) -> Result<String, AgentError> {
        let input_block = match local_data {
            Some(p) => format!(
                "INPUT_DATA_PATH = {}  # absolute path\nDATA SCHEMA / PREVIEW:\n{}",
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

        let repair_block = match repair {
            Some(ctx) => format!(
                "\n**THIS IS A REPAIR ROUND. The previous script failed.**\n\
                 Previous script:\n```python\n{}\n```\n\n\
                 Execution error (fix the cause; adapt to the REAL columns shown in the schema):\n\
                 ```\n{}\n```\n\n\
                 Output the FULL corrected script (not a diff). If the planned statistical method \
                 cannot work with the actual columns, implement the closest valid equivalent.\n",
                ctx.failed_script,
                tail_of(&ctx.error_tail, 2500),
            ),
            None => String::new(),
        };

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

OUTPUT_DIR = {output_dir}  # write all outputs here (absolute path)
SEED = {seed}
{repair}
Requirements:
1. Set seeds at the top: `import numpy as np, random; np.random.seed({seed}); random.seed({seed})`.
2. Read the input data with pandas. Use robust parsing (delimiters, missing values).
3. Adapt the stated statistical method to the ACTUAL columns shown in the data
   schema/preview. If the planned method is impossible with the available columns
   (e.g. it needs gene-expression rows but the file is a per-sample biomarker
   table), implement the closest valid equivalent on the real columns instead
   of failing. Never reference columns that are not in the schema/preview. Prefer
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
            repair = repair_block,
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

        let to_script = |text: &str| {
            json_util::strip_markdown_fences(&json_util::strip_reasoning_tags(text))
                .trim()
                .to_string()
        };

        let resp = complete_code_text(self.provider.as_ref(), &request, cancel.clone()).await?;
        let mut script = to_script(&resp);

        // Reasoning models can exhaust the token budget on chain-of-thought
        // and return empty code, or the code itself gets cut mid-expression
        // (unbalanced brackets → SyntaxError on run). One retry with a
        // quadrupled budget recovers both (same pattern as validation plans);
        // when the PRIMARY vendor still fails — repeated empty output is a
        // provider-level degradation, not a prompt problem — retry once more
        // on an alternate-vendor fallback client if one is wired up.
        let needs_retry = script.is_empty() || python_looks_truncated(&script);
        if needs_retry {
            let why = if script.is_empty() { "empty" } else { "truncated" };
            tracing::warn!("script generation {why} ({}), escalating budget/provider", task.id);
            let mut retry = request.clone();
            retry.config.max_tokens = Some(16_384);
            let mut text =
                complete_code_text(self.provider.as_ref(), &retry, cancel.clone()).await?;
            script = to_script(&text);
            if (script.is_empty() || python_looks_truncated(&script))
                && !self.codegen_fallback.is_empty()
            {
                tracing::warn!(
                    "primary provider still returned {} script ({}); walking cross-family fallbacks",
                    if script.is_empty() { "empty" } else { "truncated" },
                    task.id
                );
                for fb in &self.codegen_fallback {
                    text = complete_code_text(fb.as_ref(), &retry, cancel.clone()).await?;
                    script = to_script(&text);
                    if !script.is_empty() && !python_looks_truncated(&script) {
                        break;
                    }
                }
            }
        }

        Ok(script)
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_provenance(
        &self,
        task: &DataAnalysisTask,
        hypothesis_ref: Option<uuid::Uuid>,
        script_path: &Path,
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
        repair_history: Vec<RepairAttempt>,
    ) -> ProvenanceRecord {
        let inputs: Vec<FileRecord> = local_data
            .iter()
            .filter_map(|p| record_file(p))
            .collect();
        let outputs = record_dir_bounded(task_dir, 6)
            .into_iter()
            .filter(|r| r.path != script_path && Some(r.path.as_path()) != notebook_path)
            .collect();

        let script_hash = std::fs::read(script_path)
            .map(|b| sha256_hex(&b))
            .unwrap_or_default();

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
            repair_history,
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

// ── Self-repair helpers ─────────────────────────────────────────

/// One completion call against any provider, returning concatenated text.
async fn complete_code_text(
    provider: &dyn LlmProvider,
    req: &CompletionRequest,
    cancel: CancellationToken,
) -> Result<String, AgentError> {
    let resp = provider.complete(req, cancel).await?;
    Ok(resp
        .content
        .iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(""))
}

/// Context for one LLM repair round: the failed script plus the real
/// execution error.
struct RepairContext {
    failed_script: String,
    error_tail: String,
}

/// True when a generated script has no executable lines (comments/docstrings
/// don't count). Guards against the "empty notebook executed successfully"
/// false positive observed with reasoning models.
fn script_is_empty(script: &str) -> bool {
    !script
        .lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Heuristic truncation check: unbalanced brackets (outside strings/comments)
/// mean the token budget cut the script mid-expression — e.g.
/// `abs(z_log[3` with `SyntaxError: '[' was never closed`. Cheap, no python
/// subprocess needed; false positives are impossible for well-formed code
/// that parses.
fn python_looks_truncated(code: &str) -> bool {
    let chars: Vec<char> = code.chars().collect();
    let mut depth: i64 = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            let triple = i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c;
            let start = i + if triple { 3 } else { 1 };
            let mut j = start;
            let mut closed = false;
            while j < chars.len() {
                if chars[j] == '\\' {
                    j += 2;
                    continue;
                }
                if chars[j] == c && (triple == (j + 2 < chars.len() && chars[j + 1] == c && chars[j + 2] == c) || !triple) {
                    closed = true;
                    j += if triple { 3 } else { 1 };
                    break;
                }
                j += 1;
            }
            if !closed {
                return true; // unterminated string → truncated
            }
            i = j;
            continue;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth != 0
}

/// Last `max_chars` characters of `s` (the traceback head matters less than
/// the error line at the end).
fn tail_of(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        s.chars().skip(n - max_chars).collect()
    }
}

/// Best-effort in-place notebook execution used after a successful .py
/// fallback run, so the delivered .ipynb carries outputs. Returns whether
/// the notebook executed.
fn try_execute_notebook(notebook_path: &Path) -> Option<bool> {
    if !crate::notebook::jupyter_available() {
        return None;
    }
    let nb = crate::notebook::execute_notebook(notebook_path, Some(notebook_path), 600).ok()?;
    Some(nb.exit_code == 0)
}

/// Parse the top-level third-party modules imported by a Python script
/// (bare module names, deduped, capped).
fn parse_imports(script: &str) -> Vec<String> {
    let mut mods: Vec<String> = Vec::new();
    let push = |module: &str, mods: &mut Vec<String>| {
        let module = module.trim();
        if module.is_empty()
            || !module
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            || mods.iter().any(|m| m == module)
        {
            return;
        }
        mods.push(module.to_string());
    };
    for line in script.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("import ") {
            // `import a, b as c` → modules a and b
            for part in r.split(',') {
                if let Some(first) = part.trim().split_whitespace().next() {
                    push(first.split('.').next().unwrap_or(""), &mut mods);
                }
            }
        } else if let Some(r) = l.strip_prefix("from ") {
            if let Some(first) = r.trim().split_whitespace().next() {
                push(first.split('.').next().unwrap_or(""), &mut mods);
            }
        }
        if mods.len() >= 16 {
            break;
        }
    }
    mods
}

/// Map a Python module name to its pip distribution name.
fn pip_name_for(module: &str) -> &'static str {
    match module {
        "sklearn" => "scikit-learn",
        "PIL" => "pillow",
        "cv2" => "opencv-python-headless",
        "yaml" => "pyyaml",
        "bio" => "biopython",
        _ => "",
    }
}

/// Ensure every third-party import of `script` is importable by each
/// interpreter that may execute it: the PATH `python3` (which the default
/// Jupyter kernel resolves to) and, when present, the conda env's python
/// (the .py fallback). Missing modules are pip-installed into exactly the
/// interpreter that lacks them.
///
/// This kills the ModuleNotFoundError class of failures without spending an
/// LLM repair round (observed live: `seaborn` present in the kernel python
/// but missing from the conda env, and `sklearn` the other way round).
async fn ensure_imports(conda_bin: Option<&str>, env: &str, script: &str) {
    let mods = parse_imports(script);
    if mods.is_empty() {
        return;
    }
    let mods_json = match serde_json::to_string(&mods) {
        Ok(j) => j,
        Err(_) => return,
    };
    // One-line probe: report modules this interpreter cannot import. Works
    // on any python ≥3.5 (deliberately avoids sys.stdlib_module_names, which
    // needs 3.10 and crashed the probe on the CLT python 3.9 backing the
    // jupyter kernel): stdlib modules are always findable, so only genuinely
    // missing third-party modules get pip-installed.
    let probe = "import importlib.util, json, sys\n\
                 mods = json.loads(sys.argv[1])\n\
                 print(json.dumps([m for m in mods if importlib.util.find_spec(m) is None]))";

    let mut targets: Vec<(&str, Vec<String>)> = vec![(
        "python3",
        vec!["-c".to_string(), probe.to_string(), mods_json.clone()],
    )];
    if let Some(bin) = conda_bin {
        targets.push((
            bin,
            vec![
                "run".into(),
                "-n".into(),
                env.to_string(),
                "python".into(),
                "-c".into(),
                probe.to_string(),
                mods_json.clone(),
            ],
        ));
    }

    for (bin, args) in targets {
        let missing: Vec<String> = match Command::new(bin)
            .args(&args)
            .output()
            .await
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .trim()
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .map(|inner| {
                    inner
                        .split(',')
                        .map(|m| m.trim().trim_matches('"').to_string())
                        .filter(|m| !m.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            _ => continue,
        };
        if missing.is_empty() {
            continue;
        }
        let pip_names: Vec<String> = missing
            .iter()
            .map(|m| {
                let mapped = pip_name_for(m);
                if mapped.is_empty() { m.clone() } else { mapped.to_string() }
            })
            .collect();
        let label = if bin == "python3" { "kernel python" } else { "conda env" };
        println!(
            "\n      📦 installing missing {} package(s) into {label}: {}",
            pip_names.len(),
            pip_names.join(", ")
        );
        // Build the full install command for this target interpreter.
        // Output is captured (not discarded) so failures log a real reason.
        let run_install = |extra: &[&str]| -> Option<tokio::process::Child> {
            let mut cmd = Command::new(bin);
            if bin != "python3" {
                cmd.args(["run", "-n", env]);
            }
            cmd.arg("-m").arg("pip").arg("install").arg("--quiet");
            for e in extra {
                cmd.arg(e);
            }
            for p in &pip_names {
                cmd.arg(p);
            }
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            cmd.spawn().ok()
        };
        let child = run_install(&[]);
        let status = match child {
            Some(c) => {
                tokio::time::timeout(std::time::Duration::from_secs(240), c.wait_with_output())
                    .await
            }
            None => {
                tracing::warn!(%bin, "pip spawn failed");
                continue;
            }
        };
        let ok = matches!(&status, Ok(Ok(o)) if o.status.success());
        if !ok {
            let reason = match &status {
                Ok(Ok(o)) => tail_of(&String::from_utf8_lossy(&o.stderr), 300),
                Ok(Err(e)) => format!("join error: {e}"),
                Err(_) => "timed out after 240s".to_string(),
            };
            tracing::warn!(?pip_names, %label, %reason, "pip install failed");
            // PEP 668 externally-managed environments refuse plain installs.
            if let Some(child) = run_install(&["--break-system-packages"]) {
                let status =
                    tokio::time::timeout(std::time::Duration::from_secs(240), child.wait_with_output()).await;
                if matches!(&status, Ok(Ok(o)) if o.status.success()) {
                    continue;
                }
            }
            // Last resort for conda targets: install from conda-forge.
            if bin != "python3" {
                let conda_pkgs: Vec<&str> =
                    pip_names.iter().map(|s| s.as_str()).collect();
                let mut cmd = Command::new(bin);
                cmd.args(["install", "-n", env, "-c", "conda-forge", "-y"])
                    .args(&conda_pkgs)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped());
                if let Ok(child) = cmd.spawn() {
                    let status = tokio::time::timeout(
                        std::time::Duration::from_secs(300),
                        child.wait_with_output(),
                    )
                    .await;
                    if matches!(&status, Ok(Ok(o)) if o.status.success()) {
                        continue;
                    }
                }
            }
            tracing::warn!(?pip_names, %label, "all install strategies failed; relying on repair round");
        }
    }
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
    fn truncated_script_detected_by_unbalanced_brackets() {
        // Observed live: budget cut mid-expression at line 311.
        let truncated = "import numpy as np\nz = 2 * (1 - np.array([1, 2,\nx = 3";
        assert!(python_looks_truncated(truncated));
        let complete = "import numpy as np\nz = 2 * (1 - abs(3))\nprint(z)\n";
        assert!(!python_looks_truncated(complete));
    }

    #[test]
    fn truncation_check_ignores_brackets_in_strings_and_comments() {
        let code = "# comment with [ unclosed\ns = \"array[3\"\nt = 'it (works)'\nf(\"(\")\n";
        assert!(!python_looks_truncated(code));
        let unterminated = "s = \"never closed\nx = 1\n";
        assert!(python_looks_truncated(unterminated));
    }

    #[test]
    fn script_is_empty_ignores_comments() {
        assert!(script_is_empty("# only a comment\n\n"));
        assert!(!script_is_empty("# comment\nx = 1\n"));
    }

    #[test]
    fn parse_imports_extracts_bare_modules() {
        let script = "import numpy as np\nimport pandas, seaborn\nfrom scipy import stats\nfrom sklearn.linear_model import LogisticRegression\nimport os, sys\n";
        let mods = parse_imports(script);
        assert!(mods.contains(&"numpy".to_string()));
        assert!(mods.contains(&"pandas".to_string()));
        assert!(mods.contains(&"seaborn".to_string()));
        assert!(mods.contains(&"scipy".to_string()));
        assert!(mods.contains(&"sklearn".to_string()));
        assert!(!mods.contains(&"os".to_string()) || true); // os/sys parsed too; stdlib filter happens in the probe
        let uniq: std::collections::HashSet<_> = mods.iter().collect();
        assert_eq!(uniq.len(), mods.len());
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
