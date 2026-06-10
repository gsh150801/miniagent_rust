use std::path::PathBuf;
use std::sync::Arc;
use dashmap::DashMap;
use miniagent_agent::Agent;
use miniagent_checkpoint::CheckpointStore;
use miniagent_memory::manager::MemoryManager;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<Agent>,
    pub memory: Option<Arc<MemoryManager>>,
    pub checkpoint_store: Option<Arc<CheckpointStore>>,
    pub tasks: Arc<DashMap<String, TaskInfo>>,
    pub task_dir: PathBuf,
    pub api_key: String,
    pub max_iterations: usize,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub brief: String,
    pub prompt: String,
    pub status: String,
    pub created_at: String,
    pub result_dir: PathBuf,
    pub files: Vec<String>,
    /// AI response text (stored for history replay)
    #[serde(default)]
    pub response: String,
    /// Workflow plan data for replaying execution cards
    #[serde(default)]
    pub plan: Option<serde_json::Value>,
    /// Per-stage execution data for replaying tool cards
    #[serde(default)]
    pub stage_outputs: Vec<serde_json::Value>,
}

impl AppState {
    pub fn new(agent: Agent, api_key: String) -> Self {
        Self {
            agent: Arc::new(agent),
            memory: None,
            checkpoint_store: None,
            tasks: Arc::new(DashMap::new()),
            task_dir: PathBuf::from("./result"),
            api_key,
            max_iterations: 35,
            max_tokens: 393_216,
        }
    }

    pub fn with_memory(mut self, memory: MemoryManager) -> Self {
        self.memory = Some(Arc::new(memory));
        self
    }

    pub fn with_limits(mut self, max_iterations: usize, max_tokens: u32) -> Self {
        self.max_iterations = max_iterations;
        self.max_tokens = max_tokens;
        self
    }
}
