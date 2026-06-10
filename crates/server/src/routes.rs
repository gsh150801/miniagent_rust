use std::collections::HashMap;
use std::path::Path as StdPath;

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use miniagent_agent::context::RunContext;
use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::message::Message as AgentMessage;
use miniagent_provider::deepseek::DeepSeekFlash;
use miniagent_provider::traits::{CompletionRequest, LlmProvider, StreamChunk};
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
        .route("/api/tasks/{task_id}", get(get_task_handler).delete(delete_task_handler))
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

async fn get_task_handler(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskInfo>, StatusCode> {
    let task = state.tasks.get(&task_id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(task.value().clone()))
}

async fn delete_task_handler(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> StatusCode {
    if let Some((_, info)) = state.tasks.remove(&task_id) {
        let _ = std::fs::remove_dir_all(&info.result_dir);
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

        // Save original filename as metadata
        let meta_path = upload_dir.join(format!("{id}.meta"));
        let _ = std::fs::write(&meta_path, &name);

        // Save raw bytes
        std::fs::write(upload_dir.join(&id), &data).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: format!("Write error: {e}") }),
            )
        })?;

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
                    "get_task" => {
                        if let Some(task) = state.tasks.get(&req.task_id) {
                            let _ = ws_send(&mut socket, serde_json::json!({
                                "type": "task_messages",
                                "task_id": req.task_id,
                                "prompt": task.prompt,
                                "response": task.response,
                                "status": task.status,
                                "files": task.files.clone(),
                                "plan": task.plan,
                                "stage_outputs": task.stage_outputs.clone(),
                            }))
                            .await;
                        }
                    }
                    "list_tasks" => {
                        let mut tasks = serde_json::Map::new();
                        for entry in state.tasks.iter() {
                            // Send summary only (no response text) to keep message small
                            tasks.insert(
                                entry.key().clone(),
                                serde_json::json!({
                                    "id": entry.value().id,
                                    "brief": entry.value().brief,
                                    "prompt": entry.value().prompt,
                                    "status": entry.value().status,
                                    "created_at": entry.value().created_at,
                                    "files": entry.value().files,
                                }),
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
    #[serde(default)]
    task_id: String,
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
    // Clean task-specific workflow dir to prevent stale data from previous runs
    let _ = std::fs::remove_dir_all(&task_workflow_dir);
    let _ = std::fs::create_dir_all(&task_workflow_dir);
    // Also clean the shared workflow dir (legacy fallback) to prevent cross-task contamination
    let shared_wf = state.task_dir.join(".workflow");
    let _ = std::fs::remove_dir_all(&shared_wf);
    let _ = std::fs::create_dir_all(&shared_wf);

    // Read and parse uploaded files
    let enriched_prompt = enrich_prompt_with_files(&prompt, &file_ids, &state.task_dir);

    // Register task
    let task_info = TaskInfo {
        id: task_id.clone(),
        brief: task_brief.clone(),
        prompt: prompt.clone(),
        status: "running".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        result_dir: task_dir.clone(),
        files: vec![],
        response: String::new(),
        plan: None,
        stage_outputs: Vec::new(),
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
    let plan_data = serde_json::json!({
        "workflow": spec.task_type,
        "stages": spec.stages.iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "handler": s.handler_type,
                "tier": s.model_tier,
                "description": s.description,
                "sub_tasks": s.sub_tasks,
                "tools": s.tools,
            })
        }).collect::<Vec<_>>(),
        "edges": spec.edges,
    });
    let _ = ws_send(socket, serde_json::json!({
        "type": "plan",
        "workflow": spec.task_type,
        "stages": plan_data["stages"],
    }))
    .await;

    // Persist plan for history replay
    if let Some(mut task) = state.tasks.get_mut(&task_id) {
        task.plan = Some(plan_data);
    }

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

    // Determine if last stage is a pure-LLM stage that can be streamed
    let last_stage = spec.stages.last();
    let stream_last = last_stage.map_or(false, |s|
        matches!(s.handler_type.as_str(), "synthesizer" | "critic" | "llm")
    );

    if stream_last && spec.stages.len() > 1 {
        // Multi-stage: run all stages except the last with progress,
        // then stream the final synthesis stage directly.
        run_multi_stage_with_streaming(
            socket, workflow, &spec, &task_workflow_dir, &api_key,
            &task_id, &task_brief, &task_dir, state, cancel,
        ).await;
    } else {
        // Single-stage or agent-only last stage: run with progress callback
        run_with_progress(socket, workflow, &spec, &task_workflow_dir,
            &task_id, &task_brief, &task_dir, state, cancel,
        ).await;
    }
}

/// Run workflow with per-stage progress (non-streaming response).
async fn run_with_progress(
    socket: &mut WebSocket,
    workflow: miniagent_workflow::Workflow,
    spec: &WorkflowSpec,
    task_workflow_dir: &StdPath,
    task_id: &str,
    task_brief: &str,
    task_dir: &StdPath,
    state: &AppState,
    cancel: CancellationToken,
) {
    let stage_names: Vec<String> = spec.stages.iter().map(|s| s.name.clone()).collect();

    // Channel for progress updates + final result from workflow
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<ProgressMsg>(32);

    // Spawn workflow execution
    let wf_cancel = cancel.clone();
    tokio::spawn(async move {
        let result = workflow.run_with_progress(None, wf_cancel, |name, status, data| {
            let _ = progress_tx.try_send(ProgressMsg::Stage {
                name: name.to_string(),
                status: status.to_string(),
                data: data.cloned(),
            });
        }).await;
        let _ = progress_tx.send(ProgressMsg::Done(result)).await;
    });

    // Forward progress to WebSocket and wait for result
    let mut final_result: Option<Result<miniagent_workflow::engine::WorkflowResult, miniagent_core::error::AgentError>> = None;
    while let Some(msg) = progress_rx.recv().await {
        match msg {
            ProgressMsg::Stage { name, status, data } => {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": status,
                })).await;
                // Send detailed stage output when stage completes
                if status == "completed" {
                    if let Some(ref d) = data {
                        let summary = extract_stage_summary(&name, d);
                        // Persist stage output for history replay
                        if let Some(mut task) = state.tasks.get_mut(task_id) {
                            task.stage_outputs.push(serde_json::json!({
                                "stage": name,
                                "summary": summary,
                            }));
                        }
                        let _ = ws_send(socket, serde_json::json!({
                            "type": "stage_output",
                            "stage": name,
                            "summary": summary,
                        })).await;
                    }
                }
            }
            ProgressMsg::Done(result) => {
                final_result = Some(result);
                break;
            }
        }
    }

    match final_result.unwrap_or(Err(miniagent_core::error::AgentError::internal("workflow task panicked"))) {
        Ok(result) => {
            // Collect response text
            let response_text = collect_response_text(&result.stage_outputs, task_workflow_dir);

            // Send response in one stream message
            if !response_text.is_empty() {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "stream",
                    "text": response_text,
                })).await;
            }

            finalize_task(socket, state, task_id, &task_brief, task_dir, task_workflow_dir, &stage_names, response_text).await;
        }
        Err(e) => {
            for name in &stage_names {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": "failed",
                })).await;
            }
            if let Some(mut task) = state.tasks.get_mut(task_id) {
                task.status = "failed".into();
            }
            let _ = ws_send(socket, serde_json::json!({
                "type": "error",
                "message": format!("{e}"),
            })).await;
        }
    }
}

/// Multi-stage: run all but last stage via workflow, then stream the final stage.
async fn run_multi_stage_with_streaming(
    socket: &mut WebSocket,
    workflow: miniagent_workflow::Workflow,
    spec: &WorkflowSpec,
    task_workflow_dir: &StdPath,
    api_key: &str,
    task_id: &str,
    task_brief: &str,
    task_dir: &StdPath,
    state: &AppState,
    cancel: CancellationToken,
) {
    let stage_names: Vec<String> = spec.stages.iter().map(|s| s.name.clone()).collect();

    // Run the full workflow with progress
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<ProgressMsg>(32);

    let wf_cancel = cancel.clone();
    tokio::spawn(async move {
        let result = workflow.run_with_progress(None, wf_cancel, |name, status, data| {
            let _ = progress_tx.try_send(ProgressMsg::Stage {
                name: name.to_string(),
                status: status.to_string(),
                data: data.cloned(),
            });
        }).await;
        let _ = progress_tx.send(ProgressMsg::Done(result)).await;
    });

    // Forward progress to WebSocket and wait for result
    let mut final_result: Option<Result<miniagent_workflow::engine::WorkflowResult, miniagent_core::error::AgentError>> = None;
    while let Some(msg) = progress_rx.recv().await {
        match msg {
            ProgressMsg::Stage { name, status, data } => {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": status,
                })).await;
                // Send detailed stage output when stage completes
                if status == "completed" {
                    if let Some(ref d) = data {
                        let summary = extract_stage_summary(&name, d);
                        // Persist stage output for history replay
                        if let Some(mut task) = state.tasks.get_mut(task_id) {
                            task.stage_outputs.push(serde_json::json!({
                                "stage": name,
                                "summary": summary,
                            }));
                        }
                        let _ = ws_send(socket, serde_json::json!({
                            "type": "stage_output",
                            "stage": name,
                            "summary": summary,
                        })).await;
                    }
                }
            }
            ProgressMsg::Done(result) => {
                final_result = Some(result);
                break;
            }
        }
    }

    match final_result.unwrap_or(Err(miniagent_core::error::AgentError::internal("workflow task panicked"))) {
        Ok(_result) => {
            // Read the synthesis from disk and re-stream it via provider.stream()
            let response_text = std::fs::read_to_string(task_workflow_dir.join("synthesis.md"))
                .or_else(|_| std::fs::read_to_string(task_workflow_dir.join(format!("{}.md", stage_names.last().unwrap_or(&String::new())))))
                .unwrap_or_default();

            if !response_text.is_empty() {
                // Stream the synthesis text token by token via the pro model
                let stream_result = stream_synthesis(socket, api_key, &response_text, cancel).await;
                if !stream_result {
                    // Fallback: send as one chunk
                    let _ = ws_send(socket, serde_json::json!({
                        "type": "stream",
                        "text": response_text,
                    })).await;
                }
            } else {
                // Try collecting from stage outputs
                let fallback_text = collect_response_text(&_result.stage_outputs, task_workflow_dir);
                if !fallback_text.is_empty() {
                    let _ = ws_send(socket, serde_json::json!({
                        "type": "stream",
                        "text": fallback_text,
                    })).await;
                }
            }

            let final_text = if !response_text.is_empty() { response_text }
                else { collect_response_text(&_result.stage_outputs, task_workflow_dir) };

            finalize_task(socket, state, task_id, &task_brief, task_dir, task_workflow_dir, &stage_names, final_text).await;
        }
        Err(e) => {
            for name in &stage_names {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": "failed",
                })).await;
            }
            if let Some(mut task) = state.tasks.get_mut(task_id) {
                task.status = "failed".into();
            }
            let _ = ws_send(socket, serde_json::json!({
                "type": "error",
                "message": format!("{e}"),
            })).await;
        }
    }
}

/// Filter `<thinking>...</thinking>` tags from streamed text.
/// DeepSeek Pro emits reasoning as `<thinking>` wrapped TextDelta chunks.
fn filter_thinking_tags(text: &str, in_thinking: &mut bool, buf: &mut String) -> String {
    let mut result = String::new();
    buf.push_str(text);

    let input = buf.clone();
    let mut chars = input.as_str();
    buf.clear();

    while !chars.is_empty() {
        if *in_thinking {
            if let Some(pos) = chars.find("</thinking>") {
                *in_thinking = false;
                chars = &chars[pos + "</thinking>".len()..];
            } else {
                // Still inside thinking, buffer remainder
                buf.push_str(chars);
                break;
            }
        } else if let Some(pos) = chars.find("<thinking>") {
            // Output everything before the tag
            if pos > 0 {
                result.push_str(&chars[..pos]);
            }
            *in_thinking = true;
            chars = &chars[pos + "<thinking>".len()..];
        } else {
            // No tag found — but check if a partial tag is at the end
            if let Some(last_lt) = chars.rfind('<') {
                let tail = &chars[last_lt..];
                if "<thinking>".starts_with(tail) {
                    // Partial tag at end — buffer it
                    result.push_str(&chars[..last_lt]);
                    buf.push_str(tail);
                    break;
                }
            }
            result.push_str(chars);
            break;
        }
    }

    result
}

/// Re-stream synthesis text through provider.stream() for real token-by-token output.
async fn stream_synthesis(
    socket: &mut WebSocket,
    api_key: &str,
    synthesis_text: &str,
    cancel: CancellationToken,
) -> bool {
    let pro = miniagent_provider::deepseek::DeepSeekPro::new(api_key);
    let request = CompletionRequest {
        system: "You are presenting final research output. Output the following text faithfully, maintaining all structure and content. Do not add or remove information.".into(),
        messages: vec![AgentMessage::user(format!("Present this output:\n\n{synthesis_text}"))],
        tools: vec![],
        config: InferenceConfig {
            max_tokens: Some(16_000),
            ..Default::default()
        },
    };

    let stream = match pro.stream(&request, cancel).await {
        Ok(s) => s.content_receiver,
        Err(_) => return false,
    };

    let mut receiver = stream;
    let mut got_text = false;
    // Track <thinking> state to filter out DeepSeek Pro reasoning tokens
    let mut in_thinking = false;
    let mut think_buf = String::new();

    while let Some(chunk) = receiver.recv().await {
        match chunk {
            Ok(StreamChunk::TextDelta { text }) => {
                // Filter <thinking>...</thinking> blocks from DeepSeek Pro reasoning
                let filtered = filter_thinking_tags(&text, &mut in_thinking, &mut think_buf);
                if !filtered.is_empty() {
                    got_text = true;
                    let _ = ws_send(socket, serde_json::json!({
                        "type": "stream",
                        "text": filtered,
                    })).await;
                }
            }
            Ok(StreamChunk::Stop(_)) => break,
            Ok(StreamChunk::Error(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }
    got_text
}

/// Collect response text from stage outputs.
fn collect_response_text(
    stage_outputs: &HashMap<StageId, miniagent_workflow::stage::StageOutput>,
    task_workflow_dir: &StdPath,
) -> String {
    let mut response_text = String::new();

    for output in stage_outputs.values() {
        if let Some(text) = output.data["response"].as_str() {
            if !text.is_empty() {
                response_text = text.to_string();
            }
        }
    }

    if response_text.is_empty() {
        let synth_path = task_workflow_dir.join("synthesis.md");
        if let Ok(content) = std::fs::read_to_string(&synth_path) {
            response_text = content;
        }
    }

    response_text
}

/// Finalize task: save output, update state, send completion.
async fn finalize_task(
    socket: &mut WebSocket,
    state: &AppState,
    task_id: &str,
    task_brief: &str,
    task_dir: &StdPath,
    task_workflow_dir: &StdPath,
    _stage_names: &[String],
    response_text: String,
) {
    // Save result file with content-based name: {brief}.md
    let output_filename = format!("{}.md", task_brief);
    let mut result_files = vec![];
    if !response_text.is_empty() {
        let output_path = task_dir.join(&output_filename);
        if std::fs::write(&output_path, &response_text).is_ok() {
            result_files.push(output_filename.clone());
        }
    }

    // List workflow artifacts
    if let Ok(entries) = std::fs::read_dir(task_workflow_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".md") || name.ends_with(".json") {
                    result_files.push(name.to_string());
                }
            }
        }
    }

    // Update task
    if let Some(mut task) = state.tasks.get_mut(task_id) {
        task.status = "completed".into();
        task.files = result_files.clone();
        task.response.clone_from(&response_text);
    }

    // Send completion
    let _ = ws_send(socket, serde_json::json!({
        "type": "complete",
        "task_id": task_id,
        "files": result_files,
    }))
    .await;
}

// ── Progress message types ──

enum ProgressMsg {
    Stage {
        name: String,
        status: String,
        data: Option<serde_json::Value>,
    },
    Done(Result<miniagent_workflow::engine::WorkflowResult, miniagent_core::error::AgentError>),
}

/// Extract human-readable info from stage output data for frontend display.
fn extract_stage_summary(name: &str, data: &serde_json::Value) -> serde_json::Value {
    let mut summary = serde_json::json!({ "stage": name });

    // Structured tool entries (from agent stages)
    if let Some(entries) = data["tool_entries"].as_array() {
        if !entries.is_empty() {
            summary["tool_count"] = serde_json::json!(entries.len());
            summary["tool_entries"] = serde_json::json!(entries);
        }
    } else if let Some(tool_calls) = data["tool_calls"].as_u64() {
        // Fallback: legacy flat format
        summary["tool_count"] = serde_json::json!(tool_calls);
        if let Some(results) = data["tool_results"].as_array() {
            let previews: Vec<serde_json::Value> = results.iter()
                .filter_map(|r| r.as_str())
                .take(5)
                .map(|r| {
                    let s = r.trim();
                    let preview: String = s.chars().take(200).collect();
                    let is_error = s.contains("Error:") || s.contains("error:");
                    serde_json::json!({ "name": "", "input_preview": "", "result_preview": preview, "is_error": is_error })
                })
                .collect();
            if !previews.is_empty() {
                summary["tool_entries"] = serde_json::json!(previews);
            }
        }
    }

    // Token usage
    if let Some(tokens_in) = data["tokens_in"].as_u64() {
        summary["tokens_in"] = serde_json::json!(tokens_in);
    }
    if let Some(tokens_out) = data["tokens_out"].as_u64() {
        summary["tokens_out"] = serde_json::json!(tokens_out);
    }

    // Response preview
    if let Some(response) = data["response"].as_str() {
        if !response.is_empty() {
            let preview: String = response.chars().take(300).collect();
            summary["response_preview"] = serde_json::json!(preview);
        }
    }

    // Critique/review content
    if let Some(critique) = data["critique"].as_str() {
        let preview: String = critique.chars().take(300).collect();
        summary["critique_preview"] = serde_json::json!(preview);
    }

    summary
}

// ── File parsing ──

/// Read uploaded files, parse CSV/TSV into markdown tables, and append to prompt.
fn enrich_prompt_with_files(prompt: &str, file_ids: &[String], task_dir: &StdPath) -> String {
    if file_ids.is_empty() {
        return prompt.to_string();
    }

    let upload_dir = task_dir.join("_uploads");
    let mut enriched = prompt.to_string();

    for fid in file_ids {
        let path = upload_dir.join(fid);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Read original filename from metadata
        let filename = std::fs::read_to_string(upload_dir.join(format!("{fid}.meta")))
            .unwrap_or_else(|_| fid.clone());

        let parsed = parse_file_content(&content, &filename);
        enriched.push_str(&format!("\n\n--- Attached file: {} ---\n{}\n--- End file ---", filename, parsed));
    }

    enriched
}

/// Parse file content based on extension. CSV/TSV → markdown table; others → raw text.
fn parse_file_content(content: &str, filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "csv" => parse_delimited(content, ','),
        "tsv" => parse_delimited(content, '\t'),
        _ => content.to_string(),
    }
}

/// Parse delimited text (CSV/TSV) into a markdown table.
fn parse_delimited(content: &str, delimiter: char) -> String {
    let lines: Vec<&str> = content.lines().take(200).collect();
    if lines.is_empty() {
        return content.to_string();
    }

    let rows: Vec<Vec<String>> = lines.iter()
        .map(|l| parse_csv_line(l, delimiter))
        .collect();

    if rows.is_empty() || rows[0].is_empty() {
        return content.to_string();
    }

    let col_count = rows[0].len();
    let mut table = String::new();

    // Header row
    table.push_str("| ");
    table.push_str(&rows[0].join(" | "));
    table.push_str(" |\n");

    // Separator
    table.push_str("| ");
    for _ in 0..col_count {
        table.push_str("--- | ");
    }
    table.push('\n');

    // Data rows
    for row in rows.iter().skip(1) {
        // Pad or trim to match column count
        let padded: Vec<String> = (0..col_count)
            .map(|i| row.get(i).cloned().unwrap_or_default())
            .collect();
        table.push_str("| ");
        table.push_str(&padded.join(" | "));
        table.push_str(" |\n");
    }

    if lines.len() < content.lines().count() {
        table.push_str(&format!("\n*... {} more rows truncated*\n", content.lines().count() - lines.len()));
    }

    table
}

/// Parse a single CSV/TSV line, handling quoted fields.
fn parse_csv_line(line: &str, delimiter: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delimiter {
            fields.push(current.trim().to_string());
            current = String::new();
        } else {
            current.push(c);
        }
    }
    fields.push(current.trim().to_string());
    fields
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
