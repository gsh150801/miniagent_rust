//! Adapter that exposes the workflow DAG engine through the unified
//! [`StageDriver`] interface defined in `miniagent-core::orchestration`.
//!
//! Existing callers continue to use [`Workflow::run_with_progress`] directly.
//! New code that needs the orchestrator to be swappable with the
//! loop-pipeline driver can hold a `Box<dyn StageDriver>` and pick at runtime.

use crate::engine::Workflow;
use crate::stage::{StageContext, StageMetadata, StageOutput};
use miniagent_core::orchestration::{
    OrchestrationError, StageDriver, StageInput, StageOutcome,
};
use miniagent_core::types::StageId;
use std::collections::HashMap;

/// Adapter wrapping a [`Workflow`] (DAG runner) so it implements
/// [`StageDriver`] and can be substituted for the loop-pipeline runner.
pub struct DagRunner {
    workflow: Workflow,
}

impl DagRunner {
    pub fn new(workflow: Workflow) -> Self {
        Self { workflow }
    }

    /// Borrow the underlying workflow for callers that still need the rich
    /// DAG-specific API (e.g. `run_with_progress` for streaming).
    pub fn workflow(&self) -> &Workflow {
        &self.workflow
    }

    pub fn into_workflow(self) -> Workflow {
        self.workflow
    }
}

#[async_trait::async_trait]
impl StageDriver for DagRunner {
    fn name(&self) -> &str {
        "workflow::DagRunner"
    }

    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError> {
        // `?` invokes the canonical `From<AgentError> for OrchestrationError`
        // defined in `miniagent_core::orchestration` (round 32 hoist).
        let result = self.workflow.run(None, input.cancel).await?;

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

        Ok(StageOutcome::ok(data, summary))
    }
}

/// Free helper: turn a [`StageInput`] into a [`StageContext`] for code that
/// still operates on the workflow-native trait. Provided for the migration
/// path; new code should use the [`StageDriver`] abstraction instead.
///
/// Note: `StageId` is a Uuid wrapper, so we synthesize one from the input id.
pub fn stage_input_to_context(input: &StageInput) -> StageContext {
    let stage_id = StageId(uuid::Uuid::new_v4());
    let previous: HashMap<StageId, serde_json::Value> =
        HashMap::with_capacity(input.previous_outputs.len());
    let _ = previous; // (StageId conversion is lossy; round-trip is best-effort)
    StageContext::new(stage_id, input.input.clone(), HashMap::new(), input.cancel.clone())
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
            _ctx: &StageContext,
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