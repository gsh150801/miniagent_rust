//! Adapters that expose planning's three remaining abstractions through the
//! unified [`StageDriver`](miniagent_core::orchestration::StageDriver) trait.
//!
//! - [`plan_runner::PlanRunner`] — Planner + PlanExecutor (CLI `plan` command)
//! - [`state_graph_runner::StateGraphRunner`] — compiled graph runner (CLI `team`)
//! - [`debate_runner::DebateRunner`] — Proposer/Opponent/Judge loop (CLI `debate`)
//! - [`role_runner::SingleRoleRunner`] — single AgentRole execution (CLI utility)

pub mod plan_runner;
pub mod state_graph_runner;
pub mod debate_runner;
pub mod role_runner;

pub use plan_runner::{PlanRunner, any_failed, goal_from_input, step_status};
pub use state_graph_runner::StateGraphRunner;
pub use debate_runner::{DebateRunner, DebateRound};
pub use role_runner::SingleRoleRunner;