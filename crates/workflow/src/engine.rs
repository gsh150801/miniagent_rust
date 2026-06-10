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

    /// Topological sort with cycle detection
    fn topological_order(&self) -> Result<Vec<usize>, String> {
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

        let mut queue: VecDeque<StageId> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut order = Vec::new();
        let stage_map: HashMap<StageId, usize> = self.stages
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();

        while let Some(id) = queue.pop_front() {
            if let Some(&idx) = stage_map.get(&id) {
                order.push(idx);
            }

            if let Some(neighbors) = adjacency.get(&id) {
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

        if order.len() != self.stages.len() {
            return Err("Cycle detected in workflow DAG".into());
        }

        Ok(order)
    }

    /// Run the workflow with a sync callback invoked before and after each stage.
    pub async fn run_with_progress<F>(
        &self,
        checkpoint_store: Option<&CheckpointStore>,
        cancel: CancellationToken,
        mut on_progress: F,
    ) -> Result<WorkflowResult, AgentError>
    where
        F: FnMut(&str, &str, Option<&serde_json::Value>) + Send,
    {
        let order = self
            .topological_order()
            .map_err(AgentError::invalid_config)?;

        let run_id = RunId::new();
        let mut outputs: HashMap<StageId, StageOutput> = HashMap::new();
        let mut step_count = 0;

        for &stage_idx in &order {
            let stage = &self.stages[stage_idx];

            on_progress(&stage.name, "running", None);

            let mut previous_outputs: HashMap<StageId, serde_json::Value> = stage
                .depends_on
                .iter()
                .filter_map(|dep_id| {
                    outputs.get(dep_id).map(|o| (*dep_id, o.data.clone()))
                })
                .collect();

            for (from, to) in &self.edges {
                if *to == stage.id
                    && let Some(output) = outputs.get(from) {
                        previous_outputs.insert(*from, output.data.clone());
                    }
            }

            let stage_input = if stage.depends_on.is_empty() && self.edges.iter().all(|(_, to)| *to != stage.id) {
                self.initial_input.clone()
            } else {
                // Pass task_dir to non-root stages so they write to the correct directory
                let mut input = serde_json::Value::Null;
                if let Some(task_dir) = self.initial_input.get("task_dir").and_then(|v| v.as_str()) {
                    input = serde_json::json!({ "task_dir": task_dir });
                }
                input
            };

            let ctx = StageContext {
                stage_id: stage.id,
                input: stage_input,
                previous_outputs,
            };

            let handler = stage.handler.clone();
            let result = self.execute_stage_with_retry(&handler, &ctx, &self.retry_policy).await;

            match result {
                Ok(output) => {
                    let data = output.data.clone();
                    outputs.insert(stage.id, output);
                    on_progress(&stage.name, "completed", Some(&data));
                }
                Err(e) => {
                    on_progress(&stage.name, "failed", None);
                    return Err(e);
                }
            }

            step_count += 1;

            if step_count % self.checkpoint_interval == 0
                && let (Some(store), Some(pid)) = (checkpoint_store, self.project_id) {
                    let ckpt = Checkpoint::new(
                        run_id,
                        StepId::new(),
                        step_count,
                        vec![],
                    )
                    .with_project(pid)
                    .with_progress(serde_json::json!({
                        "completed_stages": step_count,
                        "total_stages": order.len(),
                        "last_stage": stage.name,
                    }));
                    let _ = store.save(&ckpt);
                }

            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
        }

        Ok(WorkflowResult {
            run_id,
            stage_outputs: outputs,
            total_stages: order.len(),
        })
    }

    /// Run the workflow
    pub async fn run(
        &self,
        checkpoint_store: Option<&CheckpointStore>,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult, AgentError> {
        let order = self
            .topological_order()
            .map_err(AgentError::invalid_config)?;

        let run_id = RunId::new();
        let mut outputs: HashMap<StageId, StageOutput> = HashMap::new();
        let mut step_count = 0;

        for &stage_idx in &order {
            let stage = &self.stages[stage_idx];

            // Gather inputs from dependencies
            let mut previous_outputs: HashMap<StageId, serde_json::Value> = stage
                .depends_on
                .iter()
                .filter_map(|dep_id| {
                    outputs.get(dep_id).map(|o| (*dep_id, o.data.clone()))
                })
                .collect();

            // Collect from edges too
            for (from, to) in &self.edges {
                if *to == stage.id
                    && let Some(output) = outputs.get(from) {
                        previous_outputs.insert(*from, output.data.clone());
                    }
            }

            // Use initial_input for root stages (no dependencies)
            let stage_input = if stage.depends_on.is_empty() && self.edges.iter().all(|(_, to)| *to != stage.id) {
                self.initial_input.clone()
            } else {
                // Pass task_dir to non-root stages so they write to the correct directory
                let mut input = serde_json::Value::Null;
                if let Some(task_dir) = self.initial_input.get("task_dir").and_then(|v| v.as_str()) {
                    input = serde_json::json!({ "task_dir": task_dir });
                }
                input
            };

            let ctx = StageContext {
                stage_id: stage.id,
                input: stage_input,
                previous_outputs,
            };

            // Execute with retry
            let handler = stage.handler.clone();
            let output = self.execute_stage_with_retry(&handler, &ctx, &self.retry_policy).await?;

            outputs.insert(stage.id, output);
            step_count += 1;

            // Auto-checkpoint
            if step_count % self.checkpoint_interval == 0
                && let (Some(store), Some(pid)) = (checkpoint_store, self.project_id) {
                    let ckpt = Checkpoint::new(
                        run_id,
                        StepId::new(),
                        step_count,
                        vec![],
                    )
                    .with_project(pid)
                    .with_progress(serde_json::json!({
                        "completed_stages": step_count,
                        "total_stages": order.len(),
                        "last_stage": stage.name,
                    }));
                    let _ = store.save(&ckpt);
                }

            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
        }

        Ok(WorkflowResult {
            run_id,
            stage_outputs: outputs,
            total_stages: order.len(),
        })
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
