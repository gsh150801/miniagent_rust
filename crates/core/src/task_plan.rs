use serde::{Deserialize, Serialize};

/// A single unit of work within a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUnit {
    pub id: String,
    pub description: String,
    /// Which role/agent type should handle this
    pub assigned_role: String,
    /// IDs of tasks that must complete before this one
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Expected output description
    pub expected_output: String,
    /// Estimated difficulty (simple/medium/hard)
    #[serde(default = "default_difficulty")]
    pub difficulty: String,
    /// Whether this task failed and needs re-execution
    #[serde(default)]
    pub failed: bool,
    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
    /// Actual output after execution
    #[serde(default)]
    pub output: Option<String>,
}

fn default_difficulty() -> String {
    "medium".into()
}

/// The full plan produced by Plan stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub overall_goal: String,
    pub tasks: Vec<TaskUnit>,
    pub max_loops: usize,
}
