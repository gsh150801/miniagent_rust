//! Adapter that exposes the workflow DAG engine through the unified
//! [`StageDriver`] interface defined in `miniagent-core::orchestration`.
//!
//! Existing callers continue to use [`Workflow::run_with_progress`] directly.
//! New code that needs the orchestrator to be swappable with the
//! loop-pipeline driver can hold a `Box<dyn StageDriver>` and pick at runtime.

use crate::engine::Workflow;
use crate::stage::{StageMetadata, StageOutput};
use miniagent_core::orchestration::{
    OrchestrationError, ProgressFn, StageDriver, StageInput, StageOutcome,
};
use std::sync::{Arc, Mutex};

/// Adapter wrapping a [`Workflow`] (DAG runner) so it implements
/// [`StageDriver`] and can be substituted for the loop-pipeline runner.
pub struct DagRunner {
    workflow: Workflow,
    /// Optional progress callback the server wires into the WebSocket bridge.
    /// Wrapped in `Arc<Mutex<Option<_>>>` so:
    /// 1. `DagRunner` itself satisfies `StageDriver: Send + Sync` (a bare
    ///    `Box<dyn FnMut + Send>` is not Sync).
    /// 2. We can `.take()` the inner closure out per call (it needs `FnMut`)
    ///    while still letting the wrapper survive across runs.
    on_progress: Option<Arc<Mutex<Option<ProgressFn>>>>,
}

impl DagRunner {
    pub fn new(workflow: Workflow) -> Self {
        Self {
            workflow,
            on_progress: None,
        }
    }

    /// Borrow the underlying workflow for callers that still need the rich
    /// DAG-specific API (e.g. `run_with_progress` for streaming).
    pub fn workflow(&self) -> &Workflow {
        &self.workflow
    }

}

#[async_trait::async_trait]
impl StageDriver for DagRunner {
    fn name(&self) -> &str {
        "workflow::DagRunner"
    }

    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError> {
        // Bridge the workflow runner's coarse progress callback into the
        // unified interface. We can't move the closure into `run_with_progress`
        // because it owns `self.on_progress`; we build a fresh per-call
        // adapter that locks the Arc'd mutex to dispatch.
        let cancel = input.cancel.clone();
        let on_progress = self.take_progress_fn();
        let result = if let Some(cb) = on_progress {
            self.workflow.run_with_progress(cancel, cb).await?
        } else {
            self.workflow.run(cancel).await?
        };

        // Collect per-stage outputs into a JSON map (the unified `data` shape)
        // and emit a single digest derived from per-stage metadata.
        let mut data = serde_json::Value::Object(serde_json::Map::new());
        let mut summaries = Vec::new();
        for (stage_id, stage_output) in &result.stage_outputs {
            let key = format!("{:?}", stage_id);
            data.as_object_mut().unwrap().insert(
                key.clone(),
                serde_json::to_value(stage_output).unwrap_or_default(),
            );
            summaries.push(format!(
                "{}:{}",
                key,
                if stage_output.metadata.success { "ok" } else { "failed" }
            ));
        }
        let summary = if summaries.is_empty() {
            "DAG completed".to_string()
        } else {
            format!("DAG completed: {}", summaries.join(", "))
        };

        Ok(StageOutcome::ok(data, summary).with_mode("workflow".to_string()))
    }
}

impl DagRunner {
    /// Pull the optional progress callback out of the `Arc<Mutex>` slot and
    /// wrap it in the `Box<dyn FnMut>` shape `Workflow::run_with_progress`
    /// expects. Returns `None` when no callback is attached so the
    /// no-progress branch stays allocation-free.
    ///
    /// We lock per-call to satisfy the `FnMut` signature: `Workflow`'s
    /// progress fn is called many times in sequence and each call needs
    /// `&mut`, but the callback itself is shared via `Arc<Mutex<_>>` so the
    /// underlying channel sender can outlive any individual driver run.
    fn take_progress_fn(
        &self,
    ) -> Option<Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send + 'static>> {
        let slot = self.on_progress.as_ref()?.clone();
        Some(Box::new(move |name, status, data| {
            if let Ok(mut guard) = slot.lock() {
                if let Some(cb) = guard.as_mut() {
                    cb(name, status, data);
                }
            }
        }))
    }
}

/// Reverse helper: turn a workflow [`StageOutput`] into a unified [`StageOutcome`].
pub fn stage_output_to_outcome(stage_id: &str, output: &StageOutput) -> StageOutcome {
    let data = serde_json::json!({
        "stage_id": stage_id,
        "data": output.data,
        "metadata": output.metadata,
    });
    let summary = match &output.metadata {
        StageMetadata { success: true, error: None, .. } => format!("{} ok", stage_id),
        StageMetadata { success: false, error: Some(e), .. } => {
            format!("{} failed: {}", stage_id, e)
        }
        _ => format!("{} done", stage_id),
    };
    StageOutcome::ok(data, summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::{Stage, StageError, StageHandler};
    use miniagent_core::orchestration::StageInput;
    use tokio_util::sync::CancellationToken;

    /// Minimal `StageHandler` impl used for testing. Returns a deterministic
    /// `StageOutput` without invoking any LLM.
    pub struct NoopHandler;

    #[async_trait::async_trait]
    impl StageHandler for NoopHandler {
        fn name(&self) -> &str {
            "noop"
        }
        fn description(&self) -> &str {
            "no-op"
        }
        async fn execute(
            &self,
            _ctx: &crate::stage::StageContext,
        ) -> Result<StageOutput, StageError> {
            Ok(StageOutput {
                data: serde_json::json!({"noop": true}),
                metadata: StageMetadata {
                    duration_ms: 0,
                    items_processed: 0,
                    success: true,
                    error: None,
                },
            })
        }
    }

    #[tokio::test]
    async fn dag_runner_dispatches_via_stage_driver() {
        let workflow = Workflow::new("test-dag")
            .with_input(serde_json::json!({"prompt": "hello"}))
            .add_stage(Stage::new("s", NoopHandler));
        let runner = DagRunner::new(workflow);
        assert_eq!(runner.name(), "workflow::DagRunner");

        let outcome = runner
            .run(StageInput::new(
                "test",
                serde_json::json!({}),
                CancellationToken::new(),
            ))
            .await
            .unwrap();
        // The DAG has one stage "s" (id is a Uuid but the summary is "ok").
        assert!(
            outcome.summary.contains("ok"),
            "expected ok summary, got: {}",
            outcome.summary
        );
    }

    #[test]
    fn stage_output_to_outcome_branches() {
        let ok = StageOutput {
            data: serde_json::json!({}),
            metadata: StageMetadata {
                duration_ms: 0,
                items_processed: 0,
                success: true,
                error: None,
            },
        };
        let failed = StageOutput {
            data: serde_json::json!({}),
            metadata: StageMetadata {
                duration_ms: 0,
                items_processed: 0,
                success: false,
                error: Some("boom".into()),
            },
        };
        assert_eq!(stage_output_to_outcome("a", &ok).summary, "a ok");
        assert!(stage_output_to_outcome("a", &failed).summary.contains("failed"));
    }
}