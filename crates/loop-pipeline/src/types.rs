use serde::{Deserialize, Serialize};

pub use miniagent_core::{TaskPlan, TaskUnit};

/// Result of the Explore stage: what we learned about the task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationResult {
    /// Clarified/refined task description
    pub clarified_task: String,
    /// Key information gathered to inform planning
    pub findings: Vec<String>,
    /// Estimated complexity
    pub estimated_complexity: String,
    /// Whether task needs further decomposition
    pub needs_decomposition: bool,
}


/// Result of a single task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct TaskResult {
    pub task_id: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub tokens_used: usize,
    /// 校验员的校验报告（三角色协作产物）
    #[serde(default)]
    pub validation_report: Option<serde_json::Value>,
    /// 仲裁员的决策："pass" / "revise" / "supplement"（三角色协作产物）
    #[serde(default)]
    pub arbiter_decision: Option<String>,
}


/// Evaluation result for the entire loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub tasks_completed: usize,
    pub tasks_failed: usize,
    pub tasks_pending: usize,
    pub overall_progress_pct: f64,
    pub failed_task_ids: Vec<String>,
    pub unmet_goals: Vec<String>,
    pub should_continue: bool,
    pub summary: String,
    /// Three-way adjudication record (advocate/challenger/arbiter) produced
    /// when this evaluation decided the pipeline would stop.
    #[serde(default)]
    pub adjudication: Option<serde_json::Value>,
    /// 评估得出的下一环节路由："explore" / "plan" / "dispatch" / "repair"。
    /// pipeline 主循环据此跳过不必要的阶段（如仅重试失败任务时不再
    /// 重新探索/规划）。None = 按默认完整循环执行。
    #[serde(default)]
    pub next_action: Option<String>,
}

/// Repair analysis for a failed task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairAnalysis {
    pub failed_task_id: String,
    pub root_cause: String,
    pub suggested_fix: String,
    pub requires_re_explore: bool,
    pub requires_re_plan: bool,
    pub suggested_new_approach: Option<String>,
    /// 修正后的重试提示词（repair stage 用它增强任务描述后立即重试）
    #[serde(default)]
    pub revised_prompt: Option<String>,
    /// 本次分析对应的是第几次重试（0 = 首次失败后的分析）
    #[serde(default)]
    pub retry_attempt: usize,
}

/// A critique entry from the 3-party review (worker → critic → judge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueEntry {
    pub task_id: String,
    pub critique: String,
    pub judge_verdict: String,
    pub judge_passed: bool,
    pub improvements: Vec<String>,
}

/// The full state shared across all stages in the loop pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    pub original_task: String,
    pub current_task: String,
    pub loop_count: usize,
    pub max_loops: usize,
    pub plan: Option<TaskPlan>,
    pub task_results: Vec<TaskResult>,
    pub evaluations: Vec<EvaluationResult>,
    pub repair_analyses: Vec<RepairAnalysis>,
    pub exploration_history: Vec<ExplorationResult>,
    pub critique_entries: Vec<CritiqueEntry>,
    pub completed: bool,
    pub final_output: Option<String>,
    /// Consecutive loops where overall_progress_pct did not improve.
    /// Reset to 0 whenever progress strictly increases.
    #[serde(default)]
    pub no_progress_streak: usize,
    /// Accumulated token usage across all loops (input + output).
    #[serde(default)]
    pub total_tokens_used: usize,
    /// Collected stage outputs for history replay.
    #[serde(default)]
    pub stage_outputs: Vec<StageOutputRecord>,
    /// User clarifications gathered by the Clarify stage (ask/reply).
    #[serde(default)]
    pub clarifications: Vec<crate::clarify::Clarification>,
    /// Whether the Clarify stage already ran for this pipeline run.
    #[serde(default)]
    pub clarified: bool,
    /// P3 执行中转向：用户在运行期间插入的指令（审计记录）。
    #[serde(default)]
    pub steerings: Vec<String>,
    /// 每个失败任务已被 repair 重试的次数（task_id → 次数），防止无限重试。
    #[serde(default)]
    pub repair_retries: std::collections::HashMap<String, usize>,
    /// evaluate 决定的下一环节路由；pipeline 主循环在轮首消费。
    #[serde(default)]
    pub next_action: Option<String>,
}

/// A lightweight record of a stage output for history replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageOutputRecord {
    pub stage: String,
    pub summary: serde_json::Value,
}

impl PipelineState {
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            original_task: task.into(),
            current_task: String::new(),
            loop_count: 0,
            max_loops: 5,
            plan: None,
            task_results: Vec::new(),
            evaluations: Vec::new(),
            repair_analyses: Vec::new(),
            exploration_history: Vec::new(),
            critique_entries: Vec::new(),
            completed: false,
            final_output: None,
            no_progress_streak: 0,
            total_tokens_used: 0,
            stage_outputs: Vec::new(),
            clarifications: Vec::new(),
            clarified: false,
            steerings: Vec::new(),
            repair_retries: std::collections::HashMap::new(),
            next_action: None,
        }
    }

    pub fn with_max_loops(mut self, n: usize) -> Self {
        self.max_loops = n;
        self
    }
}

/// Messages passed between stages for lightweight coordination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageMessage {
    pub from_stage: String,
    pub to_stage: String,
    pub content: String,
    pub task_id: Option<String>,
}

/// Available roles for dispatch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentRoleType {
    Researcher,
    Executor,
    Writer,
    Critic,
    Synthesizer,
    Analyst,
    Custom(String),
}

impl AgentRoleType {
    pub fn as_str(&self) -> &str {
        match self {
            AgentRoleType::Researcher => "researcher",
            AgentRoleType::Executor => "executor",
            AgentRoleType::Writer => "writer",
            AgentRoleType::Critic => "critic",
            AgentRoleType::Synthesizer => "synthesizer",
            AgentRoleType::Analyst => "analyst",
            AgentRoleType::Custom(s) => s,
        }
    }
}
