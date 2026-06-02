pub mod routes;
pub mod state;

use axum::Router;
use miniagent_agent::Agent;
use miniagent_checkpoint::CheckpointStore;
use miniagent_memory::manager::MemoryManager;
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

    let app = Router::new()
        .merge(routes::create_router(state))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    tracing::info!("Server starting on {addr}");
    println!("Miniagent Server running on http://{addr}");
    println!("   Open http://{addr} in your browser");
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
