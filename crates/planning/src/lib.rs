pub mod plan;
pub mod orchestrator;
pub mod roles;
pub mod state_graph;
pub mod tool_binding;
pub mod agent_profile;
pub mod control_shell;
pub mod hooks;
pub mod event_stream;
pub mod todo_attention;
pub mod context_manager;
pub mod tournament;
pub mod research;

pub use plan::{Plan, PlanStep, Planner, PlanExecutor, StepStatus};
pub use orchestrator::{Orchestrator, OrchestrationPattern, RoleAgent};
pub use roles::{
    AgentRole, FileContext, Blackboard, RoleOutput, EvidenceItem, DecisionRecord, BudgetState,
    ProposerRole, OpponentRole, JudgeRole,
    ResearcherRole, CriticRole, SynthesizerRole, ReviewerRole,
    SupervisorRole, PlannerRole, ExecutorRole, WriterRole, EvaluatorRole, ObserverRole,
    persist_output, load_checkpoint, load_todo, save_todo, append_event, read_role_artifacts,
};
pub use state_graph::{StateGraph, GraphState, GraphNode, NodeOutput, Checkpoint, GraphMessage, ModelTier};
pub use tool_binding::{ToolRegistry, ToolDescriptor, ToolCategory, IoType, SafetyLevel, default_registry};
pub use agent_profile::{AgentProfile, AgentRoleType, ActivationPolicy, default_profiles};
pub use control_shell::{ControlShell, ActivationRule};
pub use hooks::{
    Hook, HookAction, HookContext, HookEvent, HookRegistry,
    AuditLogHook, TokenBudgetHook, ToolApprovalHook, ContextSizeHook, ErrorRecoveryHook,
    PathSandboxHook, DangerousCommandHook, PermissionGuardHook, TaskVerificationHook,
    default_hooks,
};
pub use event_stream::{EventStream, AgentEvent, EventKind};
pub use todo_attention::{TodoAttention, TodoItem, TodoStatus};
pub use context_manager::ContextManager;
pub use tournament::{
    EloEngine, PlayerRating, MatchOutcome,
    DebateRubricScores, DebateSession, Verdict,
    TournamentArena, TournamentPhase, DebateResult,
    NashEquilibriumDetector,
};
pub use research::{
    AgentSpec, AgentTemplate, DynamicAgent, AgentRegistry, AgentStatus, SchedulerRole,
    PrincipalInvestigatorRole, TournamentMasterRole, EvidenceAccumulatorRole, SynthesisJudgeRole,
    build_alzheimers_pipeline, AlzheimersConfig, AlzheimersHypothesisProfile,
};
