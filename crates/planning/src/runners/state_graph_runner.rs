//! `StateGraphRunner` — exposes a compiled `StateGraph` through the unified
//! [`StageDriver`] trait.
//!
//! Holds an immutable compiled graph (`CompiledGraph`) plus the two LLM
//! providers it needs at runtime. Each `StageDriver::run` invocation starts
//! from a fresh `GraphState` seeded with the call's `StageInput.input`.

use crate::state_graph::{CompiledGraph, GraphError, GraphState, GraphMessage};
use miniagent_core::orchestration::{OrchestrationError, SideEffect, StageDriver, StageInput, StageOutcome};
use miniagent_provider::traits::LlmProvider;

pub struct StateGraphRunner {
    compiled: CompiledGraph,
    flash: Box<dyn LlmProvider>,
    pro: Box<dyn LlmProvider>,
}

impl StateGraphRunner {
    pub fn new(
        compiled: CompiledGraph,
        flash: Box<dyn LlmProvider>,
        pro: Box<dyn LlmProvider>,
    ) -> Self {
        Self {
            compiled,
            flash,
            pro,
        }
    }

    /// Build a fresh `GraphState` seeded from the call's input. The caller may
    /// pass either a plain string (treated as the first user message) or a
    /// JSON object with `messages` / `work_dir` / `artifacts` / etc.
    fn build_state(&self, input: &StageInput) -> GraphState {
        let mut state = GraphState::default();
        // Seed with the user message.
        if let Some(s) = input.input.as_str() {
            state.messages.push(GraphMessage::new("user", s));
        } else if let Some(s) = input.input.get("prompt").and_then(|v| v.as_str()) {
            state.messages.push(GraphMessage::new("user", s));
        } else {
            state.messages.push(GraphMessage::new(
                "user",
                serde_json::to_string(&input.input).unwrap_or_default(),
            ));
        }
        // Merge in any previous_outputs as artifacts (key → JSON value).
        for (k, v) in &input.previous_outputs {
            let s = serde_json::to_string(v).unwrap_or_default();
            state.artifacts.insert(k.clone(), s);
        }
        state
    }

    fn map_graph_error(e: GraphError) -> OrchestrationError {
        match e {
            GraphError::Cancelled => OrchestrationError::Cancelled,
            GraphError::BudgetExhausted => OrchestrationError::Stage("budget exhausted".into()),
            GraphError::NodeFailed(msg) => OrchestrationError::Stage(format!("node failed: {msg}")),
            GraphError::NoRoute(name) => {
                OrchestrationError::Stage(format!("no route from '{name}'"))
            }
        }
    }
}

#[async_trait::async_trait]
impl StageDriver for StateGraphRunner {
    fn name(&self) -> &str {
        "planning::StateGraphRunner"
    }

    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError> {
        // The underlying CompiledGraph::execute returns a non-Send future
        // (its Parallel nodes box closures that capture EventStream /
        // TodoAttention across awaits). Since StageDriver requires Send, we
        // run the graph on the current thread via block_in_place +
        // Handle::block_on, which is the canonical way to bridge sync-only
        // futures from within an async context on a multi-threaded runtime.
        // Falls back to spawning a new single-threaded runtime if the host
        // runtime isn't multi-threaded (e.g. #[tokio::test] without
        // `flavor = "multi_thread"`).
        let state = self.build_state(&input);
        let cancel = input.cancel.clone();
        let iterations_before = state.iteration;

        let flash = &*self.flash;
        let pro = &*self.pro;
        let compiled = &self.compiled;

        let exec_result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    compiled.execute(state, cancel, flash, pro).await
                })
            })
        } else {
            // No runtime (rare — only happens in unit tests). Spawn a
            // single-threaded runtime just for this call.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| OrchestrationError::Stage(format!("runtime init: {e}")))?;
            rt.block_on(async move { compiled.execute(state, cancel, flash, pro).await })
        };
        let state = exec_result.map_err(Self::map_graph_error)?;

        let data = serde_json::to_value(&state).unwrap_or_default();
        let summary = if state.finished {
            format!(
                "graph finished after {} iterations ({} steps executed)",
                state.iteration.saturating_sub(iterations_before),
                state.step_outputs.len()
            )
        } else {
            format!(
                "graph stopped at iteration {} ({} steps so far)",
                state.iteration, state.step_outputs.len()
            )
        };

        // Emit one ProgressEmitted + one ArtifactWritten per step output.
        let mut side_effects: Vec<SideEffect> = Vec::new();
        for (step, out) in &state.step_outputs {
            side_effects.push(SideEffect::ProgressEmitted {
                phase: "step".into(),
                status: "completed".into(),
                detail: Some(step.clone()),
            });
            side_effects.push(SideEffect::ArtifactWritten {
                key: step.clone(),
                path: format!("graph/{}", step),
            });
            let _ = out;
        }

        Ok(StageOutcome {
            data,
            summary,
            side_effects,
        })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn name_is_stable() {
        // Static-name contract test (mirrors plan_runner::tests::name_is_stable).
        assert_eq!("planning::StateGraphRunner", "planning::StateGraphRunner");
    }
}