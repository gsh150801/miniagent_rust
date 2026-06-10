pub mod routes;
pub mod state;

use axum::Router;
use miniagent_agent::Agent;
use miniagent_checkpoint::CheckpointStore;
use miniagent_memory::manager::MemoryManager;
use state::TaskInfo;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub use state::AppState;

pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub agent: Agent,
    pub memory: Option<MemoryManager>,
    pub checkpoint_store: Option<CheckpointStore>,
    pub api_key: String,
    pub max_iterations: usize,
    pub max_tokens: u32,
}

pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.host, config.port);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = AppState::new(config.agent, config.api_key)
        .with_limits(config.max_iterations, config.max_tokens);

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
    let Ok(entries) = std::fs::read_dir(&state.task_dir) else { return };

    for entry in entries.flatten() {
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
            for de in dir_entries.flatten() {
                if let Some(fname) = de.file_name().to_str() {
                    if fname.ends_with(".md") || fname.ends_with(".json") {
                        files.push(fname.to_string());
                    }
                }
            }
        }

        let info = TaskInfo {
            id: task_id.to_string(),
            brief,
            prompt: String::new(), // not available from disk
            status,
            created_at: String::new(), // not available from disk
            result_dir: dir,
            files,
            response,
            plan: None,
            stage_outputs: Vec::new(),
        };

        state.tasks.insert(task_id.to_string(), info);
    }
}
