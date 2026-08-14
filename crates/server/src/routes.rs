use std::collections::HashMap;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use miniagent_agent::Agent;
use miniagent_agent::context::RunContext;
use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::message::Message as AgentMessage;
use miniagent_provider::deepseek::DeepSeekFlash;
use miniagent_provider::stepfun::StepFunFlash;
use miniagent_provider::minimax::MiniMaxFlash;
use miniagent_provider::traits::{CompletionRequest, LlmProvider, StreamChunk};
use miniagent_core::types::StageId;
use miniagent_workflow::builder::{WorkflowBuilder, WorkflowSpec, StageSpec};
use miniagent_workflow::stage::{StageContext, StageHandler as _};
use miniagent_workflow::stages::PlannerStage;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use futures_util::stream::{SplitSink, StreamExt};
use futures_util::SinkExt;
use tokio::sync::Mutex;

use crate::state::{AppState, TaskInfo};

type WsSink = SplitSink<WebSocket, Message>;

// ── Embedded static assets (inline, no CDN — project convention) ──

static INDEX_HTML: &str = include_str!("static/index.html");
static STYLES_CSS: &str = include_str!("static/styles.css");
static APP_JS: &str = include_str!("static/app.js");
static MARKED_JS: &str = include_str!("static/marked.min.js");

// ── Router ──

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/styles.css", get(styles_handler))
        .route("/app.js", get(app_js_handler))
        .route("/marked.min.js", get(marked_js_handler))
        .route("/ws/chat", get(ws_upgrade_handler))
        .route("/api/upload", post(upload_handler))
        .route("/api/cancel", post(cancel_handler))
        .route("/api/tasks", get(tasks_handler))
        // Catch-all path supports nested artifacts (.workflow/research_output.json, turn_*/...)
        .route("/api/download/{task_id}/{*path}", get(download_handler))
        .route("/api/tasks/{task_id}/files", get(files_handler))
        .route("/api/tasks/{task_id}/preview/{*path}", get(preview_handler))
        .route("/api/tasks/{task_id}", get(get_task_handler).delete(delete_task_handler))
        // Keep legacy routes
        .route("/api/health", get(health_handler))
        .route("/api/skills", get(skills_handler))
        .route("/api/trace/{task_id}", get(trace_handler))
        .route("/api/provenance/{task_id}", get(provenance_handler))
        .route("/api/metrics", get(metrics_handler))
        .route("/api/run", post(run_handler))
        .route("/api/resume", post(resume_handler))
        .with_state(state)
}

async fn styles_handler() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLES_CSS,
    )
}

async fn app_js_handler() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        APP_JS,
    )
}

async fn marked_js_handler() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        MARKED_JS,
    )
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

/// 返回已发现的 skill 列表（供前端浏览/搜索）。
///
/// 扫描 `skills/` 和 `.miniagent/skills/` 目录下的 SKILL.md 文件，
/// 返回 `[{name, description, triggers, tags, tools_needed, priority, actionable}]`。
/// 前端 `loadSkills()` fetch 此端点渲染 skill 面板。
async fn skills_handler() -> Json<Vec<serde_json::Value>> {
    use miniagent_skill::SkillDiscovery;

    let discovery = SkillDiscovery::new();
    let bundles = discovery.discover();

    let skills: Vec<serde_json::Value> = bundles.iter().map(|b| {
        serde_json::json!({
            "name": b.metadata.name,
            "description": b.metadata.description,
            "triggers": b.metadata.triggers,
            "tags": b.metadata.tags,
            "tools_needed": b.metadata.tools_needed,
            "priority": b.metadata.priority,
            "actionable": b.metadata.actionable,
            "version": b.metadata.version,
        })
    }).collect();

    Json(skills)
}

/// 返回指定 task 的完整事件轨迹（需求2: 全链路可追溯）。
///
/// 包含：工具调用（含完整 input/output/error）、skill 调用、子任务开始/结束等。
/// 每个 AgentEvent 都带时间戳。前端可用此端点查看历史 task 的完整执行轨迹。
async fn trace_handler(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Json<serde_json::Value> {
    let task = state.tasks.get(&task_id).map(|t| t.clone());

    match task {
        Some(task) => Json(serde_json::json!({
            "task_id": task.id,
            "brief": task.brief,
            "status": task.status,
            "created_at": task.created_at,
            "event_log": task.event_log,
            "stage_outputs": task.stage_outputs,
            "message_count": task.messages.len(),
        })),
        None => Json(serde_json::json!({
            "error": "task not found",
            "task_id": task_id,
        })),
    }
}

/// Return the provenance record (audit trail) for a data-analysis task.
///
/// Provenance is written by `miniagent-analysis` to
/// `analysis/<task_id>/provenance.json` (script hash, I/O hashes, conda env +
/// package versions, seed, git commit, exit code, stdout/stderr digests). This
/// endpoint makes the audit trail queryable so every analysis result is
/// reproducible and inspectable.
async fn provenance_handler(Path(task_id): Path<String>) -> Json<serde_json::Value> {
    // Sanitize the task id to prevent path traversal.
    let safe: String = task_id
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let dir = std::path::Path::new("analysis").join(&safe);
    let prov_path = dir.join("provenance.json");

    if !prov_path.exists() {
        return Json(serde_json::json!({
            "error": "provenance not found",
            "task_id": task_id,
            "expected_path": prov_path.display().to_string(),
        }));
    }

    let body = match std::fs::read_to_string(&prov_path) {
        Ok(b) => b,
        Err(e) => {
            return Json(serde_json::json!({
                "error": format!("failed to read provenance: {e}"),
                "task_id": task_id,
            }));
        }
    };
    let record: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            // Fall back to returning the raw text if it isn't valid JSON.
            serde_json::json!({ "raw": body })
        }
    };

    // List sibling artifacts (script + outputs) for convenience.
    let mut artifacts: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                artifacts.push(name.to_string());
            }
        }
    }

    Json(serde_json::json!({
        "task_id": task_id,
        "provenance": record,
        "artifacts": artifacts,
    }))
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

// ── Path safety: resolve `rel` under `base`, reject traversal escapes ──
//
// The download/preview routes use catch-all `{*path}` which can contain `../`.
// We strip any leading slashes (so absolute inputs can't re-root via join),
// then canonicalize both sides and verify the target stays inside the task's
// result directory. canonicalize() resolves `..` and symlinks, so this is the
// authoritative containment check — mirrors the intent of agent/hooks.rs:is_path_safe.

fn resolve_safe(base: &StdPath, rel: &str) -> Option<std::path::PathBuf> {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    let candidate = base.join(rel);
    let base_canon = base.canonicalize().ok()?;
    let cand_canon = candidate.canonicalize().ok()?;
    if cand_canon.starts_with(&base_canon) {
        Some(cand_canon)
    } else {
        None
    }
}

// ── Download ──

async fn download_handler(
    State(state): State<AppState>,
    Path((task_id, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, StatusCode> {
    let (result_dir, filename) = {
        let task = state.tasks.get(&task_id).ok_or(StatusCode::NOT_FOUND)?;
        (task.result_dir.clone(), task.brief.clone())
    };
    let resolved = resolve_safe(&result_dir, &path).ok_or(StatusCode::NOT_FOUND)?;
    if !resolved.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    let data = std::fs::read(&resolved).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let body = axum::body::Body::from(data);
    let disp_name = resolved
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(filename);
    let disposition = format!("attachment; filename=\"{}\"", disp_name);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".into()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    ))
}

// ── Task file tree ──

#[derive(Debug, Serialize)]
struct FileNode {
    path: String,
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<String>,
    ext: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<FileNode>,
}

const MAX_TREE_DEPTH: usize = 6;

fn build_file_tree(dir: &StdPath, base: &StdPath, depth: usize) -> Vec<FileNode> {
    if depth >= MAX_TREE_DEPTH {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes: Vec<FileNode> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Skip noise / hidden files
            if name.starts_with('.') && name != ".workflow" {
                return None;
            }
            let meta = e.metadata().ok()?;
            let rel = e
                .path()
                .strip_prefix(base)
                .ok()?
                .to_string_lossy()
                .to_string();
            let ext = std::path::Path::new(&name)
                .extension()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_default()
                });
            if meta.is_dir() {
                Some(FileNode {
                    path: rel,
                    name: name.clone(),
                    is_dir: true,
                    size: 0,
                    modified,
                    ext: String::new(),
                    children: build_file_tree(&e.path(), base, depth + 1),
                })
            } else {
                Some(FileNode {
                    path: rel,
                    name,
                    is_dir: false,
                    size: meta.len(),
                    modified,
                    ext,
                    children: Vec::new(),
                })
            }
        })
        .collect();
    // Directories first, then files; each group alphabetical.
    nodes.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    nodes
}

async fn files_handler(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<FileNode>>, StatusCode> {
    let result_dir = state
        .tasks
        .get(&task_id)
        .map(|t| t.result_dir.clone())
        .ok_or(StatusCode::NOT_FOUND)?;
    if !result_dir.is_dir() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(build_file_tree(&result_dir, &result_dir, 0)))
}

// ── Text preview ──

const PREVIEW_BYTES: usize = 200_000;
const TEXT_EXTS: &[&str] = &["md", "json", "txt", "csv", "tsv", "py", "rs", "js", "ts", "html", "css", "yaml", "yml", "toml", "sh", "log"];

async fn preview_handler(
    State(state): State<AppState>,
    Path((task_id, path)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let result_dir = state
        .tasks
        .get(&task_id)
        .map(|t| t.result_dir.clone())
        .ok_or(StatusCode::NOT_FOUND)?;
    let resolved = resolve_safe(&result_dir, &path).ok_or(StatusCode::NOT_FOUND)?;
    if !resolved.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    let meta = std::fs::metadata(&resolved).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let size = meta.len();
    let ext = resolved
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_text = TEXT_EXTS.iter().any(|e| *e == ext) || ext.is_empty();

    if !is_text {
        return Ok(Json(serde_json::json!({
            "preview": false,
            "size": size,
            "is_text": false,
        })));
    }

    let bytes = std::fs::read(&resolved).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = bytes.len();
    let truncated = total > PREVIEW_BYTES;
    let slice = if truncated { &bytes[..PREVIEW_BYTES] } else { &bytes };
    let mut content = String::from_utf8_lossy(slice).to_string();
    if truncated {
        content.push_str("\n\n…[truncated, download for full content]…");
    }
    Ok(Json(serde_json::json!({
        "preview": true,
        "is_text": true,
        "size": size,
        "truncated": truncated,
        "ext": ext,
        "content": content,
    })))
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

// ── Cancel API ──

#[derive(Debug, Deserialize)]
struct CancelRequest {
    task_id: String,
}

async fn cancel_handler(
    State(state): State<AppState>,
    Json(req): Json<CancelRequest>,
) -> impl IntoResponse {
    if req.task_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "task_id required"})));
    }
    match state.cancels.remove(&req.task_id) {
        Some((_, token)) => {
            token.cancel();
            (StatusCode::OK, Json(serde_json::json!({"status": "cancelled", "task_id": req.task_id})))
        }
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"status": "not_found", "task_id": req.task_id}))),
    }
}

// ── WebSocket ──

async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: AppState) {
    let (sink, mut stream) = socket.split();
    let sink = Arc::new(Mutex::new(sink));
    let mut running: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        let msg = match stream.next().await {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                tracing::warn!(error = ?e, "WebSocket stream error");
                break;
            }
            None => break,
        };

        if let Message::Text(text) = msg {
            let req: WsRequest = match serde_json::from_str(&text) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, raw = %text.chars().take(200).collect::<String>(), "ws: failed to parse client message");
                    continue;
                }
            };
            match req.r#type.as_str() {
                "run" => {
                    if let Some(handle) = running.take() {
                        handle.abort();
                        tokio::spawn(async move {
                            let _ = handle.await;
                        });
                    }

                    let state = state.clone();
                    let sink = Arc::clone(&sink);
                    let task_id = if req.task_id.is_empty() {
                        None
                    } else {
                        Some(req.task_id.clone())
                    };

                    let state2 = state.clone();
                    let sink2 = Arc::clone(&sink);
                    let handle = tokio::spawn(async move {
                        // 统一新流程：explore→ask→plan→dispatch→feedback（不再区分 workflow/loop）
                        let _ = handle_run(&sink2, &state2, req.prompt, req.files, task_id).await;
                    });
                    running = Some(handle);
                }
                "cancel" => {
                    if !req.task_id.is_empty()
                        && let Some((_, token)) = state.cancels.remove(&req.task_id) {
                            token.cancel();
                        }
                }
                "ask_reply"
                    // 双向 ws：前端回复 ask 问题，唤醒暂停的 task 执行
                    if !req.task_id.is_empty() => {
                        let answer = req.prompt.clone(); // 复用 prompt 字段传 answer
                        if let Some((_, sender)) = state.asks.remove(&req.task_id) {
                            let _ = sender.send(answer);
                        }
                    }
                "get_task" => {
                    if let Some(task) = state.tasks.get(&req.task_id) {
                        let file_tree = if task.result_dir.is_dir() {
                            build_file_tree(&task.result_dir, &task.result_dir, 0)
                        } else {
                            Vec::new()
                        };
                        let mut response = serde_json::json!({
                            "type": "task_messages",
                            "task_id": req.task_id,
                            "prompt": task.prompt,
                            "response": task.response,
                            "status": task.status,
                            "files": task.files.clone(),
                            "plan": task.plan,
                            "stage_outputs": task.stage_outputs.clone(),
                            "file_tree": file_tree,
                        });
                        if !task.messages.is_empty() {
                            response["messages"] = serde_json::json!(task.messages);
                        }
                        ws_send(&sink, response).await;
                    }
                }
                "list_tasks" => {
                    let mut tasks = serde_json::Map::new();
                    for entry in state.tasks.iter() {
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
                    ws_send(&sink, serde_json::json!({
                        "type": "tasks",
                        "tasks": tasks,
                    })).await;
                }
                _ => {}
            }
        }
    }

    if let Some(handle) = running {
        handle.abort();
        let _ = handle.await;
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WsRequest {
    r#type: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    skills: Vec<String>,
}

/// Loop pipeline: iterative Explore->Plan->Dispatch->Evaluate->Repair cycles.
#[allow(dead_code)]
async fn handle_run_loop(
    socket: &Arc<Mutex<WsSink>>,
    state: &AppState,
    prompt: String,
    existing_task_id: Option<String>,
) {
    let _api_key = state.config.require_active_key().unwrap_or_else(|e| {
        eprintln!("FATAL: {e}");
        std::process::exit(1);
    });
    let _agent_arc = state.agent.clone();

    let (task_id, task_brief, task_dir, _task_workflow_dir) =
        if let Some(ref existing_id) = existing_task_id {
            if let Some(ref task) = state.tasks.get(existing_id) {
                let brief = task.brief.clone();
                let dir = task.result_dir.clone();
                let wf_dir = dir.join(".workflow");
                let _ = std::fs::create_dir_all(&wf_dir);
                (existing_id.clone(), brief, dir.clone(), wf_dir)
            } else {
                let uuid_str = Uuid::new_v4().to_string();
                let brief = prompt.chars().take(32).collect::<String>();
                let task_dir = PathBuf::from(format!("./result/{}_{}", uuid_str, brief.replace("/", "_")));
                let _ = std::fs::create_dir_all(&task_dir);
                let wf_dir = task_dir.join(".workflow");
                let _ = std::fs::create_dir_all(&wf_dir);
                state.tasks.insert(uuid_str.clone(), TaskInfo {
                    id: uuid_str.clone(),
                    brief: brief.clone(),
                    prompt: prompt.clone(),
                    status: "running".into(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    result_dir: task_dir.clone(),
                    files: vec![],
                    response: String::new(),
                    messages: vec![serde_json::json!({"role": "user", "content": prompt.clone()})],
                    plan: None,
                    stage_outputs: Vec::new(),
            event_log: Vec::new(),
                });
                (uuid_str, brief, task_dir, wf_dir)
            }
        } else {
            let uuid_str = Uuid::new_v4().to_string();
            let brief = prompt.chars().take(32).collect::<String>();
            let task_dir = PathBuf::from(format!("./result/{}_{}", uuid_str, brief.replace("/", "_")));
            let _ = std::fs::create_dir_all(&task_dir);
            let wf_dir = task_dir.join(".workflow");
            let _ = std::fs::create_dir_all(&wf_dir);
            state.tasks.insert(uuid_str.clone(), TaskInfo {
                id: uuid_str.clone(),
                brief: brief.clone(),
                prompt: prompt.clone(),
                status: "running".into(),
                created_at: chrono::Utc::now().to_rfc3339(),
                result_dir: task_dir.clone(),
                files: vec![],
                response: String::new(),
                messages: vec![serde_json::json!({"role": "user", "content": prompt.clone()})],
                plan: None,
                stage_outputs: Vec::new(),
            event_log: Vec::new(),
            });
            (uuid_str, brief, task_dir, wf_dir)
        };

    let _ = ws_send(socket, serde_json::json!({
        "type": "task_started",
        "task_id": task_id,
    })).await;

    let _ = ws_send(socket, serde_json::json!({
        "type": "status",
        "message": format!("Starting loop pipeline: {}", task_brief),
    })).await;

    let cancel = CancellationToken::new();
    state.cancels.insert(task_id.clone(), cancel.clone());
    let max_loops = state.config.loop_max_loops;

    let result = miniagent_loop_pipeline::LoopPipeline::run(
        prompt.clone(),
        state.config.clone(),
        max_loops,
        cancel.clone(),
    ).await;

    state.cancels.remove(&task_id);

    match result {
        Ok(pipeline_state) => {
            // Send plan if available
            if let Some(ref plan) = pipeline_state.plan {
                let stages: Vec<serde_json::Value> = plan.tasks.iter().map(|t| {
                    serde_json::json!({
                        "name": t.id,
                        "handler": t.assigned_role,
                        "tier": t.difficulty,
                        "description": format!("{} (expected: {})", t.description, t.expected_output),
                        "sub_tasks": t.depends_on.clone(),
                        "tools": serde_json::json!([]),
                    })
                }).collect();
                let _ = ws_send(socket, serde_json::json!({
                    "type": "plan",
                    "workflow": "loop_pipeline",
                    "stages": stages,
                })).await;
            }

            // Send stage outputs
            for stage_output in &pipeline_state.stage_outputs {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "stage_output",
                    "stage": stage_output.stage,
                    "summary": stage_output.summary,
                })).await;
            }

            let response_text = pipeline_state.final_output
                .clone()
                .unwrap_or_else(|| "(no final output)".to_string());
            let _ = ws_send(socket, serde_json::json!({
                "type": "stream",
                "text": response_text.clone(),
            })).await;

            finalize_task(socket, state, &task_id, &task_brief, &task_dir, &task_dir, &["loop_pipeline".to_string()], response_text).await;
        }
        Err(e) => {
            let _ = ws_send(socket, serde_json::json!({
                "type": "error",
                "message": format!("Loop pipeline failed: {e}"),
            })).await;
        }
    }
}

/// 总评审结果（FeedbackStage 产出）。
struct FeedbackResult {
    /// "deliver"（交付）/ "revise"（需修改）/ "unclear"（不确定）
    verdict: String,
    /// 评审摘要
    summary: String,
}

/// 运行总评审（FeedbackStage）：综合所有 stage 产物 + 原始需求，决定交付质量。
///
/// 复用三角色的 Arbiter 思路，但在 workflow 整体执行后做一次总评审，
/// 而非逐 stage 评审（避免侵入 workflow 执行）。
async fn run_feedback_review(
    provider: &dyn LlmProvider,
    original_request: &str,
    stage_outputs: &[serde_json::Value],
) -> FeedbackResult {
    let outputs_text: String = stage_outputs.iter()
        .map(|s| {
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
            let response = s.get("response").and_then(|v| v.as_str())
                .or_else(|| s.get("output").and_then(|v| v.as_str()))
                .unwrap_or("");
            format!("[{name}]: {response}\n")
        })
        .collect();

    let system = "You are a final reviewer. Evaluate whether the completed work fully addresses the user's original request. \
Respond in JSON: {\"verdict\": \"deliver|revise|unclear\", \"summary\": \"brief assessment\"}";

    let prompt = format!(
        "## Original Request\n{original_request}\n\n\
         ## Completed Work\n{outputs_text}\n\n\
         Does the completed work fully address the original request? \
         If yes, verdict=deliver. If there are significant gaps, verdict=revise. \
         If you cannot determine, verdict=unclear."
    );

    let request = miniagent_provider::traits::CompletionRequest {
        system: system.into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.1), max_tokens: Some(500), ..Default::default()
        },
    };

    match provider.complete(&request, CancellationToken::new()).await {
        Ok(resp) => {
            let text: String = resp.content.iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("");
            let cleaned = text.trim()
                .trim_start_matches("```json").trim_start_matches("```")
                .trim_end_matches("```").trim();
            match serde_json::from_str::<serde_json::Value>(cleaned) {
                Ok(v) => FeedbackResult {
                    verdict: v.get("verdict").and_then(|v| v.as_str()).unwrap_or("unclear").to_string(),
                    summary: v.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                },
                Err(e) => {
                    tracing::error!(error = %e, "feedback review parse failed");
                    FeedbackResult { verdict: "unclear".into(), summary: "Review parse failed".into() }
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "feedback review LLM call failed");
            FeedbackResult { verdict: "unclear".into(), summary: format!("Review failed: {e}") }
        }
    }
}

/// 向用户提问并等待回复（双向 ws ask 协议）。
///
/// 在 task 执行期间，server 推 `{type:'ask', question, options}` 给前端，
/// 然后注册 oneshot channel 到 `state.asks` 并 await。
/// 前端回复 `{type:'ask_reply', answer}` 时，handle_ws 取出 Sender 唤醒。
///
/// 超时 5 分钟自动返回空字符串（防前端关闭导致永久阻塞）。
async fn ask_user(
    socket: &Arc<Mutex<WsSink>>,
    state: &AppState,
    task_id: &str,
    question: &str,
    options: &[&str],
) -> String {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.asks.insert(task_id.to_string(), tx);

    let _ = ws_send(socket, serde_json::json!({
        "type": "ask",
        "task_id": task_id,
        "question": question,
        "options": options,
    })).await;

    // 等待回复，5 分钟超时
    match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
        Ok(Ok(answer)) => {
            state.asks.remove(task_id);
            answer
        }
        _ => {
            state.asks.remove(task_id);
            tracing::error!(task_id = task_id, "ask timed out or sender dropped");
            String::new()
        }
    }
}

async fn handle_run(
    socket: &Arc<Mutex<WsSink>>,
    state: &AppState,
    prompt: String,
    file_ids: Vec<String>,
    existing_task_id: Option<String>,
) {
    let api_key = state.config.require_active_key().unwrap_or_else(|e| {
        // This should not happen — server validates key at startup
        eprintln!("FATAL: {e}");
        std::process::exit(1);
    });
    let agent_arc = state.agent.clone();

    let (task_id, task_brief, task_dir, task_workflow_dir) =
        if let Some(ref existing_id) = existing_task_id {
            // Follow-up message in existing conversation
            if let Some(ref task) = state.tasks.get(existing_id) {
                let brief = task.brief.clone();
                let dir = task.result_dir.clone();
                // Each follow-up gets its own timestamped sub-directory, keeping all history
                let turn_ts = chrono::Utc::now().format("turn_%Y%m%d_%H%M%S").to_string();
                let wf_dir = dir.join(&turn_ts);
                let _ = std::fs::create_dir_all(&wf_dir);
                // Append user message to history
                if let Some(mut t) = state.tasks.get_mut(existing_id) {
                    t.status = "running".into();
                    t.messages.push(serde_json::json!({"role": "user", "content": &prompt}));
                }
                (existing_id.clone(), brief, dir, wf_dir)
            } else {
                // Fallback: new task if existing not found
                create_new_task(state, &prompt)
            }
        } else {
            create_new_task(state, &prompt)
        };

    // Read and parse uploaded files
    let enriched_prompt = enrich_prompt_with_files(&prompt, &file_ids, &state.task_dir);

    // ── ExploreStage（需求1：明确问题+获取上下文）──────────────────
    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "explore", "status": "running",
        "task_id": &task_id,
    })).await;

    // Explore：用 planner provider 分析问题，提取关键上下文
    let explore_provider: Box<dyn LlmProvider> = if state.config.is_minimax() {
        Box::new(MiniMaxFlash::new(api_key))
    } else if state.config.is_stepfun() {
        Box::new(StepFunFlash::new(api_key))
    } else {
        Box::new(DeepSeekFlash::new(api_key))
    };
    let explore_ctx = miniagent_core::config::InferenceConfig {
        temperature: Some(0.3), max_tokens: Some(2000), ..Default::default()
    };
    let today = chrono::Utc::now().format("%Y-%m-%d");

    let explore_resp = explore_provider.complete(
        &miniagent_provider::traits::CompletionRequest {
            system: format!("You are a task analyst. The current date is {today}. \
                     Analyze the user's request and provide: \
                     1) A clear problem statement \
                     2) Key context/requirements \
                     3) Whether the request is clear enough to proceed (answer 'clear' or 'ambiguous') \
                     IMPORTANT: When the user says 'this year' or 'today', they mean the year {today_year}. \
                     Respond concisely.",
                today_year = chrono::Utc::now().format("%Y")),
            messages: vec![miniagent_core::message::Message::user(&enriched_prompt)],
            tools: vec![],
            config: explore_ctx,
        },
        tokio_util::sync::CancellationToken::new(),
    ).await;

    let explore_summary = match explore_resp {
        Ok(r) => r.content.iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join(""),
        Err(e) => {
            tracing::error!(error = %e, "explore stage failed, proceeding with raw prompt");
            enriched_prompt.clone()
        }
    };

    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "explore", "status": "done",
        "task_id": &task_id,
    })).await;

    // ── AskStage（可选：若问题不清晰，反问用户）────────────────────
    if explore_summary.to_lowercase().contains("ambiguous") {
        let answer = ask_user(
            socket, state, &task_id,
            "Your request needs clarification. Could you provide more details about your specific requirements?",
            &["Proceed with best guess", "Let me clarify"],
        ).await;
        // 如果用户选 "Let me clarify" 但没输入文本，继续用 best guess
        if !answer.is_empty() && !answer.contains("Proceed") {
            // 用户提供了澄清，拼入 prompt
            // 注：answer 可能包含选项文本+用户补充
        }
    }

    // Send status
    let _ = ws_send(socket, serde_json::json!({
        "type": "status",
        "message": "Planning workflow...",
    }))
    .await;

    // ── PlanStage ──────────────────────────────────────────────────
    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "plan", "status": "running",
        "task_id": &task_id,
    })).await;

    // Plan via LLM（根据 PROVIDER 配置选择 provider，尊重 PROVIDER=stepfun）
    let planner_flash: Box<dyn LlmProvider> = if state.config.is_minimax() {
        Box::new(MiniMaxFlash::new(api_key))
    } else if state.config.is_stepfun() {
        Box::new(StepFunFlash::new(api_key))
    } else {
        Box::new(DeepSeekFlash::new(api_key))
    };
    let planner = PlannerStage::new(planner_flash);
    let plan_ctx = StageContext::new(
        StageId::new(),
        serde_json::json!({ "prompt": enriched_prompt }),
        HashMap::new(),
        CancellationToken::new(),
    );

    // Use `match` (not `unwrap_or_else`) so the WS fallback send can be
    // awaited — a sync closure can't `.await`, which is what previously caused
    // the status future to be dropped silently.
    let plan_output = match planner.execute(&plan_ctx).await {
        Ok(o) => o,
        Err(e) => {
            let _ = ws_send(socket, serde_json::json!({
                "type": "status",
                "message": format!("Planner fallback: {e}"),
            }))
            .await;
            miniagent_workflow::stage::StageOutput {
                data: serde_json::json!({ "workflow_spec": WorkflowSpec {
                    task_type: "single_agent".into(),
                    stages: vec![StageSpec {
                        name: "agent".into(),
                        handler_type: "agent".into(),
                        system_prompt: String::new(),
                        tools: vec![],
                        model_tier: "flash".into(),
                        max_iterations: 50,
                        enable_skills: true,
                        description: String::new(),
                        sub_tasks: vec![],
                    }],
                    edges: vec![],
                } }),
                metadata: miniagent_workflow::stage::StageMetadata {
                    duration_ms: 0,
                    items_processed: 0,
                    success: true,
                    error: None,
                },
            }
        }
    };

    let spec: WorkflowSpec = serde_json::from_value(plan_output.data["workflow_spec"].clone())
        .unwrap_or_else(|_| WorkflowSpec {
            task_type: "single_agent".into(),
            stages: vec![StageSpec {
                name: "agent".into(),
                handler_type: "agent".into(),
                system_prompt: String::new(),
                tools: vec![],
                model_tier: "flash".into(),
                max_iterations: 50,
                enable_skills: true,
                description: String::new(),
                sub_tasks: vec![],
            }],
            edges: vec![],
        });

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
    let builder = WorkflowBuilder::new(agent_arc.clone(), state.config.clone())
        .with_task_dir(task_workflow_dir.to_string_lossy());

    let system_prompt = format!("You are an AI agent with direct access to system tools. You MUST use tools for actions — NEVER simulate or describe tool output.\n\
         Available tools: pubmed_search, web_search, web_fetch, patent_search, clinical_trials_search, \
         read, write, edit, glob, grep, bash, git, conda, notebook_edit, ask_user.\n\n\
         ## Task Execution Principles\n\
         - Read before modifying. Do not propose changes to files you haven't read.\n\
         - Don't over-engineer. Do the minimum needed — no speculative abstractions.\n\
         - If an approach fails, diagnose why before switching tactics.\n\n\
         ## Tool Usage Preferences\n\
         - Use read/edit/write/glob/grep instead of bash equivalents (cat, sed, find, grep).\n\
         - Reserve bash for system commands requiring shell execution.\n\
         - Call multiple independent tools in parallel.\n\n\
         ## Risk Assessment\n\
         - Reversible actions (editing files, running tests) are fine to do freely.\n\
         - Destructive actions (deleting files, force push) — verify before executing.\n\n\
         ## Output Efficiency\n\
         - Go straight to the point. Lead with results, not reasoning.\n\
         - Record important tool results in your response — they may be cleared from context later.\n\
         - If you can say it in one sentence, don't use three.\n\n\
         {}{}",
        miniagent_core::context_info::env_block(&task_workflow_dir.to_string_lossy()),
        miniagent_core::context_info::project_md_block(&task_workflow_dir.to_string_lossy())
            .map(|s| format!("\n\n{s}")).unwrap_or_default()
    );

    // Use `match` (not `unwrap_or_else`) so the WS fallback send can be
    // awaited — a sync closure can't `.await`, which is what previously caused
    // the status future to be dropped silently.
    let workflow = match builder.build(&spec, &enriched_prompt, &system_prompt) {
        Ok(w) => w,
        Err(e) => {
            let _ = ws_send(socket, serde_json::json!({
                "type": "status",
                "message": format!("Build fallback: {e}"),
            }))
            .await;
            let fallback = WorkflowSpec {
                task_type: "single_agent".into(),
                stages: vec![StageSpec {
                    name: "agent".into(),
                    handler_type: "agent".into(),
                    system_prompt: String::new(),
                    tools: vec![],
                    model_tier: "flash".into(),
                    max_iterations: 50,
                    enable_skills: true,
                    description: String::new(),
                    sub_tasks: vec![],
                }],
                edges: vec![],
            };
            WorkflowBuilder::new(agent_arc.clone(), state.config.clone())
                .with_task_dir(task_workflow_dir.to_string_lossy())
                .build(&fallback, &enriched_prompt, &system_prompt)
                .expect("single-agent fallback should always build")
        }
    };

    let cancel = CancellationToken::new();
    state.cancels.insert(task_id.clone(), cancel.clone());

    // Determine if last stage is a pure-LLM stage that can be streamed
    let last_stage = spec.stages.last();
    let stream_last = last_stage.is_some_and(|s|
        matches!(s.handler_type.as_str(), "synthesizer" | "critic" | "llm")
    );

    if stream_last && spec.stages.len() > 1 {
        // Multi-stage: run all stages except the last with progress,
        // then stream the final synthesis stage directly.
        run_multi_stage_with_streaming(
            socket, workflow, &spec, &task_workflow_dir, api_key,
            &task_id, &task_brief, &task_dir, state, &agent_arc, cancel,
        ).await;
    } else {
        // Single-stage or agent-only last stage: run with progress callback
        run_with_progress(socket, workflow, &spec, &task_workflow_dir,
            &task_id, &task_brief, &task_dir, state, &agent_arc, cancel,
        ).await;
    }
    // Clean up cancel token after run completes (success or error)
    state.cancels.remove(&task_id);

    // ── FeedbackStage（需求1：总评审，决定交付或回退）─────────────
    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "feedback", "status": "running",
        "task_id": &task_id,
    })).await;

    // 收集各 stage 产物做总评审
    let stage_outputs_snapshot: Vec<serde_json::Value> = {
        let task = state.tasks.get(&task_id);
        task.map(|t| t.stage_outputs.clone()).unwrap_or_default()
    };

    // Provider for validator/arbiter/feedback
    let feedback_provider: Box<dyn LlmProvider> = if state.config.is_minimax() {
        Box::new(MiniMaxFlash::new(api_key))
    } else if state.config.is_stepfun() {
        Box::new(StepFunFlash::new(api_key))
    } else {
        Box::new(DeepSeekFlash::new(api_key))
    };

    // ── 三角色逐 stage 校验（接入 execute_with_roles 的 Validator+Arbiter）──
    // 对每个 stage 产物做 Validator 校验 + Arbiter 决策
    // 如果 Arbiter 决定 Revise，记录到 feedback 中
    let mut stage_issues: Vec<String> = Vec::new();
    for stage in &stage_outputs_snapshot {
        let stage_name = stage.get("name").or_else(|| stage.get("stage"))
            .and_then(|v| v.as_str()).unwrap_or("unknown");
        let stage_output = stage.get("response").or_else(|| stage.get("summary"))
            .and_then(|v| v.as_str()).unwrap_or("");

        if stage_output.is_empty() { continue; }

        // Validator 校验
        let validation = miniagent_loop_pipeline::roles::run_validator(
            &*feedback_provider,
            &format!("Stage: {stage_name}"), "Quality output",
            stage_output,
            tokio_util::sync::CancellationToken::new(),
        ).await;

        match validation {
            Ok(report) => {
                if !report.passed {
                    // Arbiter 决策
                    let decision = miniagent_loop_pipeline::roles::run_arbiter_forgiving(
                        &*feedback_provider,
                        &format!("Stage: {stage_name}"),
                        stage_output,
                        &report,
                        tokio_util::sync::CancellationToken::new(),
                    ).await;

                    if !decision.is_pass() {
                        let feedback = decision.feedback().unwrap_or("needs improvement");
                        stage_issues.push(format!(
                            "Stage '{stage_name}': {} — {}",
                            if matches!(decision, miniagent_loop_pipeline::roles::ArbiterDecision::Revise { .. }) { "Revise" } else { "Supplement" },
                            feedback,
                        ));
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, stage = stage_name, "validator failed for stage");
            }
        }
    }

    // 如果三角色发现问题，附加到 stage_issues
    if !stage_issues.is_empty() {
        tracing::info!(issue_count = stage_issues.len(), "three-role validation found issues");
    }

    // 总评审（综合三角色结果 + 所有 stage 产物）
    let feedback_result = run_feedback_review(
        &*feedback_provider, &enriched_prompt, &stage_outputs_snapshot,
    ).await;

    // 如果三角色发现问题但总评审说 deliver，修正为 revise
    let final_verdict = if !stage_issues.is_empty() && feedback_result.verdict == "deliver" {
        "revise".to_string()
    } else {
        feedback_result.verdict.clone()
    };

    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "feedback", "status": "done",
        "task_id": &task_id,
        "verdict": &final_verdict,
        "summary": &feedback_result.summary,
        "stage_issues": stage_issues,
    })).await;

    // 如果总评审或三角色发现问题，推反馈给前端
    if final_verdict != "deliver" {
        let issue_text = if stage_issues.is_empty() {
            feedback_result.summary.clone()
        } else {
            format!("{}\n\nIssues:\n{}", feedback_result.summary, stage_issues.join("\n"))
        };
        let _ = ws_send(socket, serde_json::json!({
            "type": "status",
            "message": format!("📋 Feedback: {} — {}", final_verdict, issue_text),
        })).await;
    }
    // ──────────────────────────────────────────────────────────────
}

/// Run workflow with per-stage progress (non-streaming response).
async fn run_with_progress(
    socket: &Arc<Mutex<WsSink>>,
    workflow: miniagent_workflow::Workflow,
    spec: &WorkflowSpec,
    task_workflow_dir: &StdPath,
    task_id: &str,
    task_brief: &str,
    task_dir: &StdPath,
    state: &AppState,
    agent_arc: &Arc<Agent>,
    cancel: CancellationToken,
) {
    let stage_names: Vec<String> = spec.stages.iter().map(|s| s.name.clone()).collect();

    // Channel for progress updates + final result from workflow
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<ProgressMsg>(32);

    // Bridge fine-grained agent events (tool calls, skills, lifecycle) from a
    // broadcast channel into the mpsc so the WebSocket loop below can ship them.
    let (agent_event_tx, mut agent_event_rx) =
        tokio::sync::broadcast::channel::<miniagent_core::event::AgentEvent>(64);
    let agent_tx_for_fwd = progress_tx.clone();
    tokio::spawn(async move {
        while let Ok(ev) = agent_event_rx.recv().await {
            if agent_tx_for_fwd.send(ProgressMsg::AgentEvent(ev)).await.is_err() { break; }
        }
    });

    // Spawn workflow execution
    let wf_cancel = cancel.clone();
    let progress_fn = {
        let tx = progress_tx.clone();
        move |name: &str, status: &str, data: Option<&serde_json::Value>| {
            let _ = tx.try_send(ProgressMsg::Stage {
                name: name.to_string(),
                status: status.to_string(),
                data: data.cloned(),
            });
        }
    };
    tokio::spawn(async move {
        let result = workflow.run_with_progress(None, wf_cancel, Box::new(progress_fn)).await;
        let _ = progress_tx.send(ProgressMsg::Done(result)).await;
    });

    // Wire the agent's broadcast sender so tool/skill events flow to the frontend.
    agent_arc.set_event_sender(agent_event_tx).await;

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
                if status == "completed"
                    && let Some(ref d) = data {
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
            ProgressMsg::AgentEvent(event) => {
                let event_json = serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
                // 需求2: 全链路追溯——把每个 AgentEvent（含工具调用完整 input/output/error）
                // 持久化到 task 的 event_log，重启后可经 /api/trace/{task_id} 查询。
                let traced = serde_json::json!({
                    "ts": chrono::Utc::now().to_rfc3339(),
                    "event": event_json,
                });
                if let Some(mut task) = state.tasks.get_mut(task_id) {
                    task.event_log.push(traced.clone());
                }
                let _ = ws_send(socket, serde_json::json!({
                    "type": "agent_event",
                    "event": event_json,
                })).await;
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

            finalize_task(socket, state, task_id, task_brief, task_dir, task_workflow_dir, &stage_names, response_text).await;
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
    socket: &Arc<Mutex<WsSink>>,
    workflow: miniagent_workflow::Workflow,
    spec: &WorkflowSpec,
    task_workflow_dir: &StdPath,
    api_key: &miniagent_core::secrets::ApiKey,
    task_id: &str,
    task_brief: &str,
    task_dir: &StdPath,
    state: &AppState,
    agent_arc: &Arc<Agent>,
    cancel: CancellationToken,
) {
    let stage_names: Vec<String> = spec.stages.iter().map(|s| s.name.clone()).collect();

    // Run the full workflow with progress
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<ProgressMsg>(32);

    // Bridge fine-grained agent events from broadcast → mpsc.
    let (agent_event_tx, mut agent_event_rx) =
        tokio::sync::broadcast::channel::<miniagent_core::event::AgentEvent>(64);
    let agent_tx_for_fwd = progress_tx.clone();
    tokio::spawn(async move {
        while let Ok(ev) = agent_event_rx.recv().await {
            if agent_tx_for_fwd.send(ProgressMsg::AgentEvent(ev)).await.is_err() { break; }
        }
    });

    let wf_cancel = cancel.clone();
    let progress_fn = {
        let tx = progress_tx.clone();
        move |name: &str, status: &str, data: Option<&serde_json::Value>| {
            let _ = tx.try_send(ProgressMsg::Stage {
                name: name.to_string(),
                status: status.to_string(),
                data: data.cloned(),
            });
        }
    };
    tokio::spawn(async move {
        let result = workflow.run_with_progress(None, wf_cancel, Box::new(progress_fn)).await;
        let _ = progress_tx.send(ProgressMsg::Done(result)).await;
    });

    // Wire the agent's broadcast sender so tool/skill events flow to the frontend.
    agent_arc.set_event_sender(agent_event_tx).await;

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
                if status == "completed"
                    && let Some(ref d) = data {
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
            ProgressMsg::AgentEvent(event) => {
                let event_json = serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
                // 需求2: 全链路追溯——把每个 AgentEvent（含工具调用完整 input/output/error）
                // 持久化到 task 的 event_log，重启后可经 /api/trace/{task_id} 查询。
                let traced = serde_json::json!({
                    "ts": chrono::Utc::now().to_rfc3339(),
                    "event": event_json,
                });
                if let Some(mut task) = state.tasks.get_mut(task_id) {
                    task.event_log.push(traced.clone());
                }
                let _ = ws_send(socket, serde_json::json!({
                    "type": "agent_event",
                    "event": event_json,
                })).await;
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
                let stream_result = stream_synthesis(socket, api_key, &response_text, state.config.is_minimax(), state.config.is_stepfun(), cancel).await;
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

            finalize_task(socket, state, task_id, task_brief, task_dir, task_workflow_dir, &stage_names, final_text).await;
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
    socket: &Arc<Mutex<WsSink>>,
    api_key: &miniagent_core::secrets::ApiKey,
    synthesis_text: &str,
    is_minimax: bool,
    is_stepfun: bool,
    cancel: CancellationToken,
) -> bool {
    let pro: Box<dyn LlmProvider> = if is_minimax {
        Box::new(MiniMaxFlash::new(api_key))
    } else if is_stepfun {
        Box::new(StepFunFlash::new(api_key))
    } else {
        Box::new(miniagent_provider::deepseek::DeepSeekPro::new(api_key))
    };
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
        if let Some(text) = output.data["response"].as_str()
            && !text.is_empty() {
                response_text = text.to_string();
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
    socket: &Arc<Mutex<WsSink>>,
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
            // Store path relative to task_dir so the catch-all download route can fetch it.
            result_files.push(output_filename.clone());
        }
    }

    // List workflow artifacts as paths relative to task_dir (the task's result_dir).
    // Previously these were flattened to bare filenames, which 404'd under the old
    // single-segment download route; the new catch-all route supports nested paths.
    if let Ok(entries) = std::fs::read_dir(task_workflow_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && (name.ends_with(".md") || name.ends_with(".json")) {
                    let rel = entry
                        .path()
                        .strip_prefix(task_dir)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| name.to_string());
                    result_files.push(rel);
                }
        }
    }

    // Update task
    if let Some(mut task) = state.tasks.get_mut(task_id) {
        task.status = "completed".into();
        task.files = result_files.clone();
        task.response.clone_from(&response_text);
        // Append AI response to multi-turn message history
        if !response_text.is_empty() {
            task.messages.push(serde_json::json!({"role": "assistant", "content": response_text}));
        }

        // Persist plan, stage_outputs, and messages to disk for history replay after restart
        let metadata = serde_json::json!({
            "plan": task.plan,
            "stage_outputs": task.stage_outputs.clone(),
            "messages": task.messages.clone(),
        });
        if let Ok(json_str) = serde_json::to_string_pretty(&metadata)
            && let Err(e) = std::fs::write(task_dir.join("metadata.json"), json_str) {
                tracing::error!(task_id = %task_id, error = %e, "failed to persist metadata.json — task history may be lost on restart");
            }
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
    AgentEvent(miniagent_core::event::AgentEvent),
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
    if let Some(response) = data["response"].as_str()
        && !response.is_empty() {
            let preview: String = response.chars().take(300).collect();
            summary["response_preview"] = serde_json::json!(preview);
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

async fn ws_send(socket: &Arc<Mutex<WsSink>>, msg: serde_json::Value) {
    let text = match serde_json::to_string(&msg) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "ws_send: JSON serialize failed");
            return;
        }
    };
    let mut s = socket.lock().await;
    if let Err(e) = s.send(Message::Text(text.into())).await {
        tracing::warn!(error = ?e, "ws_send: send failed");
    }
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
        .unwrap_or_else(|| {
            format!(
                "You are a helpful AI research assistant.\n\n{}",
                miniagent_core::context_info::env_block(".")
            )
        });

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
            // transcript 修复：修补崩溃时可能产生的孤立 tool_use（防 API 校验错误）
            let fixed = miniagent_core::message::validate_transcript(&mut history);
            if fixed > 0 {
                tracing::info!(fixed, "transcript repaired on resume");
            }
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

fn create_new_task(state: &AppState, prompt: &str) -> (String, String, std::path::PathBuf, std::path::PathBuf) {
    let task_id = Uuid::new_v4().to_string()[..8].to_string();
    let task_brief = sanitize_task_brief(prompt);
    let task_dir_name = format!("{}_{}", task_id, task_brief);
    let task_dir = state.task_dir.join(&task_dir_name);
    let task_workflow_dir = task_dir.join(".workflow");
    let _ = std::fs::create_dir_all(&task_workflow_dir);
    // Clean shared workflow dir to prevent cross-task contamination
    let shared_wf = state.task_dir.join(".workflow");
    let _ = std::fs::remove_dir_all(&shared_wf);
    let _ = std::fs::create_dir_all(&shared_wf);

    let task_info = TaskInfo {
        id: task_id.clone(),
        brief: task_brief.clone(),
        prompt: prompt.to_string(),
        status: "running".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        result_dir: task_dir.clone(),
        files: vec![],
        response: String::new(),
        messages: vec![serde_json::json!({"role": "user", "content": prompt})],
        plan: None,
        stage_outputs: Vec::new(),
            event_log: Vec::new(),
    };
    state.tasks.insert(task_id.clone(), task_info);

    (task_id, task_brief, task_dir, task_workflow_dir)
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
