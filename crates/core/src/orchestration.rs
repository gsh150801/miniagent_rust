//! Unified orchestration primitives shared by every runner.
//!
//! Before this module, `workflow`, `loop-pipeline`, and `planning` each shipped
//! their own Kahn scheduler, stage-traits, and state containers. This module
//! collects the *minimal* common types so that all runners can implement a
//! single [`StageDriver`] trait and share [`kahn_waves`]. The trait stays
//! narrow on purpose: each runner keeps its own stage trait (e.g.
//! `workflow::StageHandler`, `loop-pipeline::PipelineStage`) and adapts it
//! to the unified interface at the entry point.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Identifier for a node in a DAG / loop-pipeline plan.
pub type NodeId = String;

/// Unified input handed to a stage / driver. JSON-typed so the strongly-typed
/// states of each runner (workflow: `serde_json::Value`; loop-pipeline:
/// `PipelineState`; planning: `Blackboard`) all serialize into the same shape.
#[derive(Debug, Clone)]
pub struct StageInput {
    pub id: NodeId,
    pub input: serde_json::Value,
    pub previous_outputs: HashMap<NodeId, serde_json::Value>,
    pub cancel: CancellationToken,
    /// Which [`DriverKind`] the driver is being invoked under. Lets a driver
    /// branch behaviour or stamp its output without inspecting external state.
    pub mode: &'static str,
}

impl StageInput {
    pub fn new(
        id: impl Into<NodeId>,
        input: serde_json::Value,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            id: id.into(),
            input,
            previous_outputs: HashMap::new(),
            cancel,
            mode: "workflow",
        }
    }

    pub fn with_mode(mut self, mode: &'static str) -> Self {
        self.mode = mode;
        self
    }

}

/// Side effect emitted by a stage, surfaced through `StageOutcome::side_effects`
/// so any driver (DAG / loop / state-graph) can observe cross-cutting events
/// (artifact writes, todo updates, progress, LLM usage) in a uniform way.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SideEffect {
    /// An artifact was written to disk / in-memory store.
    ArtifactWritten {
        key: String,
        path: String,
    },
    /// A todo item's status changed.
    TodoUpdated {
        id: String,
        status: TodoStatus,
        note: Option<String>,
    },
    /// A coarse progress event (phase-level) was emitted.
    ProgressEmitted {
        phase: String,
        status: String,
        detail: Option<String>,
    },
    /// An LLM call completed (or failed) and contributed to usage counters.
    LlmCallMade {
        model: String,
        input_tokens: Option<usize>,
        output_tokens: Option<usize>,
    },
}

/// Status for [`SideEffect::TodoUpdated`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// Unified output returned by a stage / driver. The `data` field carries
/// domain payload (any JSON), `summary` is the human-readable digest that
/// loop-pipeline's `StageOutput.summary` and planning's `RoleOutput.content`
/// both produce, and `side_effects` records cross-cutting events.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StageOutcome {
    pub data: serde_json::Value,
    pub summary: String,
    pub side_effects: Vec<SideEffect>,
    /// Mode stamp the driver was running under (e.g. `"workflow"`, `"loop"`,
    /// `"debate"`). Filled by each driver so the server can route the
    /// outcome to the right post-processing path without re-deriving it.
    /// Stored as `String` (not `&'static str`) so the outcome survives a
    /// JSON round-trip without tying the lifetime to a static literal.
    #[serde(default)]
    pub mode: String,
}

impl StageOutcome {
    pub fn ok(data: serde_json::Value, summary: impl Into<String>) -> Self {
        Self {
            data,
            summary: summary.into(),
            side_effects: Vec::new(),
            mode: "workflow".into(),
        }
    }

    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = mode.into();
        self
    }

    pub fn with_side_effect(mut self, effect: SideEffect) -> Self {
        self.side_effects.push(effect);
        self
    }
}

/// Unified error type returned by orchestration runners.
#[derive(Debug, Error)]
pub enum OrchestrationError {
    #[error("stage error: {0}")]
    Stage(String),
    #[error("planning error: {0}")]
    Plan(String),
    #[error("repair error: {0}")]
    Repair(String),
    #[error("agent error: {0}")]
    Agent(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cancelled")]
    Cancelled,
}

impl From<tokio_util::sync::CancellationToken> for OrchestrationError {
    fn from(_: tokio_util::sync::CancellationToken) -> Self {
        OrchestrationError::Cancelled
    }
}

/// Canonical mapping from [`AgentError`] to [`OrchestrationError`].
///
/// Every orchestration runner (`workflow::DagRunner`, `loop_pipeline::LoopRunner`,
/// and the `planning::runners::*` family) used to carry its own verbatim copy of
/// this match — round 32 hoisted it here as the single source of truth. Callers
/// should use `?`, `.into()`, or `.map_err(OrchestrationError::from)?` instead
/// of re-matching by hand.
///
/// Mapping rationale:
/// - `Cancelled` ⇒ `Cancelled` (propagate the cancellation signal).
/// - `InvalidConfig` / `InvalidState` ⇒ `Plan` (recoverable planning-level failure).
/// - `Checkpoint` ⇒ `Stage("checkpoint: …")` (preserves the checkpoint context tag).
/// - Everything else ⇒ `Stage` (generic, message-preserving).
impl From<crate::error::AgentError> for OrchestrationError {
    fn from(e: crate::error::AgentError) -> Self {
        use crate::error::AgentError;
        match e {
            AgentError::Cancelled => OrchestrationError::Cancelled,
            AgentError::InvalidConfig(msg) | AgentError::InvalidState(msg) => {
                OrchestrationError::Plan(msg)
            }
            AgentError::Checkpoint(msg) => OrchestrationError::Stage(format!("checkpoint: {msg}")),
            AgentError::Provider(msg)
            | AgentError::ToolNotFound(msg)
            | AgentError::PolicyDenied(msg)
            | AgentError::Serialization(msg)
            | AgentError::Internal(msg) => OrchestrationError::Stage(msg),
            AgentError::Tool { message, .. } => OrchestrationError::Stage(message),
            AgentError::BudgetExhausted { .. } => {
                OrchestrationError::Stage("budget exhausted".into())
            }
            AgentError::ContextOverflow { .. } => {
                OrchestrationError::Stage("context overflow".into())
            }
        }
    }
}

/// Edge in a DAG used by [`kahn_waves`].
///
/// `depends_on` enumerates the IDs that must complete *before* `to` may run.
#[derive(Debug, Clone)]
pub struct DagEdge {
    pub to: NodeId,
    pub depends_on: NodeId,
}

/// Compute Kahn-style topological waves from `(node, dependencies)`.
///
/// Returns one wave per layer: wave `i` contains all nodes whose dependencies
/// were satisfied by waves `0..i`. Single-node waves stay sequential (no
/// fan-out overhead), wide waves execute in parallel.
///
/// This is the canonical implementation; `workflow::engine::topological_waves`,
/// `loop-pipeline::dispatch::resolve_execution_order`, `planning::plan::execution_order`,
/// and `planning::state_graph::compile` all reduce to this same algorithm and
/// can (in subsequent phases) delegate here.
pub fn kahn_waves(
    nodes: &[NodeId],
    edges: &[DagEdge],
) -> Result<Vec<Vec<NodeId>>, OrchestrationError> {
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    for n in nodes {
        indegree.entry(n.as_str()).or_insert(0);
    }
    for e in edges {
        *indegree.entry(e.to.as_str()).or_insert(0) += 1;
    }
    // Build the forward adjacency from `depends_on -> to` (i.e. what `depends_on`
    // unlocks once it completes).
    let mut unlocks: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        unlocks.entry(e.depends_on.as_str()).or_default().push(e.to.as_str());
    }

    let mut waves: Vec<Vec<NodeId>> = Vec::new();
    let mut frontier: Vec<&str> = nodes
        .iter()
        .filter(|n| indegree.get(n.as_str()).copied().unwrap_or(0) == 0)
        .map(|n| n.as_str())
        .collect();
    // Sort frontier for deterministic output (matches the existing tests).
    frontier.sort();

    while !frontier.is_empty() {
        let wave: Vec<NodeId> = frontier.iter().map(|s| (*s).to_string()).collect();
        let mut next_frontier: Vec<&str> = Vec::new();
        for done in &frontier {
            if let Some(unlocked) = unlocks.get(done) {
                for &u in unlocked {
                    if let Some(d) = indegree.get_mut(u) {
                        *d -= 1;
                        if *d == 0 {
                            next_frontier.push(u);
                        }
                    }
                }
            }
        }
        next_frontier.sort();
        next_frontier.dedup();
        waves.push(wave);
        frontier = next_frontier;
    }

    // Sanity: if any node still has non-zero indegree, there's a cycle.
    let unresolved: Vec<NodeId> = indegree
        .into_iter()
        .filter(|(_, d)| *d > 0)
        .map(|(n, _)| n.to_string())
        .collect();
    if !unresolved.is_empty() {
        return Err(OrchestrationError::Plan(format!(
            "cycle detected involving: {unresolved:?}"
        )));
    }
    Ok(waves)
}

/// Trait every orchestration runner implements.
///
/// Three runners are targeted: **DagRunner** (workflow), **LoopRunner**
/// (loop-pipeline), and (in the future) the StateGraph runner from planning.
/// Adding a new runner only requires implementing this trait plus an adapter
/// that converts its native stage trait (`StageHandler` / `PipelineStage`)
/// into [`StageOutcome`].
#[async_trait]
pub trait StageDriver: Send + Sync {
    fn name(&self) -> &str;

    /// Execute the orchestrated pipeline to completion.
    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError>;
}

/// Adapt any callable stage into a [`StageDriver::run`] body. Provided as a
/// free function rather than a separate trait so existing runners don't need
/// to invent a new stage-trait hierarchy: just call `adapt_stage` and forward
/// the outcome.
pub async fn adapt_stage<F, Fut>(runner: &str, f: F) -> Result<StageOutcome, OrchestrationError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<StageOutcome, OrchestrationError>>,
{
    let outcome = f().await?;
    if outcome.summary.is_empty() {
        tracing::debug!(runner, "stage produced empty summary");
    }
    Ok(outcome)
}

/// Three execution backends the server can dispatch to. Each variant maps 1:1
/// onto a [`StageDriver`] impl: `Workflow` → `workflow::DagRunner`,
/// `Loop` → `loop_pipeline::LoopRunner`, `Debate` → `planning::runners::DebateRunner`.
///
/// Lives in `miniagent_core` so the WebSocket layer, the driver factories, and
/// any future CLI surfaces all share the same vocabulary. A new backend only
/// needs to add a variant here and a matching arm in the server's `build_driver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    /// LLM-decides DAG path: explore→ask→plan→dispatch→feedback, served by
    /// `workflow::DagRunner` (this is the historical default and what the
    /// server falls back to when no mode is specified).
    Workflow,
    /// Cyclic Explore→Plan→Dispatch→Evaluate→Repair path, served by
    /// `loop_pipeline::LoopRunner`. Useful when the task may need several
    /// iterations to converge.
    Loop,
    /// Proposer vs Opponent → Judge multi-round debate, served by
    /// `planning::runners::DebateRunner`.
    Debate,
}

impl DriverKind {
    /// Stable lowercase identifier used on the wire (matches the
    /// `payload.mode` value sent by the frontend).
    pub const fn as_str(self) -> &'static str {
        match self {
            DriverKind::Workflow => "workflow",
            DriverKind::Loop => "loop",
            DriverKind::Debate => "debate",
        }
    }

    /// Parse the wire string, defaulting to `Workflow` for unknown / empty
    /// values so old clients keep working.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "loop" | "loop_pipeline" | "loop-pipeline" => DriverKind::Loop,
            "debate" => DriverKind::Debate,
            // "workflow" + anything else falls through to the historical default
            _ => DriverKind::Workflow,
        }
    }
}

impl Default for DriverKind {
    fn default() -> Self {
        DriverKind::Workflow
    }
}

/// Coarse progress callback shared by all drivers. Mirrors the signature used
/// by `Workflow::run_with_progress` so the server can wire a single
/// `progress_fn: ProgressFn` regardless of which driver is running.
///
/// `name` is the stage identifier (e.g. `"explore"`, `"plan"`, `"dispatch"`).
/// `status` is one of `"running"`, `"completed"`, `"failed"`. `data` carries
/// optional JSON payload (stage summary, response preview, etc.).
pub type ProgressFn = Box<dyn FnMut(&str, &str, Option<&serde_json::Value>) + Send + 'static>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kahn_waves_handles_chain() {
        // a -> b -> c
        let nodes = vec!["a".into(), "b".into(), "c".into()];
        let edges = vec![
            DagEdge {
                to: "b".into(),
                depends_on: "a".into(),
            },
            DagEdge {
                to: "c".into(),
                depends_on: "b".into(),
            },
        ];
        let waves = kahn_waves(&nodes, &edges).unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["a"]);
        assert_eq!(waves[1], vec!["b"]);
        assert_eq!(waves[2], vec!["c"]);
    }

    #[test]
    fn kahn_waves_handles_diamond() {
        //   a
        //  / \
        // b   c
        //  \ /
        //   d
        let nodes = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let edges = vec![
            DagEdge {
                to: "b".into(),
                depends_on: "a".into(),
            },
            DagEdge {
                to: "c".into(),
                depends_on: "a".into(),
            },
            DagEdge {
                to: "d".into(),
                depends_on: "b".into(),
            },
            DagEdge {
                to: "d".into(),
                depends_on: "c".into(),
            },
        ];
        let waves = kahn_waves(&nodes, &edges).unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["a"]);
        assert_eq!(waves[1], vec!["b", "c"]); // parallel
        assert_eq!(waves[2], vec!["d"]);
    }

    #[test]
    fn kahn_waves_handles_independent_nodes() {
        // a, b, c with no edges
        let nodes = vec!["a".into(), "b".into(), "c".into()];
        let edges = vec![];
        let waves = kahn_waves(&nodes, &edges).unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn kahn_waves_detects_cycle() {
        // a -> b -> a
        let nodes = vec!["a".into(), "b".into()];
        let edges = vec![
            DagEdge {
                to: "b".into(),
                depends_on: "a".into(),
            },
            DagEdge {
                to: "a".into(),
                depends_on: "b".into(),
            },
        ];
        let err = kahn_waves(&nodes, &edges).unwrap_err();
        match err {
            OrchestrationError::Plan(msg) => assert!(msg.contains("cycle")),
            other => panic!("expected Plan error, got {other:?}"),
        }
    }

    #[test]
    fn stage_outcome_roundtrips_json() {
        let outcome = StageOutcome::ok(serde_json::json!({"k": 1}), "did a thing")
            .with_side_effect(SideEffect::ArtifactWritten {
                key: "k".into(),
                path: "/tmp/k".into(),
            });
        let json = serde_json::to_string(&outcome).unwrap();
        let back: StageOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back.summary, "did a thing");
        assert_eq!(back.side_effects.len(), 1);
    }

    #[test]
    fn side_effect_todo_updated_serializes() {
        let effect = SideEffect::TodoUpdated {
            id: "DA-1".into(),
            status: TodoStatus::Completed,
            note: Some("ran without errors".into()),
        };
        let json = serde_json::to_string(&effect).unwrap();
        assert!(json.contains("\"kind\":\"todo_updated\""));
        assert!(json.contains("\"status\":\"completed\""));
    }

    #[tokio::test]
    async fn adapt_stage_passes_through() {
        let outcome = adapt_stage("test", || async {
            Ok::<_, OrchestrationError>(StageOutcome::ok(
                serde_json::json!({}),
                "from closure",
            ))
        })
        .await
        .unwrap();
        assert_eq!(outcome.summary, "from closure");
    }

    #[test]
    fn agent_error_cancelled_maps_to_orchestration_cancelled() {
        use crate::error::AgentError;
        let mapped: OrchestrationError = AgentError::Cancelled.into();
        assert!(matches!(mapped, OrchestrationError::Cancelled));
    }

    #[test]
    fn agent_error_invalid_state_becomes_plan() {
        use crate::error::AgentError;
        let mapped: OrchestrationError = AgentError::InvalidState("nope".into()).into();
        match mapped {
            OrchestrationError::Plan(msg) => assert_eq!(msg, "nope"),
            other => panic!("expected Plan, got {other:?}"),
        }
    }

    #[test]
    fn agent_error_invalid_config_becomes_plan() {
        use crate::error::AgentError;
        let mapped: OrchestrationError = AgentError::InvalidConfig("bad".into()).into();
        assert!(matches!(mapped, OrchestrationError::Plan(_)));
    }

    #[test]
    fn agent_error_checkpoint_preserves_context_tag() {
        use crate::error::AgentError;
        let mapped: OrchestrationError = AgentError::Checkpoint("snap".into()).into();
        match mapped {
            OrchestrationError::Stage(msg) => assert_eq!(msg, "checkpoint: snap"),
            other => panic!("expected Stage, got {other:?}"),
        }
    }

    #[test]
    fn agent_error_message_variants_become_stage() {
        use crate::error::AgentError;
        for (name, err, expected) in [
            ("provider", AgentError::Provider("p".into()), "p"),
            (
                "tool",
                AgentError::Tool {
                    tool: "t".into(),
                    message: "boom".into(),
                },
                "boom",
            ),
            ("tool_not_found", AgentError::ToolNotFound("x".into()), "x"),
            ("policy_denied", AgentError::PolicyDenied("d".into()), "d"),
            ("serialization", AgentError::Serialization("s".into()), "s"),
            ("internal", AgentError::Internal("i".into()), "i"),
        ] {
            let mapped: OrchestrationError = err.into();
            match mapped {
                OrchestrationError::Stage(msg) => assert_eq!(msg, expected, "{name} case"),
                other => panic!("{name}: expected Stage, got {other:?}"),
            }
        }
    }

    #[test]
    fn agent_error_budget_and_overflow_become_fixed_stage_messages() {
        use crate::error::AgentError;
        let budget: OrchestrationError =
            AgentError::BudgetExhausted { budget_type: "tokens".into() }.into();
        assert!(matches!(budget, OrchestrationError::Stage(_)));
        let overflow: OrchestrationError = AgentError::ContextOverflow {
            input_tokens: 10,
            limit_tokens: 1,
        }
        .into();
        match overflow {
            OrchestrationError::Stage(msg) => assert_eq!(msg, "context overflow"),
            other => panic!("expected Stage, got {other:?}"),
        }
    }
}