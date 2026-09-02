use std::path::PathBuf;
use std::sync::Arc;
use dashmap::DashMap;
use miniagent_agent::{Agent, EventSenderGuard};
use miniagent_core::models::ModelRegistry;
use miniagent_core::settings::AppConfig;
use miniagent_memory::manager::MemoryManager;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<Agent>,
    pub memory: Option<Arc<MemoryManager>>,
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
    /// P3 执行中转向：运行中任务的待处理 steering 指令。前端发
    /// {type:'steer'} 入队；管线在阶段边界取出并注入后续执行上下文。
    pub steers: Arc<DashMap<String, Vec<String>>>,
    /// Per-task event-sender RAII guards. Each running task registers its
    /// own broadcast sender via `Agent::register_event_sender`; the guard
    /// is stored here and removed (dropped) on completion / cancel so the
    /// shared `Agent`'s event list does not grow without bound and a
    /// finished task no longer receives events.
    pub event_guards: Arc<DashMap<String, EventSenderGuard>>,
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
            tasks: Arc::new(DashMap::new()),
            task_dir: miniagent_core::paths::result_root(),
            config,
            models: Arc::new(std::sync::RwLock::new(models)),
            cancels: Arc::new(DashMap::new()),
            asks: Arc::new(DashMap::new()),
            steers: Arc::new(DashMap::new()),
            event_guards: Arc::new(DashMap::new()),
        }
    }

    pub fn with_memory(mut self, memory: MemoryManager) -> Self {
        self.memory = Some(Arc::new(memory));
        self
    }

    /// P5 跨会话记忆：任务完成时把摘要写入 L1 情景记忆（持久 SQLite），
    /// 供后续任务检索复用（biomni know-how 思路：经验作为可检索资产）。
    pub fn remember_task(&self, task_id: &str, brief: &str, response: &str) {
        let Some(mem) = self.memory.as_ref() else { return };
        let summary = miniagent_memory::types::StructuredSummary {
            background: brief.to_string(),
            method: String::new(),
            key_findings: vec![response.chars().take(600).collect()],
            limitations: vec![],
            contributions: vec![],
            raw_summary: response.chars().take(2_000).collect(),
        };
        let record = miniagent_memory::types::EpisodicRecord {
            id: uuid::Uuid::new_v4(),
            title: format!("[{task_id}] {brief}"),
            content: summary,
            tags: vec!["task".into()],
            source: Some(task_id.to_string()),
            importance: 0.6,
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            access_count: 1,
            decay_rate: 0.02,
            retention_floor: 0.2,
            current_strength: 0.8,
        };
        if let Err(e) = mem.store(&record) {
            tracing::warn!(error = %e, "memory store failed");
        }
    }

    /// P5 跨会话记忆：新任务创建时检索相关历史经验（FTS5 全文匹配），
    /// 返回 top-k 摘要文本供上下文注入。
    pub fn recall_related(&self, prompt: &str, top_k: usize) -> Vec<String> {
        let Some(mem) = self.memory.as_ref() else { return Vec::new() };
        let cfg = miniagent_memory::types::SearchConfig {
            query: prompt.to_string(),
            max_results: top_k,
            importance_threshold: 0.0,
            strength_threshold: 0.0,
            tags: vec![],
            use_fts: true,
            use_vector: false,
            use_graph: false,
        };
        mem.search(&cfg)
            .unwrap_or_default()
            .iter()
            .map(|r| format!("[过往经验] {}: {}", r.title, r.snippet))
            .collect()
    }

    pub fn with_limits(self, max_iterations: usize, max_tokens: u32) -> Self {
        // Limits now come from AppConfig; this method is kept for backward compat
        // but values are sourced from config at the call site.
        let _ = (max_iterations, max_tokens); // already in config
        self
    }
}
