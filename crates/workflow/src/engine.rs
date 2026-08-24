use std::collections::HashMap;
use std::sync::Arc;

use miniagent_core::error::AgentError;
use miniagent_core::secrets::ApiKey;
use miniagent_core::types::{RunId, StageId};
use tokio_util::sync::CancellationToken;

use crate::retry::RetryPolicy;
use crate::stage::{Stage, StageContext, StageOutput, StageHandler};
use crate::stages::GenericLlmStage;

pub struct Workflow {
    #[allow(dead_code)]
    pub name: String,
    pub stages: Vec<Stage>,
    pub edges: Vec<(StageId, StageId)>,
    retry_policy: RetryPolicy,
    initial_input: serde_json::Value,
    task_dir: Option<String>,
    config: Option<std::sync::Arc<miniagent_core::settings::AppConfig>>,
}

impl Workflow {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            stages: Vec::new(),
            edges: Vec::new(),
            retry_policy: RetryPolicy::default(),
            initial_input: serde_json::Value::Null,
            task_dir: None,
            config: None,
        }
    }

    pub fn with_config(mut self, config: std::sync::Arc<miniagent_core::settings::AppConfig>) -> Self {
        self.config = Some(config);
        self
    }

    pub fn add_stage(mut self, stage: Stage) -> Self {
        self.stages.push(stage);
        self
    }

    pub fn add_edge(mut self, from: StageId, to: StageId) -> Self {
        self.edges.push((from, to));
        self
    }

    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.initial_input = input;
        self
    }

    pub fn with_task_dir(mut self, dir: impl Into<String>) -> Self {
        self.task_dir = Some(dir.into());
        self
    }

    /// Topological sort with cycle detection, returning **waves** of stages.
    ///
    /// Each wave is a group of stages with no dependencies on each other;
    /// all stages within a wave can execute in parallel.
    /// Waves execute sequentially (wave N+1 starts only after wave N completes).
    ///
    /// Implementation delegates to `miniagent_core::orchestration::kahn_waves`
    /// — the canonical Kahn scheduler used by every runner in the workspace.
    /// We map the result back to `Vec<Vec<usize>>` (wave-of-indices) for
    /// the workflow API's stable contract.
    fn topological_waves(&self) -> Result<Vec<Vec<usize>>, String> {
        use miniagent_core::orchestration::{kahn_waves, DagEdge, NodeId};

        // Build (nodes, edges) for the canonical algorithm. StageId is a
        // newtype around Uuid; we convert to string for the unified API.
        let nodes: Vec<NodeId> = self.stages.iter().map(|s| s.id.0.to_string()).collect();
        let edges: Vec<DagEdge> = self
            .edges
            .iter()
            .map(|(from, to)| DagEdge {
                to: to.0.to_string(),
                depends_on: from.0.to_string(),
            })
            .collect();

        // Map NodeId → stage index for callers that need usize indices.
        let stage_map: HashMap<String, usize> = self
            .stages
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.0.to_string(), i))
            .collect();

        let waves = kahn_waves(&nodes, &edges).map_err(|e| e.to_string())?;
        Ok(waves
            .into_iter()
            .map(|wave| {
                wave.into_iter()
                    .filter_map(|id| stage_map.get(&id).copied())
                    .collect()
            })
            .collect())
    }

    /// Internal: run stages in waves with optional progress callback.
    ///
    /// Stages within the same wave run concurrently via `join_all`.
    /// Waves execute sequentially — wave N+1 starts only after all stages
    /// in wave N complete.
    async fn run_inner(
        &self,
        cancel: CancellationToken,
        mut on_progress: Option<Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send>>,
    ) -> Result<WorkflowResult, AgentError> {
        let waves = self.topological_waves().map_err(AgentError::invalid_config)?;
        let run_id = RunId::new();
        let mut outputs: HashMap<StageId, StageOutput> = HashMap::new();

        for (wave_idx, wave) in waves.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            if wave.len() == 1 {
                // Single stage in wave — no concurrency overhead
                let stage = &self.stages[wave[0]];
                if let Some(ref mut cb) = on_progress {
                    cb(&stage.name, "running", None);
                }

                let previous_outputs = self.gather_inputs(stage, &outputs);
                let stage_input = self.build_stage_input(stage);
                let ctx = StageContext::new(stage.id, stage_input, previous_outputs, cancel.child_token());
                let handler = stage.handler.clone();

                match self.execute_stage_with_retry(&handler, &ctx, &self.retry_policy).await {
                    Ok(output) => {
                        let data = output.data.clone();
                        outputs.insert(stage.id, output);
                        if let Some(ref mut cb) = on_progress {
                            cb(&stage.name, "completed", Some(&data));
                        }
                    }
                    Err(e) => {
                        if let Some(ref mut cb) = on_progress {
                            cb(&stage.name, "failed", None);
                        }
                        return Err(e);
                    }
                }
            } else {
                // Multiple stages in wave — execute concurrently
                tracing::info!(wave = wave_idx + 1, stages = wave.len(), "Running wave");

                // Build contexts for all stages in this wave (inputs come from earlier waves only)
                let mut futures: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = (usize, Result<StageOutput, AgentError>)> + Send>>> = Vec::new();

                for &stage_idx in wave {
                    let stage = &self.stages[stage_idx];
                    if let Some(ref mut cb) = on_progress {
                        cb(&stage.name, "running", None);
                    }

                    let previous_outputs = self.gather_inputs(stage, &outputs);
                    let stage_input = self.build_stage_input(stage);
                    let ctx = StageContext::new(stage.id, stage_input, previous_outputs, cancel.child_token());
                    let handler = stage.handler.clone();
                    let retry = self.retry_policy.clone();
                    let stage_idx_owned = stage_idx;

                    // We can't spawn (needs 'static), but we can box the future
                    // and join_all them on the current task.
                    let stage_ref = &self.stages[stage_idx_owned];
                    let _ = stage_ref; // suppress unused warning
                    futures.push(Box::pin(async move {
                        let result = Self::execute_with_retry_static(handler, ctx, retry).await;
                        (stage_idx_owned, result)
                    }));
                }

                // Run all stages in this wave concurrently
                let results = futures_util::future::join_all(futures).await;

                // Process results
                for (stage_idx, result) in results {
                    let stage = &self.stages[stage_idx];
                    match result {
                        Ok(output) => {
                            let data = output.data.clone();
                            outputs.insert(stage.id, output);
                            if let Some(ref mut cb) = on_progress {
                                cb(&stage.name, "completed", Some(&data));
                            }
                        }
                        Err(e) => {
                            if let Some(ref mut cb) = on_progress {
                                cb(&stage.name, "failed", None);
                            }
                            return Err(e);
                        }
                    }
                }
            }

        }

        let total_stages: usize = waves.iter().map(|w| w.len()).sum();
        Ok(WorkflowResult { run_id, stage_outputs: outputs, total_stages })
    }

    /// Incremental execution with failure handling, artifact reuse, and dynamic replanning.
    ///
    /// Unlike `run_inner` which executes the entire DAG once, `run_incremental`:
    /// 1. Skips stages whose artifacts still exist (safe re-execution avoidance)
    /// 2. Tracks per-stage failure state in `WorkflowState`
    /// 3. Supports up to `max_loops` repair cycles via `ReplanRequest`
    /// 4. Allows dynamic DAG modification between loops
    ///
    /// The method loops over the DAG waves. If any stage fails after all retries,
    /// the `repair_handler` is invoked to produce a `ReplanRequest`. The DAG is
    /// modified accordingly and the next loop begins, skipping already-successful
    /// stages whose artifacts are intact.
    pub async fn run_incremental(
        &mut self,
        cancel: CancellationToken,
        mut on_progress: Option<Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send>>,
        mut state: WorkflowState,
        max_loops: usize,
        mut repair_handler: impl FnMut(&WorkflowState, &HashMap<StageId, StageOutput>, &str) -> ReplanRequest,
    ) -> Result<WorkflowResult, AgentError> {
        let task_dir = self.task_dir.clone().unwrap_or_else(crate::stages::default_workflow_dir);

        loop {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            if state.loop_count >= max_loops {
                tracing::warn!(loops = state.loop_count, "Max loops reached, stopping");
                break;
            }

            let waves = self.topological_waves().map_err(AgentError::invalid_config)?;
            let mut outputs: HashMap<StageId, StageOutput> = state.completed_stages.clone();
            let mut any_failure = false;

            for (wave_idx, wave) in waves.iter().enumerate() {
                // Filter: skip stages already completed in this state
                let pending_wave: Vec<usize> = wave.iter()
                    .filter(|&&idx| {
                        let sid = self.stages[idx].id;
                        !state.completed_stages.contains_key(&sid)
                    })
                    .copied()
                    .collect();

                if pending_wave.is_empty() {
                    continue;
                }

                if pending_wave.len() == 1 {
                    // Single pending stage
                    let idx = pending_wave[0];
                    let stage = &self.stages[idx];
                    let sid = stage.id;

                    if let Some(ref mut cb) = on_progress {
                        cb(&stage.name, "running", None);
                    }

                    let previous_outputs = self.gather_inputs(stage, &outputs);
                    let stage_input = self.build_stage_input(stage);
                    let ctx = StageContext::new(sid, stage_input, previous_outputs, cancel.child_token());

                    match self.execute_stage_with_retry(&stage.handler, &ctx, &self.retry_policy).await {
                        Ok(output) => {
                            let data = output.data.clone();
                            // Extract artifact paths from output data
                            let artifacts = Self::extract_artifacts(&output);
                            outputs.insert(sid, output.clone());
                            state.mark_completed(sid, output, artifacts);
                            if let Some(ref mut cb) = on_progress {
                                cb(&stage.name, "completed", Some(&data));
                            }
                        }
                        Err(e) => {
                            any_failure = true;
                            let retryable = Self::is_retryable_error(&e);
                            state.mark_failed(sid, e.to_string(), retryable);
                            if let Some(ref mut cb) = on_progress {
                                cb(&stage.name, "failed", None);
                            }
                            tracing::error!(stage = %stage.name, error = %e, "Stage failed");
                        }
                    }
                } else {
                    // Multiple pending stages — execute concurrently
                    tracing::info!(wave = wave_idx + 1, stages = pending_wave.len(), "Running wave (incremental)");

                    let mut futures: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = (StageId, String, Result<StageOutput, AgentError>)> + Send>>> = Vec::new();

                    for &idx in &pending_wave {
                        let stage = &self.stages[idx];
                        let sid = stage.id;

                        if let Some(ref mut cb) = on_progress {
                            cb(&stage.name, "running", None);
                        }

                        let previous_outputs = self.gather_inputs(stage, &outputs);
                        let stage_input = self.build_stage_input(stage);
                        let ctx = StageContext::new(sid, stage_input, previous_outputs, cancel.child_token());
                        let handler = stage.handler.clone();
                        let retry = self.retry_policy.clone();
                        let name = stage.name.clone();

                        futures.push(Box::pin(async move {
                            let result = Self::execute_with_retry_static(handler, ctx, retry).await;
                            (sid, name, result)
                        }));
                    }

                    if futures.is_empty() {
                        continue;
                    }

                    let results = futures_util::future::join_all(futures).await;

                    for (sid, name, result) in results {
                        match result {
                            Ok(output) => {
                                let data = output.data.clone();
                                let artifacts = Self::extract_artifacts(&output);
                                outputs.insert(sid, output.clone());
                                state.mark_completed(sid, output, artifacts);
                                if let Some(ref mut cb) = on_progress {
                                    cb(&name, "completed", Some(&data));
                                }
                            }
                            Err(e) => {
                                any_failure = true;
                                let retryable = Self::is_retryable_error(&e);
                                state.mark_failed(sid, e.to_string(), retryable);
                                if let Some(ref mut cb) = on_progress {
                                    cb(&name, "failed", None);
                                }
                                tracing::error!(stage = %name, error = %e, "Stage failed");
                            }
                        }
                    }
                }

                // If any stage in this wave failed, stop — don't run downstream waves
                // in the same loop; let the repair handler decide how to proceed.
                if any_failure {
                    break;
                }

            }

            // If no failures, we're done
            if !any_failure {
                state.completed = true;
                break;
            }

            // Failures occurred — check if we can repair/replan
            state.loop_count += 1;

            if state.loop_count >= max_loops {
                tracing::warn!(loops = state.loop_count, failures = ?state.failed_stages.keys().map(|k| k.0.to_string()).collect::<Vec<_>>(), "Max loops reached with failures");
                break;
            }

            // Apply repair/replan
            let replan = repair_handler(&state, &outputs, &task_dir);

            tracing::info!(
                add = replan.add_stages.len(),
                remove = replan.remove_stages.len(),
                "Applying replan (loop {})",
                state.loop_count
            );

            // Build name → StageId map from existing stages
            let mut name_to_id: std::collections::HashMap<String, StageId> = self.stages
                .iter()
                .map(|s| (s.name.clone(), s.id))
                .collect();

            // Remove stages (do this before adding, so depends_on can reference remaining stages)
            let remove_set: std::collections::HashSet<String> = replan.remove_stages.iter().cloned().collect();
            self.stages.retain(|s| !remove_set.contains(&s.name));
            self.edges.retain(|(f, t)| {
                let f_name = name_to_id.iter().find(|(_, id)| **id == *f).map(|(n, _)| n.as_str()).unwrap_or("");
                let t_name = name_to_id.iter().find(|(_, id)| **id == *t).map(|(n, _)| n.as_str()).unwrap_or("");
                !remove_set.contains(f_name) && !remove_set.contains(t_name)
            });
            // Clean up state for removed stages
            for name in &replan.remove_stages {
                if let Some(&sid) = name_to_id.get(name) {
                    state.remove_stage(sid);
                }
            }

            // Apply edge removals
            for (from, to) in &replan.remove_edges {
                let from_id = name_to_id.get(from).copied();
                let to_id = name_to_id.get(to).copied();
                if let (Some(f_id), Some(t_id)) = (from_id, to_id) {
                    self.edges.retain(|(f, t)| !(*f == f_id && *t == t_id));
                }
            }

            // Apply edge additions
            for (from, to) in &replan.add_edges {
                let from_id = name_to_id.get(from).copied();
                let to_id = name_to_id.get(to).copied();
                if let (Some(f_id), Some(t_id)) = (from_id, to_id)
                    && !self.edges.contains(&(f_id, t_id)) {
                        self.edges.push((f_id, t_id));
                    }
            }

            // Apply stage additions
            for (name, _handler_type, deps) in &replan.add_stages {
                let new_id = StageId::new();
                let deps_ids: Vec<StageId> = deps.iter()
                    .filter_map(|d| name_to_id.get(d).copied())
                    .collect();

                // Resolve API key from config for new LLM stages
                let api_key = self.config.as_ref()
                    .and_then(|c| c.require_deepseek_key().ok())
                    .cloned()
                    .unwrap_or_else(|| ApiKey::new(""));
                let provider: Box<dyn miniagent_provider::traits::LlmProvider> =
                    Box::new(miniagent_provider::DeepSeekFlash::new(&api_key));

                let stage = Stage::new(name, GenericLlmStage::new(
                    provider,
                    name,
                    "You are a helpful assistant. Process the input from previous stages.",
                ));

                // Wire dependencies as edges (matching WorkflowBuilder's pattern)
                for dep_id in &deps_ids {
                    self.edges.push((*dep_id, new_id));
                }

                name_to_id.insert(name.clone(), new_id);
                self.stages.push(stage);
            }

            // Update prompt if replan requests it
            if let Some(ref new_prompt) = replan.new_prompt
                && let Some(obj) = self.initial_input.as_object_mut() {
                    obj.insert("prompt".into(), serde_json::json!(new_prompt));
                }

            // Reset retry counts for previously failed stages (fresh attempts after replan)
            for sid in state.failed_stages.keys().copied().collect::<Vec<_>>() {
                state.reset_retry(sid);
            }

            tracing::info!(loop_count = state.loop_count, "Replan applied, continuing");
        }

        let total_stages = state.completed_stages.len() + state.failed_stages.len();
        Ok(WorkflowResult {
            run_id: RunId::new(),
            stage_outputs: state.completed_stages,
            total_stages,
        })
    }

    /// Extract artifact file paths from a stage's output data.
    fn extract_artifacts(output: &StageOutput) -> Vec<String> {
        let mut paths = Vec::new();
        if let Some(arr) = output.data.get("artifacts").and_then(|v| v.as_array()) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    paths.push(s.to_string());
                }
            }
        }
        // Also check for direct file_path field
        if let Some(s) = output.data.get("file_path").and_then(|v| v.as_str()) {
            paths.push(s.to_string());
        }
        paths
    }

    /// Classify an AgentError as retryable or not.
    fn is_retryable_error(e: &AgentError) -> bool {
        let msg = e.to_string().to_lowercase();
        msg.contains("timeout") || msg.contains("rate limit") || msg.contains("429") || msg.contains("502") || msg.contains("503") || msg.contains("504")
    }

    /// Gather upstream outputs for a stage from the completed outputs map.
    fn gather_inputs(
        &self,
        stage: &Stage,
        outputs: &HashMap<StageId, StageOutput>,
    ) -> HashMap<StageId, serde_json::Value> {
        let mut previous_outputs: HashMap<StageId, serde_json::Value> = stage
            .depends_on.iter()
            .filter_map(|dep_id| outputs.get(dep_id).map(|o| (*dep_id, o.data.clone())))
            .collect();

        for (from, to) in &self.edges {
            if *to == stage.id && let Some(output) = outputs.get(from) {
                previous_outputs.insert(*from, output.data.clone());
            }
        }
        previous_outputs
    }

    /// Static retry wrapper that doesn't borrow `self` (for use in spawned futures).
    async fn execute_with_retry_static(
        handler: std::sync::Arc<dyn StageHandler>,
        ctx: StageContext,
        policy: RetryPolicy,
    ) -> Result<StageOutput, AgentError> {
        let mut last_error = None;

        for attempt in 0..=policy.max_retries {
            match handler.execute(&ctx).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    let is_retryable = matches!(&e, crate::stage::StageError::Retryable(_));
                    if is_retryable && attempt < policy.max_retries {
                        let delay = policy.delay_for_attempt(attempt);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = policy.max_retries + 1,
                            error = %e,
                            "Stage attempt failed (retryable), retrying in {:?}",
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        last_error = Some(e);
                    } else {
                        return Err(AgentError::internal(format!("Stage failed: {}", e)));
                    }
                }
            }
        }

        Err(AgentError::internal(format!(
            "Stage exhausted retries: {:?}", last_error
        )))
    }

    pub async fn run_with_progress(
        &self,
        cancel: CancellationToken,
        on_progress: Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send + 'static>,
    ) -> Result<WorkflowResult, AgentError> {
        self.run_inner(cancel, Some(on_progress)).await
    }

    pub async fn run(
        &self,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult, AgentError> {
        self.run_inner(cancel, None).await
    }

    fn build_stage_input(&self, stage: &Stage) -> serde_json::Value {
        if stage.depends_on.is_empty() && self.edges.iter().all(|(_, to)| *to != stage.id) {
            self.initial_input.clone()
        } else {
            let mut input = serde_json::json!({});
            if let Some(val) = self.initial_input.get("prompt").and_then(|v| v.as_str()) {
                input["prompt"] = serde_json::json!(val);
            }
            if let Some(val) = self.initial_input.get("system").and_then(|v| v.as_str()) {
                input["system"] = serde_json::json!(val);
            }
            if let Some(val) = self.initial_input.get("complexity").and_then(|v| v.as_str()) {
                input["complexity"] = serde_json::json!(val);
            }
            if let Some(val) = self.initial_input.get("provider").and_then(|v| v.as_str()) {
                input["provider"] = serde_json::json!(val);
            }
            if let Some(val) = self.initial_input.get("stage_sub_tasks") {
                input["stage_sub_tasks"] = val.clone();
            }
            if let Some(task_dir) = self.initial_input.get("task_dir").and_then(|v| v.as_str()) {
                input["task_dir"] = serde_json::json!(task_dir);
            }
            input
        }
    }

    async fn execute_stage_with_retry(
        &self,
        handler: &Arc<dyn StageHandler>,
        ctx: &StageContext,
        policy: &RetryPolicy,
    ) -> Result<StageOutput, AgentError> {
        let mut last_error = None;

        for attempt in 0..=policy.max_retries {
            match handler.execute(ctx).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    let is_retryable = matches!(&e, crate::stage::StageError::Retryable(_));
                    if is_retryable && attempt < policy.max_retries {
                        let delay = policy.delay_for_attempt(attempt);
                        tracing::warn!(
                            "Stage attempt {}/{} failed (retryable): {}. Retrying in {:?}",
                            attempt + 1, policy.max_retries + 1, e, delay
                        );
                        tokio::time::sleep(delay).await;
                        last_error = Some(e);
                    } else {
                        return Err(AgentError::internal(format!(
                            "Stage failed: {}", e
                        )));
                    }
                }
            }
        }

        Err(AgentError::internal(format!(
            "Stage exhausted retries: {:?}", last_error
        )))
    }

    /// Generate a Mermaid graph visualization
    pub fn visualize(&self) -> String {
        let mut mermaid = String::from("```mermaid\ngraph TD\n");

        for stage in &self.stages {
            let provider_icon = match stage.provider {
                crate::stage::ProviderSelector::Flash => "⚡",
                crate::stage::ProviderSelector::Pro => "🧠",
                crate::stage::ProviderSelector::Auto => "🤖",
            };
            mermaid.push_str(&format!(
                "    {}[\"{} {}<br/>{}<br/>parallel:{}\"]\n",
                stage.id.0.to_string().replace('-', "_"),
                provider_icon,
                stage.name,
                stage.handler.description(),
                stage.parallel,
            ));
        }

        for (from, to) in &self.edges {
            mermaid.push_str(&format!(
                "    {} --> {}\n",
                from.0.to_string().replace('-', "_"),
                to.0.to_string().replace('-', "_"),
            ));
        }

        // Add implicit dependency edges
        for stage in &self.stages {
            for dep in &stage.depends_on {
                mermaid.push_str(&format!(
                    "    {} --> {}\n",
                    dep.0.to_string().replace('-', "_"),
                    stage.id.0.to_string().replace('-', "_"),
                ));
            }
        }

        mermaid.push_str("```\n");
        mermaid
    }
}

#[derive(Debug, Clone)]
pub struct WorkflowResult {
    pub run_id: RunId,
    pub stage_outputs: HashMap<StageId, StageOutput>,
    pub total_stages: usize,
}

// ── Incremental execution types (failure handling + artifact reuse) ──

/// Per-stage execution outcome used internally during incremental runs.
#[derive(Debug, Clone)]
pub enum StageResult {
    Completed {
        output: StageOutput,
        artifact_paths: Vec<String>,
    },
    Failed {
        error: String,
        attempt: usize,
        retryable: bool,
    },
    Skipped {
        reason: String,
    },
}

impl StageResult {
    pub fn is_success(&self) -> bool {
        matches!(self, StageResult::Completed { .. })
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, StageResult::Failed { .. })
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self, StageResult::Skipped { .. })
    }
}

/// Shared mutable state for incremental workflow execution.
///
/// Similar to `PipelineState` in the loop-pipeline crate: tracks which stages
/// have already succeeded (enabling artifact reuse) and which have failed
/// (enabling targeted repair).
#[derive(Debug, Clone, Default)]
pub struct WorkflowState {
    /// Completed stages keyed by `StageId` — outputs are reused across loops.
    pub completed_stages: HashMap<StageId, StageOutput>,
    /// Failed stages with their last error message.
    pub failed_stages: HashMap<StageId, String>,
    /// Retry counts per stage (for exponential back-off).
    pub retry_counts: HashMap<StageId, usize>,
    /// Artifact file paths produced by each stage (used for skip validation).
    pub artifacts: HashMap<StageId, Vec<String>>,
    /// Current execution loop count (for max-loop safety).
    pub loop_count: usize,
    /// Whether the workflow has completed successfully.
    pub completed: bool,
}

impl WorkflowState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a stage as completed and record its artifacts.
    pub fn mark_completed(&mut self, stage_id: StageId, output: StageOutput, artifacts: Vec<String>) {
        self.completed_stages.insert(stage_id, output);
        if !artifacts.is_empty() {
            self.artifacts.insert(stage_id, artifacts);
        }
        self.failed_stages.remove(&stage_id);
    }

    /// Mark a stage as failed with a retry count.
    pub fn mark_failed(&mut self, stage_id: StageId, error: String, retryable: bool) {
        let attempt = self.retry_counts.entry(stage_id).or_insert(0);
        *attempt += 1;
        self.failed_stages.insert(stage_id, error);
        if !retryable {
            self.retry_counts.remove(&stage_id);
        }
    }

    /// Check whether a stage's artifacts still exist on disk (for safe skip).
    pub fn artifacts_exist(&self, stage_id: StageId, task_dir: &str) -> bool {
        let Some(paths) = self.artifacts.get(&stage_id) else {
            return true; // no artifacts → nothing to validate
        };
        paths.iter().all(|p| std::path::Path::new(p).exists() || std::path::Path::new(&format!("{}/{}", task_dir, p)).exists())
    }

    /// Get the retry count for a stage.
    pub fn retry_count(&self, stage_id: StageId) -> usize {
        self.retry_counts.get(&stage_id).copied().unwrap_or(0)
    }

    /// Reset retry count for a stage (e.g., after dynamic replan).
    pub fn reset_retry(&mut self, stage_id: StageId) {
        self.retry_counts.remove(&stage_id);
    }

    /// Remove a stage from all tracking maps (used when DAG is dynamically modified).
    pub fn remove_stage(&mut self, stage_id: StageId) {
        self.completed_stages.remove(&stage_id);
        self.failed_stages.remove(&stage_id);
        self.retry_counts.remove(&stage_id);
        self.artifacts.remove(&stage_id);
    }
}

/// Request to dynamically modify the workflow DAG during execution.
/// Returned by a repair/replan handler and applied before the next wave.
#[derive(Debug, Clone, Default)]
pub struct ReplanRequest {
    /// Stages to add (with their handler, depends_on, etc.).
    pub add_stages: Vec<(String, String, Vec<String>)>, // (name, handler_type, depends_on_names)
    /// Stage names to remove.
    pub remove_stages: Vec<String>,
    /// New edges to add (from_name, to_name).
    pub add_edges: Vec<(String, String)>,
    /// Edges to remove (from_name, to_name).
    pub remove_edges: Vec<(String, String)>,
    /// Optional new prompt for the workflow (replaces initial_input["prompt"]).
    pub new_prompt: Option<String>,
}

impl ReplanRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_stage(mut self, name: impl Into<String>, handler: impl Into<String>, deps: Vec<impl Into<String>>) -> Self {
        self.add_stages.push((name.into(), handler.into(), deps.into_iter().map(|s| s.into()).collect()));
        self
    }

    pub fn remove_stage(mut self, name: impl Into<String>) -> Self {
        self.remove_stages.push(name.into());
        self
    }

    pub fn add_edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.add_edges.push((from.into(), to.into()));
        self
    }

    pub fn remove_edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.remove_edges.push((from.into(), to.into()));
        self
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.new_prompt = Some(prompt.into());
        self
    }
}
