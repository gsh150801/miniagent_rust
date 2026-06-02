use std::collections::HashMap;

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use miniagent_agent::context::RunContext;
use miniagent_core::config::TaskComplexity;
use miniagent_core::message::Message as AgentMessage;
use miniagent_provider::deepseek::DeepSeekFlash;
use miniagent_core::types::StageId;
use miniagent_workflow::builder::{WorkflowBuilder, WorkflowSpec};
use miniagent_workflow::stage::{StageContext, StageHandler as _};
use miniagent_workflow::stages::PlannerStage;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::state::{AppState, TaskInfo};

// ── Embedded HTML ──

static INDEX_HTML: &str = include_str!("static/index.html");

// ── Router ──

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/ws/chat", get(ws_upgrade_handler))
        .route("/api/upload", post(upload_handler))
        .route("/api/tasks", get(tasks_handler))
        .route("/api/download/{task_id}/{filename}", get(download_handler))
        .route("/api/tasks/{task_id}", delete(delete_task_handler))
        // Keep legacy routes
        .route("/api/health", get(health_handler))
        .route("/api/metrics", get(metrics_handler))
        .route("/api/run", post(run_handler))
        .route("/api/resume", post(resume_handler))
        .with_state(state)
}

// ── Legacy types ──

#[derive(Debug, Deserialize)]
struct RunRequest {
    prompt: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default = "default_provider")]
    provider: String,
    #[serde(default = "default_complexity")]
    complexity: String,
    #[serde(default)]
    history: Vec<AgentMessage>,
}

fn default_provider() -> String { "flash".into() }
fn default_complexity() -> String { "moderate".into() }

#[derive(Debug, Serialize)]
struct RunResponse {
    text: String,
    stop_reason: String,
    usage: UsageResponse,
    history: Vec<AgentMessage>,
}

#[derive(Debug, Serialize)]
struct UsageResponse {
    input_tokens: usize,
    output_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct ResumeRequest {
    checkpoint_id: String,
    prompt: String,
}

#[derive(Debug, Serialize)]
struct MetricsResponse {
    agent_runs: u64,
    agent_failures: u64,
    tool_calls: u64,
    tool_failures: u64,
    provider_calls: u64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    avg_latency_ms: f64,
    web_search_calls: u64,
    pubmed_calls: u64,
    fetch_calls: u64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

// ── Page handlers ──

async fn index_handler() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

async fn health_handler() -> &'static str {
    "OK"
}

async fn metrics_handler() -> Json<MetricsResponse> {
    let m = miniagent_telemetry::metrics::snapshot();
    Json(MetricsResponse {
        agent_runs: m.agent_runs,
        agent_failures: m.agent_failures,
        tool_calls: m.tool_calls,
        tool_failures: m.tool_failures,
        provider_calls: m.provider_calls,
        total_input_tokens: m.total_input_tokens,
        total_output_tokens: m.total_output_tokens,
        avg_latency_ms: m.avg_latency_ms,
        web_search_calls: m.web_search_calls,
        pubmed_calls: m.pubmed_calls,
        fetch_calls: m.fetch_calls,
    })
}

// ── Tasks API ──

async fn tasks_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for entry in state.tasks.iter() {
        map.insert(entry.key().clone(), serde_json::to_value(entry.value()).unwrap_or_default());
    }
    Json(serde_json::Value::Object(map))
}

async fn delete_task_handler(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> StatusCode {
    if state.tasks.remove(&task_id).is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── Download ──

async fn download_handler(
    State(state): State<AppState>,
    Path((task_id, filename)): Path<(String, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let task = state.tasks.get(&task_id).ok_or(StatusCode::NOT_FOUND)?;
    let path = task.result_dir.join(&filename);

    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let data = std::fs::read(&path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let body = axum::body::Body::from(data);
    let disposition = format!("attachment; filename=\"{}\"", filename);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".into()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    ))
}

// ── Upload ──

#[derive(Debug, Serialize)]
struct UploadResponse {
    files: Vec<FileInfo>,
}

#[derive(Debug, Serialize, Clone)]
struct FileInfo {
    id: String,
    name: String,
    size: usize,
}

async fn upload_handler(
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<UploadResponse>, (StatusCode, Json<ErrorResponse>)> {
    let upload_dir = state.task_dir.join("_uploads");
    let _ = std::fs::create_dir_all(&upload_dir);

    let mut files = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: format!("Multipart error: {e}") }),
        )
    })? {
        let name = field.file_name().unwrap_or("file").to_string();
        let data = field.bytes().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse { error: format!("Read error: {e}") }),
            )
        })?;

        let id = Uuid::new_v4().to_string()[..8].to_string();
        let path = upload_dir.join(&id);

        std::fs::write(&path, &data).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: format!("Write error: {e}") }),
            )
        })?;

        // Also save with original name for reference
        let _ = std::fs::write(upload_dir.join(format!("{id}_{}", name)), &data);

        files.push(FileInfo {
            id: id.clone(),
            name,
            size: data.len(),
        });
    }

    Ok(Json(UploadResponse { files }))
}

// ── WebSocket ──

async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Text(text) = msg {
            if let Ok(req) = serde_json::from_str::<WsRequest>(&text) {
                match req.r#type.as_str() {
                    "run" => {
                        handle_run(&mut socket, &state, req.prompt, req.files).await;
                    }
                    "list_tasks" => {
                        let mut tasks = serde_json::Map::new();
                        for entry in state.tasks.iter() {
                            tasks.insert(
                                entry.key().clone(),
                                serde_json::to_value(entry.value()).unwrap_or_default(),
                            );
                        }
                        let _ = ws_send(&mut socket, serde_json::json!({
                            "type": "tasks",
                            "tasks": tasks,
                        }))
                        .await;
                    }
                    _ => {}
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct WsRequest {
    r#type: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    files: Vec<String>,
}

async fn handle_run(
    socket: &mut WebSocket,
    state: &AppState,
    prompt: String,
    file_ids: Vec<String>,
) {
    let api_key = &state.api_key;
    let agent_arc = state.agent.clone();
    let max_iterations = state.max_iterations;
    let max_tokens = state.max_tokens;

    // Generate task id and directory
    let task_id = Uuid::new_v4().to_string()[..8].to_string();
    let task_brief = sanitize_task_brief(&prompt);
    let task_dir_name = format!("{}_{}", task_id, task_brief);
    let task_dir = state.task_dir.join(&task_dir_name);
    let task_workflow_dir = task_dir.join(".workflow");
    let _ = std::fs::create_dir_all(&task_workflow_dir);

    // Read uploaded files and append content to prompt
    let mut enriched_prompt = prompt.clone();
    if !file_ids.is_empty() {
        let upload_dir = state.task_dir.join("_uploads");
        for fid in &file_ids {
            let path = upload_dir.join(fid);
            if let Ok(content) = std::fs::read_to_string(&path) {
                enriched_prompt.push_str(&format!("\n\n--- Attached file ({fid}) ---\n{content}\n--- End file ---"));
            }
        }
    }

    // Register task
    let task_info = TaskInfo {
        id: task_id.clone(),
        brief: task_brief.clone(),
        prompt: prompt.clone(),
        status: "running".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        result_dir: task_dir.clone(),
        files: vec![],
    };
    state.tasks.insert(task_id.clone(), task_info);

    // Send status
    let _ = ws_send(socket, serde_json::json!({
        "type": "status",
        "message": "Planning workflow...",
    }))
    .await;

    // Plan via LLM
    let planner = PlannerStage::new(Box::new(DeepSeekFlash::new(api_key)));
    let plan_ctx = StageContext {
        stage_id: StageId::new(),
        input: serde_json::json!({ "prompt": enriched_prompt }),
        previous_outputs: HashMap::new(),
    };

    let plan_output = planner.execute(&plan_ctx).await.unwrap_or_else(|e| {
        let _ = ws_send(socket, serde_json::json!({
            "type": "status",
            "message": format!("Planner fallback: {e}"),
        }));
        miniagent_workflow::stage::StageOutput {
            data: serde_json::json!({ "workflow_spec": WorkflowSpec::single_agent() }),
            metadata: miniagent_workflow::stage::StageMetadata {
                duration_ms: 0,
                items_processed: 0,
                success: true,
                error: None,
            },
        }
    });

    let spec: WorkflowSpec = serde_json::from_value(plan_output.data["workflow_spec"].clone())
        .unwrap_or_else(|_| WorkflowSpec::single_agent());

    // Send plan info
    let _ = ws_send(socket, serde_json::json!({
        "type": "plan",
        "workflow": spec.task_type,
        "stages": spec.stages.iter().map(|s| {
            serde_json::json!({"name": s.name, "handler": s.handler_type, "tier": s.model_tier})
        }).collect::<Vec<_>>(),
    }))
    .await;

    // Build workflow
    let builder = WorkflowBuilder::new(agent_arc.clone(), api_key)
        .with_limits(max_iterations, max_tokens)
        .with_task_dir(task_workflow_dir.to_string_lossy());

    let system_prompt = "You are an AI agent with direct access to system tools. You MUST use tools for actions — NEVER simulate or describe tool output.\n\
         Available tools: pubmed_search, web_search, web_fetch, read, write, edit, glob, grep, bash.".to_string();

    let workflow = builder.build(&spec, &enriched_prompt, &system_prompt).unwrap_or_else(|e| {
        let _ = ws_send(socket, serde_json::json!({
            "type": "status",
            "message": format!("Build fallback: {e}"),
        }));
        let fallback = WorkflowSpec::single_agent();
        WorkflowBuilder::new(agent_arc.clone(), api_key)
            .with_limits(max_iterations, max_tokens)
            .with_task_dir(task_workflow_dir.to_string_lossy())
            .build(&fallback, &enriched_prompt, &system_prompt)
            .expect("single-agent fallback should always build")
    });

    let cancel = CancellationToken::new();

    // Send progress for each stage
    let stage_names: Vec<String> = spec.stages.iter().map(|s| s.name.clone()).collect();
    for name in &stage_names {
        let _ = ws_send(socket, serde_json::json!({
            "type": "progress",
            "stage": name,
            "status": "running",
        }))
        .await;
    }

    // Execute workflow
    match workflow.run(None, cancel).await {
        Ok(result) => {
            // Mark all stages completed
            for name in &stage_names {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": "completed",
                }))
                .await;
            }

            // Collect response text
            let mut response_text = String::new();

            for output in result.stage_outputs.values() {
                let data = &output.data;

                if let Some(text) = data["response"].as_str() {
                    if !text.is_empty() {
                        response_text = text.to_string();
                    }
                }
            }

            // Try reading synthesis from disk if response_text is empty
            if response_text.is_empty() {
                let synth_path = task_workflow_dir.join("synthesis.md");
                if let Ok(content) = std::fs::read_to_string(&synth_path) {
                    response_text = content;
                }
            }

            // Stream the response
            if !response_text.is_empty() {
                // Send in chunks for streaming effect
                let chars: Vec<char> = response_text.chars().collect();
                let chunk_size = 20usize.max(chars.len() / 50);
                for chunk in chars.chunks(chunk_size) {
                    let text: String = chunk.iter().collect();
                    let _ = ws_send(socket, serde_json::json!({
                        "type": "stream",
                        "text": text,
                    }))
                    .await;
                    // Small delay for streaming feel (non-blocking)
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }

            // Save output.md
            let mut result_files = vec![];
            if !response_text.is_empty() {
                let output_path = task_dir.join("output.md");
                if std::fs::write(&output_path, &response_text).is_ok() {
                    result_files.push("output.md".into());
                }
            }

            // List any workflow artifacts
            if let Ok(entries) = std::fs::read_dir(&task_workflow_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.ends_with(".md") || name.ends_with(".json") {
                            result_files.push(name.to_string());
                        }
                    }
                }
            }

            // Update task
            if let Some(mut task) = state.tasks.get_mut(&task_id) {
                task.status = "completed".into();
                task.files = result_files.clone();
            }

            // Send completion
            let _ = ws_send(socket, serde_json::json!({
                "type": "complete",
                "task_id": task_id,
                "files": result_files,
            }))
            .await;
        }
        Err(e) => {
            // Mark stages as failed
            for name in &stage_names {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": "failed",
                }))
                .await;
            }

            if let Some(mut task) = state.tasks.get_mut(&task_id) {
                task.status = "failed".into();
            }

            let _ = ws_send(socket, serde_json::json!({
                "type": "error",
                "message": format!("{e}"),
            }))
            .await;
        }
    }
}

async fn ws_send(socket: &mut WebSocket, msg: serde_json::Value) -> Result<(), ()> {
    let text = serde_json::to_string(&msg).map_err(|_| ())?;
    socket.send(Message::Text(text.into())).await.map_err(|_| ())
}

// ── Legacy handlers ──

async fn run_handler(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Result<Json<RunResponse>, (StatusCode, Json<ErrorResponse>)> {
    let span = miniagent_telemetry::AgentSpan::start(
        miniagent_core::types::RunId::new(),
        &req.provider,
        &req.complexity,
    );

    let complexity = parse_complexity(&req.complexity);
    let system_prompt = req
        .system
        .unwrap_or_else(|| "You are a helpful AI research assistant.".into());

    let context = RunContext::new(system_prompt)
        .with_complexity(complexity)
        .with_provider(parse_provider(&req.provider));

    let mut history = req.history;
    history.push(AgentMessage::user(&req.prompt));

    let cancel = CancellationToken::new();

    let result = state.agent.run(&history, &context, cancel).await;

    match result {
        Ok(delta) => {
            let text = delta
                .new_messages
                .iter()
                .map(|m| m.text_content())
                .collect::<Vec<_>>()
                .join("\n");

            history.extend(delta.new_messages.clone());

            let _result = span.finish(&delta.usage, None);

            Ok(Json(RunResponse {
                text,
                stop_reason: format!("{:?}", delta.stop_reason),
                usage: UsageResponse {
                    input_tokens: delta.usage.input_tokens,
                    output_tokens: delta.usage.output_tokens,
                },
                history,
            }))
        }
        Err(e) => {
            let error_msg = format!("{e}");
            let _ = span.finish(
                &miniagent_core::event::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
                Some(&error_msg),
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: error_msg }),
            ))
        }
    }
}

async fn resume_handler(
    State(state): State<AppState>,
    Json(req): Json<ResumeRequest>,
) -> Result<Json<RunResponse>, (StatusCode, Json<ErrorResponse>)> {
    let checkpoint_id = miniagent_core::types::CheckpointId(
        Uuid::parse_str(&req.checkpoint_id).unwrap_or_default(),
    );

    let checkpoint = match &state.checkpoint_store {
        Some(store) => store
            .load(&checkpoint_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: format!("{e}") })))?,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse { error: "No checkpoint store configured".into() }),
            ));
        }
    };

    match checkpoint {
        Some(ckpt) => {
            let mut history = ckpt.history;
            history.push(AgentMessage::user(&req.prompt));

            let cancel = CancellationToken::new();
            let context = RunContext::default();

            match state.agent.run(&history, &context, cancel).await {
                Ok(delta) => {
                    history.extend(delta.new_messages.clone());
                    let text = delta
                        .new_messages
                        .iter()
                        .map(|m| m.text_content())
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Json(RunResponse {
                        text,
                        stop_reason: format!("{:?}", delta.stop_reason),
                        usage: UsageResponse {
                            input_tokens: delta.usage.input_tokens,
                            output_tokens: delta.usage.output_tokens,
                        },
                        history,
                    }))
                }
                Err(e) => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse { error: format!("{e}") }),
                )),
            }
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { error: format!("Checkpoint {checkpoint_id:?} not found") }),
        )),
    }
}

// ── Helpers ──

fn parse_complexity(s: &str) -> TaskComplexity {
    match s {
        "simple" => TaskComplexity::Simple,
        "complex" => TaskComplexity::Complex,
        "deep" | "deep-research" => TaskComplexity::DeepResearch,
        _ => TaskComplexity::Moderate,
    }
}

fn parse_provider(s: &str) -> miniagent_provider::router::ProviderChoice {
    match s {
        "flash" => miniagent_provider::router::ProviderChoice::Flash,
        "pro" => miniagent_provider::router::ProviderChoice::Pro,
        _ => miniagent_provider::router::ProviderChoice::Auto,
    }
}

fn sanitize_task_brief(prompt: &str) -> String {
    let brief: String = prompt
        .chars()
        .take(30)
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let brief = brief.trim_end_matches('_');
    if brief.is_empty() {
        "task".into()
    } else {
        brief.into()
    }
}
