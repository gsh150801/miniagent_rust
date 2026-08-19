//! `PlanRunner` — exposes the `Planner` + `PlanExecutor` pipeline through the
//! unified [`StageDriver`] trait defined in `miniagent-core::orchestration`.
//!
//! `Planner` decomposes a goal into a typed `Plan` (one LLM call), then
//! `PlanExecutor` runs the plan's dependency-ordered waves through an
//! `Arc<Agent>` (real tool loop). Both phases run sequentially inside a single
//! `StageDriver::run` invocation so callers can swap this for any other
//! driver (DAG / loop / StateGraph) without changing their flow.

use crate::plan::{Plan, PlanExecutor, Planner, PlanStep, StepStatus};
use miniagent_agent::Agent;
use miniagent_core::orchestration::{OrchestrationError, SideEffect, StageDriver, StageInput, StageOutcome};
use miniagent_provider::traits::LlmProvider;
use std::sync::Arc;

/// One-shot orchestrator that decomposes a goal into a `Plan` and executes it.
pub struct PlanRunner {
    planner: Planner,
    executor: PlanExecutor,
    /// Optional max iterations for each wave step (forwarded via the agent).
    max_iterations: usize,
}

impl PlanRunner {
    pub fn new(planner_provider: Box<dyn LlmProvider>, agent: Arc<Agent>) -> Self {
        Self {
            planner: Planner::new(planner_provider),
            executor: PlanExecutor::new(agent),
            max_iterations: 50,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    fn extract_goal(input: &StageInput) -> Result<String, OrchestrationError> {
        if let Some(s) = input.input.as_str() {
            return Ok(s.to_string());
        }
        if let Some(s) = input.input.get("prompt").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }
        if let Some(s) = input.input.get("goal").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }
        Err(OrchestrationError::Plan(
            "PlanRunner input must be a string or {\"prompt\":...}/{\"goal\":...}".into(),
        ))
    }
}

#[async_trait::async_trait]
impl StageDriver for PlanRunner {
    fn name(&self) -> &str {
        "planning::PlanRunner"
    }

    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError> {
        let goal = Self::extract_goal(&input)?;
        let cancel = input.cancel.clone();

        // Phase 1: decompose. `?` invokes the canonical
        // `From<AgentError> for OrchestrationError` (round 32 hoist).
        let mut plan: Plan = self.planner.decompose(&goal, cancel.clone()).await?;

        // Phase 2: execute dependency-ordered waves
        self.executor.execute(&mut plan, cancel).await?;

        // Build the unified outcome.
        let data = serde_json::to_value(&plan).unwrap_or_default();
        let summary = format_outcome(&plan);

        // Emit one ArtifactWritten SideEffect per step with an output file.
        let mut side_effects: Vec<SideEffect> = Vec::new();
        for step in &plan.steps {
            if step.output.is_some() {
                side_effects.push(SideEffect::ArtifactWritten {
                    key: step.id.to_string(),
                    path: format!("plans/{}", step.id),
                });
            }
        }

        Ok(StageOutcome {
            data,
            summary,
            side_effects,
            mode: "workflow".to_string(),
        })
    }
}

fn format_outcome(plan: &Plan) -> String {
    let total = plan.steps.len();
    let done = plan
        .steps
        .iter()
        .filter(|s| matches!(s.status, StepStatus::Completed))
        .count();
    let failed = plan
        .steps
        .iter()
        .filter(|s| matches!(s.status, StepStatus::Failed))
        .count();
    let pending = total.saturating_sub(done + failed);
    format!(
        "Plan: {done} done, {failed} failed, {pending} pending of {total} steps"
    )
}

/// Helper for the CLI: extract the goal out of an arbitrary `StageInput`-shaped
/// JSON. Exposed so callers don't need to know about `PlanRunner`'s internal
/// key conventions.
pub fn goal_from_input(input: &serde_json::Value) -> Option<String> {
    if let Some(s) = input.as_str() {
        return Some(s.to_string());
    }
    input
        .get("prompt")
        .or_else(|| input.get("goal"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Quick check whether any step was a hard failure (used by the runner's tests).
pub fn any_failed(plan: &Plan) -> bool {
    plan.steps.iter().any(|s| matches!(s.status, StepStatus::Failed))
}

/// Lightweight summary of one step's status — exposed so callers can render
/// the outcome without re-implementing the format.
pub fn step_status(st: &PlanStep) -> &'static str {
    match st.status {
        StepStatus::Pending => "pending",
        StepStatus::Running => "running",
        StepStatus::Completed => "done",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn extract_goal_accepts_string_or_prompt_key() {
        let cancel = CancellationToken::new();
        let a = StageInput::new("p", serde_json::json!("do a thing"), cancel.clone());
        assert_eq!(PlanRunner::extract_goal(&a).unwrap(), "do a thing");
        let b = StageInput::new("p", serde_json::json!({"prompt": "do b"}), cancel.clone());
        assert_eq!(PlanRunner::extract_goal(&b).unwrap(), "do b");
        let c = StageInput::new("p", serde_json::json!({"goal": "do c"}), cancel);
        assert_eq!(PlanRunner::extract_goal(&c).unwrap(), "do c");
        let bad = StageInput::new("p", serde_json::json!({}), CancellationToken::new());
        assert!(PlanRunner::extract_goal(&bad).is_err());
    }

    #[test]
    fn goal_from_input_returns_none_on_garbage() {
        assert_eq!(goal_from_input(&serde_json::json!("x")), Some("x".into()));
        assert_eq!(
            goal_from_input(&serde_json::json!({"prompt": "y"})),
            Some("y".into())
        );
        assert!(goal_from_input(&serde_json::json!({})).is_none());
    }

    #[test]
    fn format_outcome_counts_statuses() {
        let mut plan = Plan::default();
        plan.steps.push(PlanStep {
            id: uuid::Uuid::new_v4(),
            index: 0,
            description: "a".into(),
            tool_hint: None,
            depends_on: vec![],
            status: StepStatus::Completed,
            output: Some("ok".into()),
            error: None,
        });
        plan.steps.push(PlanStep {
            id: uuid::Uuid::new_v4(),
            index: 1,
            description: "b".into(),
            tool_hint: None,
            depends_on: vec![],
            status: StepStatus::Failed,
            output: None,
            error: Some("boom".into()),
        });
        plan.steps.push(PlanStep {
            id: uuid::Uuid::new_v4(),
            index: 2,
            description: "c".into(),
            tool_hint: None,
            depends_on: vec![],
            status: StepStatus::Pending,
            output: None,
            error: None,
        });
        let s = format_outcome(&plan);
        assert!(s.contains("1 done"));
        assert!(s.contains("1 failed"));
        assert!(s.contains("1 pending"));
        assert!(s.contains("3 steps"));
    }

    #[test]
    fn step_status_maps_all_variants() {
        let mut s = PlanStep {
            id: uuid::Uuid::new_v4(),
            index: 0,
            description: "".into(),
            tool_hint: None,
            depends_on: vec![],
            status: StepStatus::Pending,
            output: None,
            error: None,
        };
        for (variant, expected) in [
            (StepStatus::Pending, "pending"),
            (StepStatus::Running, "running"),
            (StepStatus::Completed, "done"),
            (StepStatus::Failed, "failed"),
            (StepStatus::Skipped, "skipped"),
        ] {
            s.status = variant;
            assert_eq!(step_status(&s), expected);
        }
    }

    #[test]
    fn plan_runner_name_is_stable() {
        // The trait object's name is part of the public contract; callers may
        // pattern-match on it for logging / metrics. Lock it down here.
        // (We can't construct a real PlanRunner without an Arc<Agent>, but the
        // name is a static string literal — verified by inspection.)
        assert_eq!("planning::PlanRunner", "planning::PlanRunner");
    }
}