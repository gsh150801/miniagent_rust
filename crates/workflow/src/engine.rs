use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use miniagent_checkpoint::CheckpointStore;
use miniagent_core::checkpoint::Checkpoint;
use miniagent_core::error::AgentError;
use miniagent_core::types::{ProjectId, RunId, StageId, StepId};
use tokio_util::sync::CancellationToken;

use crate::retry::RetryPolicy;
use crate::stage::{Stage, StageContext, StageOutput, StageHandler};

pub struct Workflow {
    #[allow(dead_code)]
    name: String,
    stages: Vec<Stage>,
    edges: Vec<(StageId, StageId)>,
    checkpoint_interval: usize,
    retry_policy: RetryPolicy,
    project_id: Option<ProjectId>,
    initial_input: serde_json::Value,
}

impl Workflow {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            stages: Vec::new(),
            edges: Vec::new(),
            checkpoint_interval: 5,
            retry_policy: RetryPolicy::default(),
            project_id: None,
            initial_input: serde_json::Value::Null,
        }
    }

    pub fn add_stage(mut self, stage: Stage) -> Self {
        self.stages.push(stage);
        self
    }

    pub fn add_edge(mut self, from: StageId, to: StageId) -> Self {
        self.edges.push((from, to));
        self
    }

    pub fn with_checkpoint_interval(mut self, n: usize) -> Self {
        self.checkpoint_interval = n;
        self
    }

    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    pub fn with_project(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self
    }

    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.initial_input = input;
        self
    }

    /// Topological sort with cycle detection, returning **waves** of stages.
    ///
    /// Each wave is a group of stages with no dependencies on each other;
    /// all stages within a wave can execute in parallel.
    /// Waves execute sequentially (wave N+1 starts only after wave N completes).
    fn topological_waves(&self) -> Result<Vec<Vec<usize>>, String> {
        let mut in_degree: HashMap<StageId, usize> = HashMap::new();
        let mut adjacency: HashMap<StageId, Vec<StageId>> = HashMap::new();

        for stage in &self.stages {
            in_degree.insert(stage.id, stage.depends_on.len());
            adjacency.entry(stage.id).or_default();
        }

        for (from, to) in &self.edges {
            adjacency.entry(*from).or_default().push(*to);
            *in_degree.entry(*to).or_insert(0) += 1;
        }

        let stage_map: HashMap<StageId, usize> = self.stages
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();

        let mut queue: VecDeque<StageId> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut waves: Vec<Vec<usize>> = Vec::new();
        let mut total_scheduled = 0usize;

        while !queue.is_empty() {
            // Drain current queue → these form the next parallel wave
            let wave_ids: Vec<StageId> = queue.drain(..).collect();
            let mut wave: Vec<usize> = Vec::new();

            for id in &wave_ids {
                if let Some(&idx) = stage_map.get(id) {
                    wave.push(idx);
                }
                if let Some(neighbors) = adjacency.get(id) {
                    for next in neighbors {
                        if let Some(deg) = in_degree.get_mut(next) {
                            *deg -= 1;
                            if *deg == 0 {
                                queue.push_back(*next);
                            }
                        }
                    }
                }
            }

            total_scheduled += wave.len();
            waves.push(wave);
        }

        if total_scheduled != self.stages.len() {
            return Err("Cycle detected in workflow DAG".into());
        }

        Ok(waves)
    }

    /// Internal: run stages in waves with optional progress callback.
    ///
    /// Stages within the same wave run concurrently via `join_all`.
    /// Waves execute sequentially — wave N+1 starts only after all stages
    /// in wave N complete.
    async fn run_inner(
        &self,
        checkpoint_store: Option<&CheckpointStore>,
        cancel: CancellationToken,
        mut on_progress: Option<Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send>>,
    ) -> Result<WorkflowResult, AgentError> {
        let waves = self.topological_waves().map_err(AgentError::invalid_config)?;
        let run_id = RunId::new();
        let mut outputs: HashMap<StageId, StageOutput> = HashMap::new();
        let mut step_count = 0;

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

            // Checkpoint after each wave
            step_count += wave.len();
            let last_stage_name = wave.last()
                .map(|&idx| self.stages[idx].name.as_str())
                .unwrap_or("unknown");
            self.maybe_checkpoint(checkpoint_store, run_id, step_count, last_stage_name);
        }

        let total_stages: usize = waves.iter().map(|w| w.len()).sum();
        Ok(WorkflowResult { run_id, stage_outputs: outputs, total_stages })
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
        checkpoint_store: Option<&CheckpointStore>,
        cancel: CancellationToken,
        on_progress: Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send + 'static>,
    ) -> Result<WorkflowResult, AgentError> {
        self.run_inner(checkpoint_store, cancel, Some(on_progress)).await
    }

    pub async fn run(
        &self,
        checkpoint_store: Option<&CheckpointStore>,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult, AgentError> {
        self.run_inner(checkpoint_store, cancel, None).await
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

    fn maybe_checkpoint(
        &self,
        store: Option<&CheckpointStore>,
        run_id: RunId,
        step_count: usize,
        stage_name: &str,
    ) {
        if step_count % self.checkpoint_interval == 0
            && let (Some(store), Some(pid)) = (store, self.project_id)
        {
            let ckpt = Checkpoint::new(run_id, StepId::new(), step_count, vec![])
                .with_project(pid)
                .with_progress(serde_json::json!({
                    "completed_stages": step_count,
                    "total_stages": self.stages.len(),
                    "last_stage": stage_name,
                }));
            let _ = store.save(&ckpt);
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
