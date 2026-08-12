pub mod routes;
pub mod state;

use std::sync::Arc;
use axum::Router;
use miniagent_agent::Agent;
use miniagent_checkpoint::CheckpointStore;
use miniagent_core::settings::AppConfig;
use miniagent_memory::manager::MemoryManager;
use state::TaskInfo;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub use state::AppState;

pub struct ServerConfig {
    pub config: Arc<AppConfig>,
    pub agent: Arc<Agent>,
    pub memory: Option<MemoryManager>,
    pub checkpoint_store: Option<CheckpointStore>,
}

impl ServerConfig {
    pub fn host(&self) -> &str {
        &self.config.server_host
    }

    pub fn port(&self) -> u16 {
        self.config.server_port
    }
}

pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.host(), config.port());

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = AppState::new(config.agent, config.config.clone())
        .with_limits(config.config.max_iterations, config.config.max_tokens);

    let state = if let Some(mem) = config.memory {
        state.with_memory(mem)
    } else {
        state
    };

    // Ensure result directory exists
    let _ = std::fs::create_dir_all(&state.task_dir);
    let _ = std::fs::create_dir_all(state.task_dir.join("_uploads"));

    // Restore tasks from existing result directories
    restore_tasks_from_disk(&state);

    let restored = state.tasks.len();

    let app = Router::new()
        .merge(routes::create_router(state))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    tracing::info!("Server starting on {addr}");
    println!("Miniagent Server running on http://{addr}");
    println!("   Open http://{addr} in your browser");
    if restored > 0 {
        println!("   Restored {restored} tasks from disk");
    }
    println!();
    println!("   WebSocket  /ws/chat                  — Streaming chat");
    println!("   Upload     POST /api/upload           — Upload files");
    println!("   Download   GET  /api/download/{{id}}/{{file}}");
    println!("   Tasks      GET  /api/tasks");
    println!("   Legacy     POST /api/run              — REST agent");
    println!("   Health     GET  /api/health");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Scan result/{id}_{brief}/ directories and restore TaskInfo entries.
fn restore_tasks_from_disk(state: &AppState) {
    let Ok(entries) = std::fs::read_dir(&state.task_dir) else {
        tracing::warn!(dir = %state.task_dir.display(), "Failed to read task directory during restore");
        return;
    };

    let mut restored = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "restore: skipping directory entry");
                continue;
            }
        };
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip non-task directories
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }

        // Parse "{id}_{brief}" from directory name
        let (task_id, brief) = match name.find('_') {
            Some(pos) => (&name[..pos], name[pos + 1..].to_string()),
            None => continue,
        };

        // Find the result file: prefer {brief}.md, fall back to output.md (legacy)
        let dir = entry.path();
        let brief_filename = format!("{}.md", brief);
        let response = std::fs::read_to_string(dir.join(&brief_filename))
            .or_else(|_| std::fs::read_to_string(dir.join("output.md")))
            .unwrap_or_default();

        // Determine status
        let status = if !response.is_empty() {
            "completed".into()
        } else {
            "unknown".into()
        };

        // Collect result files
        let mut files = vec![];
        if let Ok(dir_entries) = std::fs::read_dir(&dir) {
            for de in dir_entries {
                let de = match de {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(error = %e, dir = %dir.display(), "restore: skipping file entry");
                        continue;
                    }
                };
                if let Some(fname) = de.file_name().to_str()
                    && (fname.ends_with(".md") || fname.ends_with(".json")) {
                        files.push(fname.to_string());
                    }
            }
        }

        let info = TaskInfo {
            id: task_id.to_string(),
            brief,
            prompt: String::new(),
            status,
            created_at: String::new(),
            result_dir: dir,
            files,
            response: response.clone(),
            messages: vec![
                serde_json::json!({"role": "user", "content": ""}),
                serde_json::json!({"role": "assistant", "content": response}),
            ],
            plan: None,
            stage_outputs: Vec::new(),
            event_log: Vec::new(),
        };

        // Restore plan, stage_outputs, and messages from metadata.json if present
        let info = if let Ok(metadata_str) = std::fs::read_to_string(info.result_dir.join("metadata.json")) {
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&metadata_str) {
                let mut info_mut = info;
                if let Some(plan) = metadata.get("plan") {
                    info_mut.plan = Some(plan.clone());
                }
                if let Some(outputs) = metadata.get("stage_outputs").and_then(|v| v.as_array()) {
                    info_mut.stage_outputs = outputs.clone();
                }
                if let Some(messages) = metadata.get("messages").and_then(|v| v.as_array()) {
                    info_mut.messages = messages.clone();
                }
                info_mut
            } else {
                info
            }
        } else {
            info
        };

        state.tasks.insert(task_id.to_string(), info);
        restored += 1;
    }

    if restored > 0 {
        tracing::info!(count = restored, "Restored tasks from disk");
    }
}
