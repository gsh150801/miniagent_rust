use std::path::PathBuf;
use std::sync::Arc;
use dashmap::DashMap;
use miniagent_agent::Agent;
use miniagent_checkpoint::CheckpointStore;
use miniagent_core::models::ModelRegistry;
use miniagent_core::settings::AppConfig;
use miniagent_memory::manager::MemoryManager;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<Agent>,
    pub memory: Option<Arc<MemoryManager>>,
    pub checkpoint_store: Option<Arc<CheckpointStore>>,
    pub tasks: Arc<DashMap<String, TaskInfo>>,
    pub task_dir: PathBuf,
    pub config: Arc<AppConfig>,
    /// Runtime LLM model registry (built-ins from env + customs from
    /// models.json). RwLock: /api/models handlers mutate; task paths read.
    pub models: Arc<std::sync::RwLock<ModelRegistry>>,
    /// Per-task cancellation tokens, keyed by task_id.
    pub cancels: Arc<DashMap<String, CancellationToken>>,
    /// Per-task ask reply channels: 当 task 执行需要向用户提问时，注册一个 oneshot::Sender；
    /// 前端回复 {type:'ask_reply'} 时，handle_ws 取出 Sender 并 send(answer) 唤醒 task。
    pub asks: Arc<DashMap<String, tokio::sync::oneshot::Sender<String>>>,
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
    /// AI response text (stored for history replay, kept for backward compat)
    #[serde(default)]
    pub response: String,
    /// Full multi-turn message history: each entry is {role, content}
    #[serde(default)]
    pub messages: Vec<serde_json::Value>,
    /// Workflow plan data for replaying execution cards
    #[serde(default)]
    pub plan: Option<serde_json::Value>,
    /// Per-stage execution data for replaying tool cards
    #[serde(default)]
    pub stage_outputs: Vec<serde_json::Value>,
    /// Full event trace: every AgentEvent (tool calls, skill invocations, etc.)
    /// persisted for post-hoc traceability (需求2: 全链路可追溯)。
    /// Each entry is the serialized AgentEvent with a timestamp.
    #[serde(default)]
    pub event_log: Vec<serde_json::Value>,
}

impl AppState {
    pub fn new(agent: Arc<Agent>, config: Arc<AppConfig>) -> Self {
        let models = ModelRegistry::load(&config);
        Self {
            agent,
            memory: None,
            checkpoint_store: None,
            tasks: Arc::new(DashMap::new()),
            task_dir: miniagent_core::paths::result_root(),
            config,
            models: Arc::new(std::sync::RwLock::new(models)),
            cancels: Arc::new(DashMap::new()),
            asks: Arc::new(DashMap::new()),
        }
    }

    pub fn with_memory(mut self, memory: MemoryManager) -> Self {
        self.memory = Some(Arc::new(memory));
        self
    }

    pub fn with_limits(self, max_iterations: usize, max_tokens: u32) -> Self {
        // Limits now come from AppConfig; this method is kept for backward compat
        // but values are sourced from config at the call site.
        let _ = (max_iterations, max_tokens); // already in config
        self
    }
}
