use std::collections::HashMap;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use miniagent_agent::Agent;
use miniagent_core::config::InferenceConfig;
use miniagent_core::message::Message as AgentMessage;
use miniagent_core::models::{ModelKind, ModelProfile};
use miniagent_provider::factory::{build_provider, build_provider_pair, ProviderTier};
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
    // Cap the per-request body size at 64 MiB. The /api/upload endpoint
    // streams multipart payloads directly into a per-task buffer and the
    // previous `unbounded` default allowed a single client to OOM the
    // server with a multi-GB upload. WebSocket upgrades are exempt from
    // this limit by virtue of using a separate request path.
    const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

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
        // Runtime LLM model registry (add / select / delete models)
        .route("/api/models", get(models_handler).post(add_model_handler))
        .route("/api/models/{id}", axum::routing::put(update_model_handler).delete(delete_model_handler))
        .route("/api/models/{id}/activate", post(activate_model_handler))
        // Debate role model selection (⚙️ settings)
        .route("/api/debate-models", get(debate_models_handler).post(set_debate_models_handler))
        // Unified settings snapshots: /api/kinds enumerates every supported
        // provider family (icon + label), /api/settings/active returns the
        // currently-active model + debate role snapshot in one round-trip so
        // the frontend header dropdown and the settings page can hydrate
        // from a single response.
        .route("/api/kinds", get(kinds_handler))
        .route("/api/settings/active", get(settings_active_handler))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
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

/// Return the provenance record(s) (audit trail) for a run's data-analysis
/// tasks.
///
/// Provenance is written by `miniagent-analysis` to
/// `analysis/**/provenance.json` inside the task's result directory (script
/// hash, I/O hashes, conda env + package versions, seed, git commit, exit
/// code, stdout/stderr digests). The handler resolves the run directory from
/// the task registry — never from the process CWD — and returns every
/// provenance record found beneath it, so each analysis result is
/// reproducible and inspectable.
async fn provenance_handler(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Json<serde_json::Value> {
    let Some(task) = state.tasks.get(&task_id) else {
        return Json(serde_json::json!({
            "error": "task not found",
            "task_id": task_id,
        }));
    };
    let result_dir = task.result_dir.clone();
    drop(task);

    // Depth-bounded recursive scan for provenance records under the run dir.
    fn collect_provenance(
        dir: &std::path::Path,
        depth: usize,
        out: &mut Vec<(std::path::PathBuf, serde_json::Value)>,
    ) {
        if depth > 5 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_provenance(&path, depth + 1, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("provenance.json") {
                if let Ok(body) = std::fs::read_to_string(&path) {
                    let record = serde_json::from_str(&body)
                        .unwrap_or_else(|_| serde_json::json!({ "raw": body }));
                    out.push((path, record));
                }
            }
        }
    }

    let mut records: Vec<(std::path::PathBuf, serde_json::Value)> = Vec::new();
    collect_provenance(&result_dir, 0, &mut records);

    if records.is_empty() {
        return Json(serde_json::json!({
            "error": "provenance not found",
            "task_id": task_id,
            "searched_dir": result_dir.display().to_string(),
        }));
    }

    Json(serde_json::json!({
        "task_id": task_id,
        "count": records.len(),
        "records": records
            .into_iter()
            .map(|(path, record)| {
                serde_json::json!({
                    "path": path.display().to_string(),
                    "provenance": record,
                })
            })
            .collect::<Vec<_>>(),
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
) -> Result<Json<serde_json::Value>, StatusCode> {
    let task = state.tasks.get(&task_id).ok_or(StatusCode::NOT_FOUND)?;
    // P2: include the session goal_state (session.json) so REST consumers see
    // the accumulated constraints, same as the WS get_task payload.
    let mut v = serde_json::to_value(task.value().clone())
        .unwrap_or(serde_json::Value::Null);
    if let Some(gs) = load_goal_state(&task.result_dir)
        && let Ok(gsv) = serde_json::to_value(&gs) {
            v["goal_state"] = gsv;
        }
    Ok(Json(v))
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
                    let mode = req.mode.clone();
                    let files2 = req.files.clone();
                    let handle = tokio::spawn(async move {
                        match mode.as_str() {
                            "debate" => {
                                // 辩论模式：正方 vs 反方 → 裁判（角色模型来自 ⚙️ 设置）
                                let _ = handle_debate_run(&sink2, &state2, req.prompt, task_id).await;
                            }
                            "loop" | "loop_pipeline" | "loop-pipeline" => {
                                // Loop pipeline: 迭代 Explore→Plan→Dispatch→Evaluate→Repair
                                let _ = handle_run_loop(&sink2, &state2, req.prompt, task_id).await;
                            }
                            "research" => {
                                // Research pipeline: 文献→KG→致病机理假说→辩论→验证计划→数据分析 notebook
                                let _ = handle_research_run(&sink2, &state2, req.prompt, task_id).await;
                            }
                            _ => {
                                // 默认：单智能体 ReAct + 计划 + 反馈（workflow 路径）
                                let _ = handle_run(&sink2, &state2, req.prompt, files2, task_id).await;
                            }
                        }
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
                "steer"
                    // P3 执行中转向：运行中用户插入指令。入队等待阶段边界
                    // 消费；同时记入对话历史（用户消息在重绘后不丢），并
                    // 立即回执让用户知道指令已受理。
                    if !req.task_id.is_empty() => {
                        state.steers.entry(req.task_id.clone()).or_default().push(req.prompt.clone());
                        if let Some(mut t) = state.tasks.get_mut(&req.task_id) {
                            t.messages.push(serde_json::json!({
                                "role": "user",
                                "content": format!("➡️ [转向] {}", req.prompt),
                            }));
                        }
                        let _ = ws_send(&sink, serde_json::json!({
                            "type": "status",
                            "task_id": &req.task_id,
                            "message": "✅ 转向指令已受理：将在当前阶段完成后生效",
                        })).await;
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
                            "event_log": task.event_log.clone(),
                        });
                        if !task.messages.is_empty() {
                            response["messages"] = serde_json::json!(task.messages);
                        }
                        // P2: expose the session goal_state for the frontend
                        // constraints panel.
                        if let Some(gs) = load_goal_state(&task.result_dir)
                            && let Ok(v) = serde_json::to_value(&gs) {
                                response["goal_state"] = v;
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
///
/// Wired in via [`DriverKind::Loop`]. Emits the same `progress` /
/// `agent_event` envelope as the workflow path so the front-end progress
/// panel renders both modes uniformly.
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
    let agent_arc = state.agent.clone();

    // Task creation goes through the same `create_new_task` helper as the
    // workflow mode: 8-char id + sanitized brief under `state.task_dir`, so
    // all modes share one `result/{id}_{brief}` naming scheme and the
    // restart-restore scan picks loop tasks up too.
    let (task_id, task_brief, task_dir, _task_workflow_dir) =
        if let Some(ref existing_id) = existing_task_id {
            // Read-then-drop before the get_mut below (same-shard guard rules).
            let reused = state.tasks.get(existing_id).map(|task| {
                (task.brief.clone(), task.result_dir.clone())
            });
            if let Some((brief, dir)) = reused {
                let wf_dir = dir.join(".workflow");
                let _ = std::fs::create_dir_all(&wf_dir);
                // Record the user's message in the multi-turn history — loop
                // follow-ups previously skipped this, so redraws (task click /
                // page refresh) lost the user's message while the reply
                // remained (live-reported "对话消息被吞").
                if let Some(mut t) = state.tasks.get_mut(existing_id) {
                    t.status = "running".into();
                    t.messages.push(serde_json::json!({"role": "user", "content": &prompt}));
                }
                (existing_id.clone(), brief, dir, wf_dir)
            } else {
                create_new_task(state, &prompt)
            }
        } else {
            create_new_task(state, &prompt)
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

    // ── Progress + AgentEvent bridges (mirror handle_run's pattern) ──
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<ProgressMsg>(32);
    let (agent_event_tx, mut agent_event_rx) =
        tokio::sync::broadcast::channel::<miniagent_core::event::AgentEvent>(64);
    let agent_tx_for_fwd = progress_tx.clone();
    tokio::spawn(async move {
        while let Ok(ev) = agent_event_rx.recv().await {
            if agent_tx_for_fwd.send(ProgressMsg::AgentEvent(ev)).await.is_err() { break; }
        }
    });
    // Register a *per-task* event sender so concurrent loop pipelines no
    // longer clobber each other's event streams (each task gets its own
    // independent subscription). The RAII guard is stored in AppState and
    // dropped on completion / cancel so the shared Agent's event list
    // stays bounded.
    let event_guard = agent_arc.register_event_sender(agent_event_tx.clone()).await;
    state.event_guards.insert(task_id.to_string(), event_guard);

    // Build a ProgressFn closure that ships per-stage events into the
    // progress channel. The closure is `FnMut` and only touches local
    // variables, so it can be moved into LoopPipeline::run.
    let progress_tx_for_cb = progress_tx.clone();
    let on_progress: miniagent_core::orchestration::ProgressFn = Box::new(move |name, status, data| {
        let payload = ProgressMsg::Stage {
            name: name.to_string(),
            status: status.to_string(),
            data: data.cloned(),
        };
        let _ = progress_tx_for_cb.try_send(payload);
    });

    // Drain the channel while the pipeline runs in a separate task.
    let drain_socket = Arc::clone(socket);
    let drain_state = state.clone();
    let drain_task_id = task_id.clone();
    let drain_task_dir = task_dir.clone();
    let drain_task_brief = task_brief.clone();
    let drain_handle = tokio::spawn(async move {
        drain_progress_channel(
            drain_socket,
            drain_state,
            drain_task_id,
            drain_task_brief,
            drain_task_dir,
            &mut progress_rx,
        )
        .await;
    });

    // Clarify channel: the loop pipeline's Clarify stage asks the user
    // through the WS ask/reply protocol when the task has material ambiguity
    // (timeout ⇒ assumption noted, run continues). Same semantics as
    // research mode.
    let ask_socket = Arc::clone(socket);
    let ask_state = state.clone();
    let ask_task_id = task_id.clone();
    let ask_hook: miniagent_loop_pipeline::ClarifyHook = Arc::new(move |question, options| {
        let socket = Arc::clone(&ask_socket);
        let state = ask_state.clone();
        let task_id = ask_task_id.clone();
        Box::pin(async move {
            let opts: Vec<&str> = options.iter().map(|s| s.as_str()).collect();
            ask_user(&socket, &state, &task_id, &question, &opts).await
        })
    });

    // P5 跨模式会话记忆：新 loop 任务也能检索相关历史经验。注入顺序
    // 必须是"任务在前、背景在后"——记忆块放在任务前面会被规划器当成
    // 任务本身（live: task_1 目录名变成了"相关历史经验_来自以往任务"，
    // 真实任务被挤成背景）。措辞亦改为纯参考语气，不含可执行的
    // 元指令。
    let memory_block = {
        let recalled = state.recall_related(&prompt, 2);
        if recalled.is_empty() {
            String::new()
        } else {
            println!("   🧠 loop recalled {} related memory item(s)", recalled.len());
            format!(
                "\n## 背景参考（来自以往任务的记忆，仅供延续参考——不是本轮任务，不要针对它制定子任务）\n{}\n",
                recalled.join("\n")
            )
        }
    };
    let run_prompt = if memory_block.is_empty() {
        prompt.clone()
    } else {
        // 任务在前、记忆作后置附录（附录已明确标注"不是本轮任务"）
        format!("{prompt}{memory_block}")
    };

    // P3 执行中转向：pipeline 在每轮循环边界拉取待处理指令。
    let steer_state = state.clone();
    let steer_task_id = task_id.clone();
    let steer_hook: miniagent_loop_pipeline::SteerHook = Arc::new(move || {
        take_steers(&steer_state, &steer_task_id)
    });

    // Anchor every pipeline artifact (dispatch outputs, checkpoints, tool
    // writes) inside the task's result directory.
    let result = miniagent_loop_pipeline::LoopPipeline::run_with_clarify(
        run_prompt,
        state.config.clone(),
        max_loops,
        cancel.clone(),
        Some(on_progress),
        Some(task_dir.clone()),
        Some(ask_hook),
        Some(steer_hook),
    ).await;

    state.cancels.remove(&task_id);
    // Release the per-task event sender FIRST: dropping it closes the
    // agent-event broadcast channel, which lets the forwarding task (holding
    // a progress_tx clone) exit, which closes the progress channel so the
    // drain task can finish. Awaiting the drain BEFORE dropping the guard
    // deadlocks the three tasks in a cycle (drain ← forwarder ← guard) and
    // the run hangs forever after the pipeline finishes: no stream/complete
    // ever reaches the frontend and the task stays "running" (live-verified).
    state.event_guards.remove(&task_id);
    // Drop the sender so the drain task exits when the channel empties.
    drop(progress_tx);
    // Bounded wait: the drain normally drains in milliseconds once the
    // channel closes; don't let a straggler block finalization.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), drain_handle).await;

    match result {
        Ok(pipeline_state) => {
            // The plan is normally shipped by `drain_progress_channel` the
            // moment the plan stage completes (data.plan_tasks). This is the
            // fallback for degraded runs where the plan event never fired.
            if let Some(ref plan) = pipeline_state.plan
                && let Some(mut task) = state.tasks.get_mut(&task_id)
                && task.plan.is_none() {
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
                let plan_json = serde_json::json!({
                    "workflow": "loop_pipeline",
                    "stages": stages,
                });
                task.plan = Some(plan_json.clone());
                let _ = ws_send(socket, serde_json::json!({
                    "type": "plan",
                    "task_id": task_id,
                    "workflow": "loop_pipeline",
                    "stages": stages,
                })).await;
            }

            let response_text = pipeline_state.final_output
                .clone()
                .unwrap_or_else(|| "(no final output)".to_string());
            let _ = ws_send(socket, serde_json::json!({
                "type": "stream",
                "text": response_text.clone(),
            })).await;

            // Loop artifacts live directly under task_dir (tasks/, dispatch
            // summaries, checkpoints) — pass task_dir as the workflow dir so
            // finalize_task's recursive collector lists them all.
            finalize_task(socket, state, &task_id, &task_brief, &task_dir, &task_dir, &["loop_pipeline".to_string()], response_text, "loop").await;
        }
        Err(e) => {
            if let Some(mut task) = state.tasks.get_mut(&task_id) {
                task.status = "failed".into();
            }
            let _ = ws_send(socket, serde_json::json!({
                "type": "error",
                "message": format!("Loop pipeline failed: {e}"),
            })).await;
        }
    }
}

/// Research pipeline mode: the full goals-1-4 pipeline (literature → KG →
/// link prediction → pathogenesis hypotheses → evidence debate → validation
/// plans → executable notebook analysis), driven by the exact same
/// `miniagent_research::run_research` code path the CLI uses. All artifacts
/// (papers.json, kg.json, hypotheses, debate report, plans, analysis/
/// notebooks, project.json audit manifest) land inside the task's
/// `result/{id}_{brief}` directory.
async fn handle_research_run(
    socket: &Arc<Mutex<WsSink>>,
    state: &AppState,
    prompt: String,
    existing_task_id: Option<String>,
) {
    use miniagent_research::{ResearchOptions, ResearchProgress};

    if let Err(e) = state.config.require_active_key() {
        let _ = ws_send(socket, serde_json::json!({
            "type": "error", "message": format!("FATAL: {e}"),
        })).await;
        return;
    }

    let (task_id, task_brief, task_dir, _task_workflow_dir) =
        if let Some(ref existing_id) = existing_task_id {
            // Read-then-drop before the get_mut below (same-shard guard rules).
            let reused = state.tasks.get(existing_id).map(|task| {
                (task.brief.clone(), task.result_dir.clone())
            });
            if let Some((brief, dir)) = reused {
                if let Some(mut t) = state.tasks.get_mut(existing_id) {
                    t.status = "running".into();
                    // Follow-up: record the user message (workflow mode did
                    // this; research previously lost it on redraws).
                    t.messages.push(serde_json::json!({"role": "user", "content": &prompt}));
                }
                (existing_id.clone(), brief, dir.clone(), dir)
            } else {
                create_new_task(state, &prompt)
            }
        } else {
            create_new_task(state, &prompt)
        };

    let _ = ws_send(socket, serde_json::json!({
        "type": "task_started", "task_id": &task_id,
    })).await;

    let cancel = CancellationToken::new();
    state.cancels.insert(task_id.clone(), cancel.clone());

    // Fixed 7-phase plan — the pipeline emits matching stage keys.
    let plan_stages: Vec<serde_json::Value> = vec![
        serde_json::json!({ "name": "literature", "handler": "research",
            "description": "文献检索：查询翻译 → PubMed 检索 → 摘要获取 → 相关性过滤",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "kg_build", "handler": "research",
            "description": "知识图谱：实体/关系抽取 + canonical 合并",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "link_prediction", "handler": "research",
            "description": "链接预测：TransE 嵌入 + 疾病锚定候选外推",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "hypotheses", "handler": "research",
            "description": "致病机理假说生成（候选验证 + 机制解释）与排序",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "debate", "handler": "research",
            "description": "假说辩论：证据-矛盾交锋（外部文献证据注入）+ 精炼",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "validation_plans", "handler": "research",
            "description": "验证计划：数据分析任务（GEO 数据集落地）+ 湿实验方案",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "analysis", "handler": "research",
            "description": "端到端数据分析：GEO 下载 → 可复现 .ipynb 执行 + 溯源记录",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
        serde_json::json!({ "name": "review", "handler": "research",
            "description": "报告审核验证：机械校验 + LLM 结构化审核（report_review.json）",
            "sub_tasks": [], "tools": serde_json::json!([]) }),
    ];
    if let Some(mut task) = state.tasks.get_mut(&task_id) {
        task.plan = Some(serde_json::json!({
            "workflow": "research",
            "stages": plan_stages,
        }));
    }
    let _ = ws_send(socket, serde_json::json!({
        "type": "plan",
        "task_id": &task_id,
        "workflow": "research",
        "stages": plan_stages,
    })).await;

    let _ = ws_send(socket, serde_json::json!({
        "type": "status",
        "message": format!("Starting research pipeline (artifacts → {}): {}", task_dir.display(), task_brief),
    })).await;

    // Web-mode defaults: the full goals-1-4 experience — debate + validation
    // plans + notebook analysis are all on.
    let opts = ResearchOptions {
        // No fixed corpus preset: the LLM requirement-extraction pass derives
        // the size from the request's own scope language.
        max_papers: None,
        validate: true,
        analyze: true,
        debate: true,
        ..Default::default()
    };

    // Progress bridge: pipeline phase callbacks → WS progress envelopes +
    // stage_outputs persistence. Cloned Arcs keep the closure 'static.
    let sink_cb = Arc::clone(socket);
    let state_cb = state.clone();
    let task_id_cb = task_id.clone();
    let main_rt = tokio::runtime::Handle::current();
    let on_progress: ResearchProgress = Arc::new(move |stage: &str, status: &str, detail: Option<&str>| {
        let payload = serde_json::json!({
            "type": "progress",
            "stage": stage,
            "status": status,
            "task_id": task_id_cb,
            // Phase summary rides in `data` so the frontend can render an
            // expandable per-subtask execution card (loop-mode parity).
            "data": detail.map(|d| serde_json::json!({"summary": d})),
        });
        let socket = Arc::clone(&sink_cb);
        let state = state_cb.clone();
        let stage = stage.to_string();
        let status = status.to_string();
        let detail = detail.map(|d| d.chars().take(400).collect::<String>());
        let task_id_key = task_id_cb.clone();
        // Spawn via the captured server-runtime handle: the callback runs on
        // the pipeline's dedicated thread, `Handle::current` there would fail.
        main_rt.spawn(async move {
            let _ = ws_send(&socket, payload).await;
            if status == "completed"
                && let Some(mut task) = state.tasks.get_mut(&*task_id_key) {
                task.stage_outputs.push(serde_json::json!({
                    "stage": stage,
                    "summary": { "response_preview": detail.unwrap_or_else(|| format!("{stage} phase completed")) },
                }));
            }
        });
    });

    // Run the pipeline on a dedicated blocking thread driving its own
    // current-thread runtime: the root future of `block_on` needs no `Send`
    // (rustc's higher-ranked Send check rejects parts of the pipeline chain),
    // thread panics surface through the JoinHandle, and the outer timeout
    // bounds a stalled run.
    let run_dir = task_dir.clone();
    let run_opts = opts.clone();
    let run_config = state.config.clone();
    let run_cb = on_progress.clone();
    // Interactive clarify channel: the loop orchestrator's Clarify step asks
    // the user through the WS ask/reply protocol (5-min timeout ⇒ assumption
    // noted, run continues — same semantics as workflow-mode asks).
    let ask_socket = Arc::clone(socket);
    let ask_state = state.clone();
    let ask_task_id = task_id.clone();
    let ask_hook: miniagent_loop_pipeline::ClarifyHook = Arc::new(move |question, options| {
        let socket = Arc::clone(&ask_socket);
        let state = ask_state.clone();
        let task_id = ask_task_id.clone();
        Box::pin(async move {
            let opts: Vec<&str> = options.iter().map(|s| s.as_str()).collect();
            ask_user(&socket, &state, &task_id, &question, &opts).await
        })
    });
    let steer_state2 = state.clone();
    let steer_task_id2 = task_id.clone();
    let steer_hook: miniagent_loop_pipeline::SteerHook = Arc::new(move || {
        take_steers(&steer_state2, &steer_task_id2)
    });

    // P5 跨模式会话记忆：research 新任务注入相关历史经验。与 loop 相同，
    // 记忆块必须放在任务之后（否则翻译/需求提取会把记忆文本当作任务）。
    let research_memory = {
        let recalled = state.recall_related(&prompt, 2);
        if recalled.is_empty() {
            String::new()
        } else {
            println!("   🧠 research recalled {} related memory item(s)", recalled.len());
            format!(
                "\n## 背景参考（来自以往任务的记忆，仅供延续参考——不是本轮任务，不要针对它制定计划）\n{}\n",
                recalled.join("\n")
            )
        }
    };
    let run_prompt = if research_memory.is_empty() {
        prompt.clone()
    } else {
        format!("{prompt}{research_memory}")
    };

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let join = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        // Research × Loop: every phase (文献检索/KG/链路预测/假说/辩论/验证/
        // 分析/审核) runs as a loop subtask with three-way adjudication.
        let summary = rt.block_on(miniagent_research::run_research_in_loop(
            run_prompt, run_dir, run_opts, run_config, Some(run_cb), Some(ask_hook), Some(steer_hook), true,
        ));
        let _ = tx.send(summary);
        Ok::<(), String>(())
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(3600), join).await;

    state.cancels.remove(&task_id);

    let summary = match result {
        Ok(Ok(Ok(()))) => match rx.await {
            Ok(summary) => summary,
            Err(_) => String::new(),
        },
        Ok(Ok(Err(e))) => {
            // runtime build failure inside the thread
            let _ = ws_send(socket, serde_json::json!({
                "type": "error", "task_id": &task_id, "message": format!("research runtime error: {e}"),
            })).await;
            if let Some(mut t) = state.tasks.get_mut(&task_id) {
                t.status = "failed".into();
            }
            let _ = ws_send(socket, serde_json::json!({
                "type": "complete", "task_id": &task_id, "status": "failed", "files": [],
            })).await;
            return;
        }
        Ok(Err(panic)) => {
            let msg = format!("research pipeline thread failed: {panic}");
            tracing::error!(task_id = %task_id, "{msg}");
            let _ = ws_send(socket, serde_json::json!({
                "type": "error", "task_id": &task_id, "message": msg,
            })).await;
            if let Some(mut t) = state.tasks.get_mut(&task_id) {
                t.status = "failed".into();
            }
            let _ = ws_send(socket, serde_json::json!({
                "type": "complete", "task_id": &task_id, "status": "failed", "files": [],
            })).await;
            return;
        }
        Err(_) => {
            let msg = "research pipeline timed out (60 min)".to_string();
            let _ = ws_send(socket, serde_json::json!({
                "type": "error", "task_id": &task_id, "message": msg,
            })).await;
            if let Some(mut t) = state.tasks.get_mut(&task_id) {
                t.status = "failed".into();
            }
            let _ = ws_send(socket, serde_json::json!({
                "type": "complete", "task_id": &task_id, "status": "failed", "files": [],
            })).await;
            return;
        }
    };

    if summary.trim().is_empty() {
        // Stage-validation gates abort with an empty summary — surface it.
        if let Some(mut t) = state.tasks.get_mut(&task_id) {
            t.status = "failed".into();
        }
        let _ = ws_send(socket, serde_json::json!({
            "type": "error", "task_id": &task_id,
            "message": "research pipeline aborted (stage validation failed — see server logs / project.json event log)",
        })).await;
        let _ = ws_send(socket, serde_json::json!({
            "type": "complete", "task_id": &task_id, "status": "failed", "files": [],
        })).await;
        return;
    }

    let _ = ws_send(socket, serde_json::json!({
        "type": "stream",
        "text": summary.clone(),
    })).await;

    // Artifacts live directly under task_dir — pass it as the workflow dir so
    // finalize_task's recursive collector lists hypotheses/plans/analysis.
    finalize_task(socket, state, &task_id, &task_brief, &task_dir, &task_dir, &["research".to_string()], summary, "research").await;
}

/// Drain `ProgressMsg` items into WebSocket envelopes and persist
/// `agent_event`s into `task.event_log`. Mirrors the loop body of
/// `run_with_progress` but is driver-agnostic so the loop-pipeline can reuse
/// it without depending on `Workflow::run_with_progress`.
async fn drain_progress_channel(
    socket: Arc<Mutex<WsSink>>,
    state: AppState,
    task_id: String,
    _task_brief: String,
    task_dir: PathBuf,
    progress_rx: &mut tokio::sync::mpsc::Receiver<ProgressMsg>,
) {
    while let Some(msg) = progress_rx.recv().await {
        match msg {
            ProgressMsg::Stage { name, status, data } => {
                // Early plan shipping: the loop pipeline attaches the freshly
                // decomposed task list to the plan stage's completed event, so
                // the frontend renders the pill strip before dispatch starts
                // (workflow-mode parity) and the plan survives restarts via
                // metadata.json.
                if name == "plan" && status == "completed"
                    && let Some(ref d) = data
                    && let Some(tasks) = d.get("plan_tasks").and_then(|v| v.as_array())
                    && !tasks.is_empty() {
                    let stages: Vec<serde_json::Value> = tasks.iter().map(|t| {
                        serde_json::json!({
                            "name": t.get("id").cloned().unwrap_or(serde_json::json!("task")),
                            "handler": t.get("handler").cloned().unwrap_or(serde_json::json!("executor")),
                            "tier": t.get("tier").cloned().unwrap_or(serde_json::json!("medium")),
                            "description": t.get("description").cloned().unwrap_or(serde_json::json!("")),
                            "sub_tasks": t.get("sub_tasks").cloned().unwrap_or(serde_json::json!([])),
                            "tools": serde_json::json!([]),
                        })
                    }).collect();
                    if let Some(mut task) = state.tasks.get_mut(&task_id) {
                        task.plan = Some(serde_json::json!({
                            "workflow": "loop_pipeline",
                            "stages": stages,
                        }));
                    }
                    let _ = ws_send(&socket, serde_json::json!({
                        "type": "plan",
                        "task_id": task_id,
                        "workflow": "loop_pipeline",
                        "stages": stages,
                    })).await;
                }
                let _ = ws_send(&socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": status,
                    "task_id": task_id,
                    "data": data,
                })).await;
                if status == "completed"
                    && let Some(ref d) = data
                    && name != "task"   // per-subtask events persist below, not as stage outputs
                {
                    let summary = serde_json::json!({
                        "response_preview": d.get("summary").and_then(|v| v.as_str()).unwrap_or("").chars().take(200).collect::<String>(),
                        "tokens_in": 0, "tokens_out": 0,
                        "tool_count": 0, "tool_entries": [],
                    });
                    if let Some(mut task) = state.tasks.get_mut(&task_id) {
                        task.stage_outputs.push(serde_json::json!({
                            "stage": name,
                            "summary": summary,
                        }));
                    }
                }
                // Per-subtask events (stage == "task"): persist as "subtask"
                // stage_outputs so the frontend can restore the todo list and
                // per-subtask execution summaries after a page reload.
                if name == "task"
                    && (status == "completed" || status == "failed")
                    && let Some(ref d) = data
                    && let Some(mut task) = state.tasks.get_mut(&task_id)
                {
                    task.stage_outputs.push(serde_json::json!({
                        "stage": "subtask",
                        "summary": {
                            "task_id": d.get("task_id").cloned().unwrap_or(serde_json::json!("")),
                            "title": d.get("title").cloned().unwrap_or(serde_json::json!("")),
                            "role": d.get("role").cloned().unwrap_or(serde_json::json!("")),
                            "status": status,
                            "reused": d.get("reused").cloned().unwrap_or(serde_json::json!(false)),
                            "response_preview": d.get("output").and_then(|v| v.as_str()).unwrap_or("").chars().take(600).collect::<String>(),
                            "error": d.get("error").cloned().unwrap_or(serde_json::json!(null)),
                            "tokens_used": d.get("tokens_used").cloned().unwrap_or(serde_json::json!(0)),
                        },
                    }));
                }
            }
            ProgressMsg::AgentEvent(event) => {
                let event_json = serde_json::to_value(&event).unwrap_or_else(|_| serde_json::json!({}));
                let traced = serde_json::json!({
                    "ts": chrono::Utc::now().to_rfc3339(),
                    "event": event_json,
                });
                if let Some(mut task) = state.tasks.get_mut(&task_id) {
                    task.event_log.push(traced.clone());
                }
                // Append-only audit trail on disk (goal: traceability). The
                // in-memory event_log is lost on restart; the JSONL survives.
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(task_dir.join("event_log.jsonl"))
                {
                    let _ = writeln!(f, "{traced}");
                }
                let _ = ws_send(&socket, serde_json::json!({
                    "type": "agent_event",
                    "task_id": task_id,
                    "event": event_json,
                })).await;
            }
            ProgressMsg::Done(_) => {
                // Loop pipeline doesn't push Done; reserved for workflow parity.
                break;
            }
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

/// 辩论模式：Proposer vs Opponent → Judge（DebateRunner），角色模型按 ⚙️
/// 设置路由（默认全部主模型）。结果按轮次渲染为 Markdown。
async fn handle_debate_run(
    socket: &Arc<Mutex<WsSink>>,
    state: &AppState,
    prompt: String,
    existing_task_id: Option<String>,
) {
    use miniagent_core::models::DebateRole;
    use miniagent_core::orchestration::{StageInput, StageDriver as _};
    use miniagent_planning::runners::{DebateRunner, DebateRound};

    // Resolve role providers once per debate: ⚙️ selection > env > main model.
    let (prop_profile, opp_profile, judge_profile) = {
        let reg = state.models.read().unwrap();
        (
            reg.role_profile(DebateRole::Proposer).clone(),
            reg.role_profile(DebateRole::Opponent).clone(),
            reg.role_profile(DebateRole::Judge).clone(),
        )
    };
    let (proposer, opponent, judge) =
        match (
            miniagent_provider::factory::resolve_role_provider_from(&prop_profile),
            miniagent_provider::factory::resolve_role_provider_from(&opp_profile),
            miniagent_provider::factory::resolve_role_provider_from(&judge_profile),
        ) {
            (Ok(p), Ok(o), Ok(j)) => (p, o, j),
            (r1, r2, r3) => {
                let e = r1.err().or(r2.err()).or(r3.err()).unwrap_or_default();
                let _ = ws_send(socket, serde_json::json!({
                    "type": "error",
                    "message": format!("辩论角色模型不可用: {e}"),
                })).await;
                return;
            }
        };

    let (task_id, task_brief, _task_dir, task_workflow_dir) =
        if let Some(ref existing_id) = existing_task_id {
            // Read-then-drop before the get_mut below (same-shard guard rules).
            let reused = state.tasks.get(existing_id).map(|task| {
                (task.brief.clone(), task.result_dir.clone())
            });
            if let Some((brief, dir)) = reused {
                // Follow-up rounds reuse the task's result dir but keep artifacts
                // under `.workflow` — same layout as a fresh task (previously the
                // first round wrote into `.workflow/` and follow-ups dumped
                // proposer/judge files straight into the result dir root).
                let wf_dir = dir.join(".workflow");
                let _ = std::fs::create_dir_all(&wf_dir);
                if let Some(mut t) = state.tasks.get_mut(existing_id) {
                    t.status = "running".into();
                    // Follow-up: record the user message (was lost on redraws).
                    t.messages.push(serde_json::json!({"role": "user", "content": &prompt}));
                }
                (existing_id.clone(), brief, dir.clone(), wf_dir)
            } else {
                create_new_task(state, &prompt)
            }
        } else {
            create_new_task(state, &prompt)
        };
    let _ = ws_send(socket, serde_json::json!({
        "type": "task_started", "task_id": &task_id,
    })).await;

    let cancel = CancellationToken::new();
    state.cancels.insert(task_id.clone(), cancel.clone());

    // Plan pills for the progress panel (workflow/loop modes emit one too, so
    // the debate mode no longer renders an empty "No workflow yet" panel).
    {
        let plan_stages = vec![
            serde_json::json!({
                "name": "proposer", "handler": "debate",
                "description": "正方：构建论证（提出/修改假说）",
                "sub_tasks": [], "tools": serde_json::json!([]),
            }),
            serde_json::json!({
                "name": "opponent", "handler": "debate",
                "description": "反方：证据-矛盾反驳与交叉检验",
                "sub_tasks": [], "tools": serde_json::json!([]),
            }),
            serde_json::json!({
                "name": "judge", "handler": "debate",
                "description": "裁判：ACCEPT / REJECT / REVISE 裁决",
                "sub_tasks": [], "tools": serde_json::json!([]),
            }),
        ];
        if let Some(mut task) = state.tasks.get_mut(&task_id) {
            task.plan = Some(serde_json::json!({
                "workflow": "debate",
                "stages": plan_stages,
            }));
        }
        let _ = ws_send(socket, serde_json::json!({
            "type": "plan",
            "task_id": &task_id,
            "workflow": "debate",
            "stages": plan_stages,
        })).await;
    }

    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "debate", "status": "running", "task_id": &task_id,
        "detail": format!(
            "正方: {} | 反方: {} | 裁判: {}",
            prop_profile.display_name, opp_profile.display_name, judge_profile.display_name
        ),
    })).await;

    let runner = DebateRunner::new(proposer, opponent, judge, task_workflow_dir.clone())
        .with_max_revise_rounds(2);
    let input = StageInput::new("debate", serde_json::json!(prompt), cancel.clone());
    // Overall guard: role HTTP calls have no per-call timeout, so a stalled
    // connection would hang the debate forever. 10 minutes covers a full
    // 3-round debate on a slow reasoning model.
    let outcome = match tokio::time::timeout(
        std::time::Duration::from_secs(600),
        runner.run(input),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => {
            state.cancels.remove(&task_id);
            let _ = ws_send(socket, serde_json::json!({
                "type": "error", "task_id": &task_id,
                "message": "辩论超时（10 分钟）— 可能是模型端点无响应，请重试或切换角色模型",
            })).await;
            if let Some(mut t) = state.tasks.get_mut(&task_id) {
                t.status = "failed".into();
            }
            return;
        }
    };
    state.cancels.remove(&task_id);

    let response_text = match outcome {
        Ok(out) => {
            let mut md = String::new();
            if let Ok(rounds) = serde_json::from_value::<Vec<DebateRound>>(out.data.clone()) {
                for r in &rounds {
                    md.push_str(&format!(
                        "\n## 第 {} 轮（裁决：{}）\n\n### 📝 正方 Proposer\n{}\n\n### ⚔️ 反方 Opponent\n{}\n\n### ⚖️ 裁判 Judge\n{}\n\n---\n",
                        r.round, r.verdict, r.proposer.content, r.opponent.content, r.judge.content
                    ));
                }
            }
            if md.is_empty() {
                md = out.summary;
            } else {
                md.push_str(&format!("\n{}\n", out.summary));
            }
            md
        }
        Err(e) => {
            let _ = ws_send(socket, serde_json::json!({
                "type": "error", "task_id": &task_id,
                "message": format!("辩论失败: {e}"),
            })).await;
            if let Some(mut t) = state.tasks.get_mut(&task_id) {
                t.status = "failed".into();
            }
            return;
        }
    };

    // Stream the rendered transcript so the chat pane shows it live.
    for chunk in response_text.as_bytes().chunks(2048) {
        let text = String::from_utf8_lossy(chunk).to_string();
        let _ = ws_send(socket, serde_json::json!({
            "type": "stream", "task_id": &task_id, "text": text,
        })).await;
    }

    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "debate", "status": "done", "task_id": &task_id,
    })).await;
    finalize_task(socket, state, &task_id, &task_brief, &_task_dir, &task_workflow_dir, &[], response_text, "debate").await;
}

async fn handle_run(
    socket: &Arc<Mutex<WsSink>>,
    state: &AppState,
    prompt: String,
    file_ids: Vec<String>,
    existing_task_id: Option<String>,
) {
    eprintln!("🔍 handle_run enter | follow_up={existing_task_id:?} | prompt={}", prompt.chars().take(40).collect::<String>());
    // Resolve the active model profile once per task; every stage provider
    // is built from it (no hardcoded model names anywhere).
    let active_profile = state.models.read().unwrap().active().clone();
    let (_flash_provider, pro_provider) = match build_provider_pair(&active_profile) {
        Ok(p) => p,
        Err(e) => {
            let _ = ws_send(socket, serde_json::json!({
                "type": "error",
                "message": format!("当前模型配置不可用: {e}"),
            })).await;
            return;
        }
    };
    let agent_arc = state.agent.clone();

    let (task_id, task_brief, task_dir, task_workflow_dir) =
        if let Some(ref existing_id) = existing_task_id {
            // Follow-up message in existing conversation. Read-then-drop:
            // DashMap shard guards must be released BEFORE the get_mut below —
            // holding the read guard while requesting the write guard on the
            // same shard self-deadlocks the task (live: follow-up hung forever
            // with status stuck on the previous turn's value).
            let reused = state.tasks.get(existing_id).map(|task| {
                (task.brief.clone(), task.result_dir.clone())
            });
            if let Some((brief, dir)) = reused {
                // Each follow-up gets its own timestamped sub-directory, keeping all history
                let turn_ts = chrono::Utc::now().format("turn_%Y%m%d_%H%M%S").to_string();
                let wf_dir = dir.join(&turn_ts);
                let _ = std::fs::create_dir_all(&wf_dir);
                // Append user message to history (guards dropped — safe to write)
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

    // P1 多轮交互：follow-up 时把上一轮交流与产物清单注入本轮上下文，
    // 修复"逐轮失忆"断层（此前 task.messages 仅用于展示，agent 从零开始）。
    // effective_prompt 随后贯穿 explore / planner / workflow / feedback。
    let (followup_context, _goal_only) = build_followup_context(state, &task_id, &task_dir, &prompt);
    let memory_block = String::new();

    // P2: 会话级 goal_state —— LLM 提取本轮约束增量 → 合并持久化 →
    // 注入本轮执行上下文（跨轮继承；"改成只看 2024 年"这类转向无需复述）。
    let goal_block = {
        let providers: Vec<std::sync::Arc<dyn LlmProvider>> = {
            let (f, p) = match build_provider_pair(&active_profile) {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = ws_send(socket, serde_json::json!({
                        "type": "error",
                        "message": format!("当前模型配置不可用: {e}"),
                    })).await;
                    return;
                }
            };
            vec![f, p]
        };
        let prev = load_goal_state(&task_dir);
        let prev_list: Vec<String> = prev.as_ref()
            .map(|g| g.constraints.iter().map(|c| c.text.clone()).collect())
            .unwrap_or_default();
        let extracted = extract_goal_constraints(&prev_list, &prompt, &providers, tokio_util::sync::CancellationToken::new()).await;
        let turn_source = format!("turn_{}", state.tasks.get(&task_id).map(|t| t.messages.len()).unwrap_or(1).max(1));
        let gs = merge_goal_state(prev, &prompt, extracted, &turn_source);
        match persist_goal_state(&task_dir, &gs) {
            Ok(block) => {
                manifest_log_goal_state(state, &task_id, &gs);
                println!("   🎯 goal_state v{}: {} constraint(s)", gs.version, gs.constraints.len());
                block
            }
            Err(e) => {
                tracing::warn!(error = %e, "session.json persist failed — continuing without goal_state");
                String::new()
            }
        }
    };

    let mut context_parts: Vec<String> = Vec::new();
    if !goal_block.is_empty() {
        context_parts.push(goal_block.clone());
    }
    if !followup_context.is_empty() {
        context_parts.push(followup_context.clone());
    }
    if !memory_block.is_empty() {
        context_parts.push(memory_block.clone());
    }
    let effective_prompt = if context_parts.is_empty() {
        enriched_prompt.clone()
    } else {
        println!(
            "   💬 follow-up: context rebuilt ({} block(s), {} chars)",
            context_parts.len(),
            context_parts.iter().map(|s| s.len()).sum::<usize>()
        );
        format!(
            "{}\n## 本轮请求\n{}",
            context_parts.join("\n"),
            enriched_prompt
        )
    };

    // ── ExploreStage（需求1：明确问题+获取上下文）──────────────────
    // task_started 与 loop/research 模式对齐：前端在 run 中途依赖它建立
    // currentTaskId（缺失时 follow-up 无法携带 task_id，每轮变成独立任务
    // ——live 复现的"逐轮失忆"直接诱因之一）。
    let _ = ws_send(socket, serde_json::json!({
        "type": "task_started", "task_id": &task_id,
    })).await;
    let _ = ws_send(socket, serde_json::json!({
        "type": "progress", "stage": "explore", "status": "running",
        "task_id": &task_id,
    })).await;

    // Explore：用 planner provider 分析问题，提取关键上下文
    let explore_provider: Box<dyn LlmProvider> =
        build_provider(&active_profile, ProviderTier::Flash).expect("validated above");
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
            messages: vec![miniagent_core::message::Message::user(&effective_prompt)],
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
    let planner_flash: Box<dyn LlmProvider> =
        build_provider(&active_profile, ProviderTier::Flash).expect("validated above");
    let planner = PlannerStage::new(planner_flash);
    let plan_ctx = StageContext::new(
        StageId::new(),
        serde_json::json!({ "prompt": effective_prompt }),
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
        "task_id": task_id,
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
        .with_task_dir(task_dir.to_string_lossy());

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
        miniagent_core::context_info::env_block(&task_dir.to_string_lossy()),
        miniagent_core::context_info::project_md_block(&task_dir.to_string_lossy())
            .map(|s| format!("\n\n{s}")).unwrap_or_default()
    );

    // Use `match` (not `unwrap_or_else`) so the WS fallback send can be
    // awaited — a sync closure can't `.await`, which is what previously caused
    // the status future to be dropped silently.
    let workflow = match builder.build(&spec, &effective_prompt, &system_prompt) {
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
                .with_task_dir(task_dir.to_string_lossy())
                .build(&fallback, &effective_prompt, &system_prompt)
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
            socket, workflow, &spec, &task_workflow_dir, pro_provider.clone(),
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
    let feedback_provider: Box<dyn LlmProvider> =
        build_provider(&active_profile, ProviderTier::Flash).expect("validated above");

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
        &*feedback_provider, &effective_prompt, &stage_outputs_snapshot,
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
        let result = workflow.run_with_progress(wf_cancel, Box::new(progress_fn)).await;
        let _ = progress_tx.send(ProgressMsg::Done(result)).await;
    });

    // Wire the agent's broadcast sender so tool/skill events flow to the frontend.
    // Register a per-task sender (RAII guard) so concurrent workflow runs do
    // not clobber each other's event streams.
    let event_guard = agent_arc.register_event_sender(agent_event_tx.clone()).await;
    state.event_guards.insert(task_id.to_string(), event_guard);

    // Forward progress to WebSocket and wait for result
    let mut final_result: Option<Result<miniagent_workflow::engine::WorkflowResult, miniagent_core::error::AgentError>> = None;
    while let Some(msg) = progress_rx.recv().await {
        match msg {
            ProgressMsg::Stage { name, status, data } => {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": status,
                    "task_id": task_id,
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
                    "task_id": task_id,
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

            finalize_task(socket, state, task_id, task_brief, task_dir, task_workflow_dir, &stage_names, response_text, "workflow").await;
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
    pro_provider: std::sync::Arc<dyn LlmProvider>,
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
        let result = workflow.run_with_progress(wf_cancel, Box::new(progress_fn)).await;
        let _ = progress_tx.send(ProgressMsg::Done(result)).await;
    });

    // Wire the agent's broadcast sender so tool/skill events flow to the frontend.
    // Register a per-task sender (RAII guard) so concurrent workflow runs do
    // not clobber each other's event streams.
    let event_guard = agent_arc.register_event_sender(agent_event_tx.clone()).await;
    state.event_guards.insert(task_id.to_string(), event_guard);

    // Forward progress to WebSocket and wait for result
    let mut final_result: Option<Result<miniagent_workflow::engine::WorkflowResult, miniagent_core::error::AgentError>> = None;
    while let Some(msg) = progress_rx.recv().await {
        match msg {
            ProgressMsg::Stage { name, status, data } => {
                let _ = ws_send(socket, serde_json::json!({
                    "type": "progress",
                    "stage": name,
                    "status": status,
                    "task_id": task_id,
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
                    "task_id": task_id,
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
                let stream_result = stream_synthesis(socket, pro_provider.clone(), &response_text, cancel).await;
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

            finalize_task(socket, state, task_id, task_brief, task_dir, task_workflow_dir, &stage_names, final_text, "workflow").await;
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
    pro_provider: std::sync::Arc<dyn LlmProvider>,
    synthesis_text: &str,
    cancel: CancellationToken,
) -> bool {
    let pro = pro_provider;
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
    stage_names: &[String],
    response_text: String,
    mode: &str,
) {
    // Release the per-task broadcast sender so the shared Agent's event
    // list does not grow without bound and other concurrent runs are not
    // shadowed by this task's now-defunct sender.
    state.event_guards.remove(task_id);
    // Save the user-facing final report at `<task_dir>/<brief>.md`.
    // For workflow / loop / debate modes the AI's streamed response IS the
    // report (with a brief cover header). For research mode the research
    // pipeline writes its own (richer) `<brief>.md` first; we don't
    // overwrite it here.
    let output_filename = format!("{}.md", task_brief);
    let output_path = task_dir.join(&output_filename);
    let is_research = mode == "research";
    let mut result_files = vec![];
    if !response_text.is_empty() && !is_research {
        // Brief cover + AI body. Keeps the file self-contained even if the
        // user opens it before the in-app chat history.
        let stages_line = if stage_names.is_empty() {
            String::new()
        } else {
            format!("\n**执行阶段**: {}\n", stage_names.join(" → "))
        };
        let cover = format!(
            "# {brief} · 最终报告\n\n\
             - **任务 ID**: `{task_id}`\n\
             - **生成时间**: {ts}\n\
             - **结果目录**: `{dir}`\n{stages_line}\n\
             ---\n\n",
            brief = task_brief,
            task_id = task_id,
            ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            dir = task_dir.display(),
            stages_line = stages_line,
        );
        let composed = format!("{cover}{body}", cover = cover, body = response_text);
        if std::fs::write(&output_path, &composed).is_ok() {
            result_files.push(output_filename.clone());
        }
    } else if !is_research {
        // No AI response — at least leave a stub so the directory clearly
        // shows the task was finalized.
        let stub = format!(
            "# {brief}\n\n（任务已结束，未生成 AI 回复文本；详见 `metadata.json` 与右侧 Progress / Files 面板。）\n",
            brief = task_brief,
        );
        let _ = std::fs::write(&output_path, &stub);
        result_files.push(output_filename.clone());
    } else if is_research && !output_path.exists() {
        // The research pipeline owns the final report; if it somehow failed
        // to write one, leave a pointer stub so the run directory is never
        // silently report-less.
        let stub = format!(
            "# {brief}\n\n（研究管线未能生成完整报告；执行轨迹见 `run_report.md` 与 `project.json`。）\n",
            brief = task_brief,
        );
        let _ = std::fs::write(&output_path, &stub);
        result_files.push(output_filename.clone());
    }

    // List workflow artifacts as paths relative to task_dir (the task's result_dir).
    // Previously these were flattened to bare filenames, which 404'd under the old
    // single-segment download route; the new catch-all route supports nested paths.
    // The walk is recursive (bounded) so loop-pipeline `tasks/…` and research
    // `analysis/…` subdirectory artifacts are listed too.
    fn collect_artifacts(dir: &StdPath, task_dir: &StdPath, depth: usize, out: &mut Vec<String>) {
        if depth > 4 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "event_log.jsonl" {
                continue;
            }
            if ft.is_dir() {
                collect_artifacts(&path, task_dir, depth + 1, out);
            } else if name.ends_with(".md") || name.ends_with(".json") || name.ends_with(".ipynb") {
                if let Ok(rel) = path.strip_prefix(task_dir) {
                    out.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    let mut artifact_files = Vec::new();
    collect_artifacts(task_workflow_dir, task_dir, 0, &mut artifact_files);
    result_files.extend(artifact_files);

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

    // P5 跨会话记忆：完成的任务作为可检索经验写入 L1 情景记忆。
    state.remember_task(task_id, task_brief, &response_text);

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

/// P3: take all pending steering instructions for a task (drain the queue).
pub fn take_steers(state: &AppState, task_id: &str) -> Vec<String> {
    state
        .steers
        .remove(task_id)
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// Record a goal_state read (context rebuild) in the task audit log.
fn manifest_log_goal_state_read(state: &AppState, task_id: &str, gs: &GoalState) {
    if let Some(mut t) = state.tasks.get_mut(task_id) {
        t.event_log.push(serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "event": {
                "type": "goal_state_recalled",
                "version": gs.version,
                "constraints": gs.constraints.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
            }
        }));
    }
}

/// Append a goal_state change event to the task's audit log.
fn manifest_log_goal_state(state: &AppState, task_id: &str, gs: &GoalState) {
    if let Some(mut t) = state.tasks.get_mut(task_id) {
        t.event_log.push(serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "event": {
                "type": "goal_state_updated",
                "version": gs.version,
                "constraints": gs.constraints.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
            }
        }));
    }
}


/// P-记忆机制 Layer C：上下文重建器。
///
/// 复杂任务的多轮延续不再由各 handler 手工拼块，而是统一从这里按需
/// 重建执行上下文。四块内容全部来自结构化外存（session.json 的
/// goal_state、task.messages 的最近交流、task.result_dir 的产物清单、
/// memory.db 的历史经验），符合"状态即数据、轨迹是源数据"的审计原则。
///
/// 返回 None 表示无可注入内容（全新任务且无记忆命中）。
fn build_followup_context(
    state: &AppState,
    task_id: &str,
    task_dir: &StdPath,
    prompt: &str,
) -> (String, String) {
    // (injection_block, goal_state_only_block) —— 后者供不需要完整
    // 上下文的调用方（如仅注入约束）使用。
    let mut parts: Vec<String> = Vec::new();

    // ── 1. goal_state（P2）──
    let goal_block = match load_goal_state(task_dir) {
        Some(gs) => {
            manifest_log_goal_state_read(state, task_id, &gs);
            render_goal_state(&gs)
        }
        None => String::new(),
    };
    if !goal_block.is_empty() {
        parts.push(goal_block);
    }

    // ── 2. 最近交流 + 产物清单（P1）──
    if let Some(task) = state.tasks.get(task_id)
        && !task.messages.is_empty()
    {
        let prior = build_prior_context(&task.messages, task_dir);
        if !prior.is_empty() {
            parts.push(prior);
        }
    }

    // ── 3. 跨会话记忆召回（P5）──
    let recalled = state.recall_related(prompt, 2);
    if !recalled.is_empty() {
        parts.push(format!(
            "## 相关历史经验（来自以往任务，供参考）\n{}\n",
            recalled.join("\n")
        ));
    }

    if parts.is_empty() {
        (String::new(), String::new())
    } else {
        (parts.join("\n"), parts.join("\n"))
    }
}

/// Build a compact "prior turns" context block for a follow-up run in an
/// Build a compact "prior turns" context block for a follow-up run in an
/// existing conversation (workflow mode). Includes the last few exchanges
/// and an artifact inventory from the task's result directory, so the new
/// turn's planner and agent know what already exists — without this, every
/// follow-up started from scratch and "forgot" prior work (live-verified).
/// Bounded: recent exchanges capped, artifact walk depth-limited.

/// P2 多轮交互：会话级结构化约束（goal_state）。
///
/// 每轮运行前，用 LLM 从本轮请求 + 现有 goal_state 中提取约束增量
/// （时间窗/范围/数量/质量要求/方向决策等），合并后持久化到任务目录的
/// `session.json`。合并可审计：版本号 + 来源轮次，并写入任务事件日志。
/// goal_state 整体注入本轮执行上下文——后续轮次自动继承此前确认过的
/// 约束（"改成只看 2024 年"这类转向无需用户复述历史）。
///
/// 通用性：无关键词表、无语言假设——约束识别完全是模型判断；提取
/// 失败（LLM 不可用）时保留现有 goal_state 并继续（约束是渐进资产，
/// 不因单轮故障丢失）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GoalState {
    objective: String,
    #[serde(default)]
    constraints: Vec<GoalConstraint>,
    #[serde(default)]
    version: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GoalConstraint {
    text: String,
    source: String,
}

/// Load session.json goal_state if present.
fn load_goal_state(task_dir: &StdPath) -> Option<GoalState> {
    let raw = std::fs::read_to_string(task_dir.join("session.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Persist goal_state; returns the rendered injection block.
fn persist_goal_state(task_dir: &StdPath, gs: &GoalState) -> std::io::Result<String> {
    let json = serde_json::to_string_pretty(gs)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(task_dir.join("session.json"), json)?;
    Ok(render_goal_state(gs))
}

fn render_goal_state(gs: &GoalState) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "## 会话累积约束（goal_state v{}）\n", gs.version);
    let _ = writeln!(s, "**目标**: {}\n", gs.objective);
    if gs.constraints.is_empty() {
        s.push_str("（尚无累积约束）\n");
    } else {
        let _ = writeln!(s, "| 约束 | 来源 |\n|---|---|");
        for c in &gs.constraints {
            let _ = writeln!(s, "| {} | {} |", c.text, c.source);
        }
    }
    s
}

fn norm_constraint(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Merge this turn's extracted constraints into the goal_state (dedup by
/// normalized text; version bump per merge).
fn merge_goal_state(
    prev: Option<GoalState>,
    objective: &str,
    extracted: Vec<String>,
    turn_source: &str,
) -> GoalState {
    let mut gs = prev.unwrap_or(GoalState {
        objective: objective.to_string(),
        constraints: Vec::new(),
        version: 0,
    });
    gs.objective = objective.to_string();
    for text in extracted {
        let t = text.trim().to_string();
        if t.is_empty() {
            continue;
        }
        let nt = norm_constraint(&t);
        if !gs.constraints.iter().any(|c| norm_constraint(&c.text) == nt) {
            gs.constraints.push(GoalConstraint { text: t, source: turn_source.into() });
        }
    }
    gs.version += 1;
    gs
}

/// LLM extraction of constraint deltas for this turn.
async fn extract_goal_constraints(
    prev_constraints: &[String],
    prompt: &str,
    providers: &[std::sync::Arc<dyn LlmProvider>],
    cancel: tokio_util::sync::CancellationToken,
) -> Vec<String> {
    use miniagent_core::config::InferenceConfig;
    let prev = if prev_constraints.is_empty() {
        "（无）".to_string()
    } else {
        prev_constraints.join("; ")
    };
    let prompt = format!(
        "Existing accumulated constraints for this ongoing task:\n{prev}\n\n\
         New user message for this turn:\n{prompt}\n\n\
         Extract NEW or CHANGED constraints from the message (time window, scope, \
         quantity, quality bar, direction decisions, exclusions). Only constraints that \
         should persist across the rest of this task. Do not restate existing ones. \
         Output ONLY JSON: {{\"constraints\": [\"...\"]}}"
    );
    for provider in providers {
        let request = CompletionRequest {
            system: "You extract durable task constraints. Output ONLY valid JSON.".into(),
            messages: vec![miniagent_core::message::Message::user(&prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(4_096),
                ..Default::default()
            },
        };
        if let Ok(resp) = provider.complete(&request, cancel.child_token()).await {
            let text: String = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let repaired = miniagent_core::json_util::extract_and_repair(&text);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repaired) {
                return v["constraints"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.as_str())
                            .map(|c| c.trim().to_string())
                            .filter(|c| !c.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }
    }
    Vec::new()
}


fn build_prior_context(
    messages: &[serde_json::Value],
    result_dir: &StdPath,
) -> String {
    use std::fmt::Write as _;

    // ── Recent exchanges (last ~3 turns, each capped) ──
    let mut chat = String::new();
    let mut shown = 0usize;
    for msg in messages.iter().rev() {
        let role = msg["role"].as_str().unwrap_or("");
        let content = msg["content"].as_str().unwrap_or("");
        if content.trim().is_empty() || (role != "user" && role != "assistant") {
            continue;
        }
        let cap = if role == "assistant" { 1_200 } else { 600 };
        let body: String = content.chars().take(cap).collect();
        let body = if content.chars().count() > cap {
            format!("{body}…[截断]")
        } else {
            body
        };
        let label = if role == "user" { "用户" } else { "AI" };
        // Reverse order while collecting; assemble newest-first block below.
        chat.insert_str(0, &format!("[{label}] {body}\n\n"));
        shown += 1;
        if shown >= 6 {
            break;
        }
    }

    // ── Artifact inventory (depth 2, cap 30 files) ──
    let mut artifacts = String::new();
    let mut count = 0usize;
    fn walk(dir: &StdPath, depth: usize, out: &mut String, count: &mut usize) {
        if depth > 2 || *count >= 30 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.starts_with('.') || name == "metadata.json" {
                continue;
            }
            if p.is_dir() {
                let _ = writeln!(out, "- {name}/");
                *count += 1;
                walk(&p, depth + 1, out, count);
            } else {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = writeln!(out, "- {name} ({} bytes)", size);
                *count += 1;
            }
            if *count >= 30 {
                let _ = writeln!(out, "- …（更多文件省略）");
                return;
            }
        }
    }
    walk(result_dir, 0, &mut artifacts, &mut count);

    format!(
        "## 此前对话上下文（同一任务的延续；本轮请求见文末）\n\n### 最近的交流\n{chat}### 已有产物清单（相对任务目录）\n{artifacts}\n"
    )
}


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

// ── Helpers ──

fn create_new_task(state: &AppState, prompt: &str) -> (String, String, std::path::PathBuf, std::path::PathBuf) {
    let task_id = Uuid::new_v4().to_string()[..8].to_string();
    let task_brief = sanitize_task_brief(prompt);
    let task_dir_name = format!("{}_{}", task_id, task_brief);
    let task_dir = state.task_dir.join(&task_dir_name);
    let task_workflow_dir = task_dir.join(".workflow");
    // Fail loudly when the run's result directory cannot exist: continuing
    // would scatter artifacts relative to the process CWD (the exact class
    // of bug this centralised anchoring exists to prevent).
    if let Err(e) = std::fs::create_dir_all(&task_workflow_dir) {
        tracing::error!(path = %task_workflow_dir.display(), error = %e, "failed to create task result dir");
    }
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
    miniagent_core::paths::sanitize_task_brief(prompt)
}

// ── Model registry API（运行时 LLM 管理：列表/添加/修改/删除/切换）──

/// UI-safe view of a ModelProfile: the API key is always masked.
#[derive(Debug, Serialize)]
struct ModelProfileView {
    id: String,
    display_name: String,
    kind: String,
    kind_label: String,
    /// Short emoji glyph for the family (e.g. 🐳 / ⚡). Single source of
    /// truth — the frontend never hardcodes family icons.
    kind_icon: String,
    base_url: String,
    model_name: String,
    pro_model_name: Option<String>,
    /// "flash" | "pro" tier labels the backend suggests for tier badges. The
    /// frontend may render its own but never invents tier strings.
    flash_model_name: String,
    pro_model_name_effective: String,
    api_key_masked: String,
    has_key: bool,
    builtin: bool,
}

impl From<&ModelProfile> for ModelProfileView {
    fn from(p: &ModelProfile) -> Self {
        Self {
            id: p.id.clone(),
            display_name: p.display_name.clone(),
            kind: serde_json::to_string(&p.kind).unwrap_or_default().trim_matches('"').to_string(),
            kind_label: p.kind.label().to_string(),
            kind_icon: p.kind.icon().to_string(),
            base_url: p.base_url.clone(),
            model_name: p.model_name.clone(),
            pro_model_name: p.pro_model_name.clone(),
            flash_model_name: p.model_name.clone(),
            pro_model_name_effective: p.pro_model().to_string(),
            api_key_masked: p.masked_key(),
            has_key: p.resolve_key().is_some(),
            builtin: p.builtin,
        }
    }
}

async fn models_handler(State(state): State<AppState>) -> impl IntoResponse {
    let reg = state.models.read().unwrap();
    Json(serde_json::json!({
        "active_id": reg.active_id(),
        "models": reg.list().iter().map(|p| ModelProfileView::from(*p)).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
struct AddModelRequest {
    display_name: String,
    /// One of: deepseek | stepfun | minimax | openai_compatible | anthropic_compatible
    kind: String,
    #[serde(default)]
    base_url: String,
    model_name: String,
    #[serde(default)]
    pro_model_name: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    /// Optional env-var name to resolve the key from instead of a literal key.
    #[serde(default)]
    api_key_env: Option<String>,
}

async fn add_model_handler(
    State(state): State<AppState>,
    Json(req): Json<AddModelRequest>,
) -> Response {
    let kind = match ModelKind::from_str_loose(&req.kind) {
        Some(k) => k,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("未知模型类型: {}", req.kind)})),
            ).into_response()
        }
    };
    if req.display_name.trim().is_empty() || req.model_name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "名称和模型名不能为空"})),
        ).into_response();
    }
    if req.api_key.as_deref().map_or(true, |k| k.is_empty())
        && req.api_key_env.as_deref().map_or(true, |v| v.is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "必须提供 API key 或 key 的环境变量名"})),
        ).into_response();
    }
    let default_url = match kind {
        ModelKind::DeepSeek => "https://api.deepseek.com",
        ModelKind::StepFun => "https://api.stepfun.com/step_plan/v1",
        ModelKind::MiniMax => "https://api.minimaxi.com/v1",
        ModelKind::OpenAiCompatible | ModelKind::AnthropicCompatible => "",
    };
    let profile = ModelProfile {
        id: String::new(),
        display_name: req.display_name.trim().to_string(),
        kind,
        base_url: if req.base_url.trim().is_empty() {
            default_url.to_string()
        } else {
            req.base_url.trim().trim_end_matches('/').to_string()
        },
        model_name: req.model_name.trim().to_string(),
        pro_model_name: req.pro_model_name.filter(|s| !s.trim().is_empty()),
        api_key: req.api_key.filter(|k| !k.is_empty()),
        api_key_env: req.api_key_env.filter(|v| !v.is_empty()),
        builtin: false,
    };
    // Validate constructability before persisting.
    if let Err(e) = build_provider(&profile, ProviderTier::Flash) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response();
    }
    let mut reg = state.models.write().unwrap();
    let id = reg.add(profile);
    let view = reg.get(&id).map(ModelProfileView::from);
    (StatusCode::CREATED, Json(serde_json::json!({"id": id, "model": view}))).into_response()
}

async fn update_model_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddModelRequest>,
) -> Response {
    let kind = match ModelKind::from_str_loose(&req.kind) {
        Some(k) => k,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("未知模型类型: {}", req.kind)})),
            ).into_response()
        }
    };
    let patch = ModelProfile {
        id: id.clone(),
        display_name: req.display_name,
        kind,
        base_url: req.base_url,
        model_name: req.model_name,
        pro_model_name: req.pro_model_name,
        api_key: req.api_key,
        api_key_env: req.api_key_env,
        builtin: false,
    };
    let mut reg = state.models.write().unwrap();
    match reg.update(&id, patch) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

async fn delete_model_handler(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let mut reg = state.models.write().unwrap();
    match reg.remove(&id) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// Switch the active model: persists the selection and hot-swaps the shared
/// Agent's providers. In-flight requests finish on the old provider; new
/// requests pick up the new one immediately.
async fn activate_model_handler(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let (profile, set_result) = {
        let mut reg = state.models.write().unwrap();
        match reg.set_active(&id) {
            Ok(()) => (reg.active().clone(), ()),
            Err(e) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
        }
    };
    let _ = set_result;
    match build_provider_pair(&profile) {
        Ok((flash, pro)) => {
            state.agent.replace_providers(flash, pro);
            tracing::info!(
                model = %profile.model_name,
                profile = %profile.display_name,
                "active model switched"
            );
            (StatusCode::OK, Json(serde_json::json!({"ok": true, "active_id": id}))).into_response()
        }
        Err(e) => {
            // Roll back the persisted selection so UI state stays consistent.
            let mut reg = state.models.write().unwrap();
            let _ = reg.set_active(&profile.id);
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response()
        }
    }
}

// ── Debate role models API（⚙️ 辩论角色模型设置）──

/// Effective per-role selection: UI-persisted choice > DEBATE_*_MODEL env >
/// active main model. `null` per role means "主模型".
async fn debate_models_handler(State(state): State<AppState>) -> impl IntoResponse {
    let reg = state.models.read().unwrap();
    let sel = reg.debate_selection();
    Json(serde_json::json!({
        "proposer": sel.proposer,
        "opponent": sel.opponent,
        "judge": sel.judge,
        "active_id": reg.active_id(),
    }))
}

#[derive(Debug, Deserialize)]
struct SetDebateModelsRequest {
    /// Profile id or null (clear → main model).
    #[serde(default)]
    proposer: Option<String>,
    #[serde(default)]
    opponent: Option<String>,
    #[serde(default)]
    judge: Option<String>,
}

async fn set_debate_models_handler(
    State(state): State<AppState>,
    Json(req): Json<SetDebateModelsRequest>,
) -> Response {
    use miniagent_core::models::DebateRole;
    let mut reg = state.models.write().unwrap();
    for (role, id) in [
        (DebateRole::Proposer, req.proposer),
        (DebateRole::Opponent, req.opponent),
        (DebateRole::Judge, req.judge),
    ] {
        // Treat "" the same as null (HTML forms like sending empty strings).
        let id = id.filter(|s| !s.trim().is_empty());
        if let Err(e) = reg.set_debate_role(role, id) {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response();
        }
    }
    let sel = reg.debate_selection();
    (StatusCode::OK, Json(serde_json::json!({
        "ok": true,
        "proposer": sel.proposer,
        "opponent": sel.opponent,
        "judge": sel.judge,
    }))).into_response()
}

// ── Unified settings snapshots ──────────────────────────────────
//
// The settings page + header dropdown hydrate from `/api/settings/active`
// in a single round-trip. `/api/kinds` enumerates every supported
// provider family so the frontend never hardcodes enum values (single
// source of truth lives in `ModelKind`).

#[derive(Debug, Serialize)]
struct KindView {
    /// Stable slug (matches `ModelKind::slug()` — `deepseek`, `stepfun`, …).
    slug: String,
    /// Human-readable label (matches `ModelKind::label()`).
    label: String,
    /// Short glyph for inline rendering.
    icon: String,
    /// Default base URL applied by the server when a custom profile of
    /// this family is added with `base_url=""`. The UI surfaces it as a
    /// placeholder so users can pick the right provider without docs.
    default_base_url: String,
}

async fn kinds_handler() -> impl IntoResponse {
    use miniagent_core::models::ModelKind;
    let defaults: [(&str, &str); 5] = [
        ("deepseek", "https://api.deepseek.com"),
        ("stepfun", "https://api.stepfun.com/step_plan/v1"),
        ("minimax", "https://api.minimaxi.com/v1"),
        ("openai_compatible", ""),
        ("anthropic_compatible", ""),
    ];
    let kinds: Vec<KindView> = ModelKind::all()
        .iter()
        .map(|k| {
            let slug = k.slug();
            let default_base_url = defaults
                .iter()
                .find(|(s, _)| *s == slug)
                .map(|(_, u)| u.to_string())
                .unwrap_or_default();
            KindView {
                slug: slug.to_string(),
                label: k.label().to_string(),
                icon: k.icon().to_string(),
                default_base_url,
            }
        })
        .collect();
    Json(serde_json::json!({ "kinds": kinds }))
}

/// Single round-trip snapshot: the active profile (as ModelProfileView),
/// the resolved per-role debate selection, and the kind enum list.
async fn settings_active_handler(State(state): State<AppState>) -> impl IntoResponse {
    use miniagent_core::models::ModelKind;
    let reg = state.models.read().unwrap();
    let active = reg.active().clone();
    let active_view = ModelProfileView::from(&active);
    let sel = reg.debate_selection();
    let kinds: Vec<KindView> = ModelKind::all()
        .iter()
        .map(|k| KindView {
            slug: k.slug().to_string(),
            label: k.label().to_string(),
            icon: k.icon().to_string(),
            default_base_url: match k {
                ModelKind::DeepSeek => "https://api.deepseek.com".into(),
                ModelKind::StepFun => "https://api.stepfun.com/step_plan/v1".into(),
                ModelKind::MiniMax => "https://api.minimaxi.com/v1".into(),
                _ => String::new(),
            },
        })
        .collect();
    Json(serde_json::json!({
        "active": active_view,
        "debate": {
            "proposer": sel.proposer,
            "opponent": sel.opponent,
            "judge": sel.judge,
        },
        "kinds": kinds,
    }))
}
