pub mod plan;
pub mod roles;
pub mod runners;
pub mod state_graph;
pub mod event_stream;
pub mod todo_attention;

pub use plan::{Plan, PlanStep, Planner, PlanExecutor, StepStatus};
pub use roles::{
    AgentRole, FileContext, Blackboard, RoleOutput, EvidenceItem, DecisionRecord, BudgetState,
    ProposerRole, OpponentRole, JudgeRole,
    ResearcherRole, CriticRole, SynthesizerRole, ReviewerRole,
    SupervisorRole, PlannerRole, ExecutorRole, WriterRole, EvaluatorRole, ObserverRole,
    persist_output, load_checkpoint, load_todo, save_todo, append_event, read_role_artifacts,
};
pub use state_graph::{StateGraph, GraphState, GraphNode, NodeOutput, Checkpoint, GraphMessage};
pub use event_stream::{EventStream, AgentEvent as PlanningAgentEvent, EventKind};
pub use todo_attention::{TodoAttention, TodoItem, TodoStatus as PlanningTodoStatus};
// `ModelTier` 统一从 `miniagent_core` 重导出。
pub use miniagent_core::ModelTier;

// Removed in round 30 (planning crate consolidation):
//   tournament/ (1063 LoC, zero production references),
//   research/   (1145 LoC, zero external references),
//   hooks/      (662 LoC, demo-only),
//   control_shell, tool_binding, agent_profile, context_manager
//   (all CLI-demo-only or zero-ref).
// Net reduction: ~3.2k lines, ~50% of the crate.
