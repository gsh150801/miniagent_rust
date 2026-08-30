use std::sync::Arc;
use miniagent_core::error::AgentError;
use miniagent_core::orchestration::ProgressFn;
use miniagent_core::settings::AppConfig;
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::PipelineState;
use crate::explore::ExploreStage;
use crate::clarify::ClarifyStage;
use crate::plan::PlanStage;
use crate::dispatch::DispatchStage;
use crate::evaluate::EvaluateStage;
use crate::repair::RepairStage;

/// The LoopPipeline orchestrator manages the cyclic execution:
/// Explore → Plan → Dispatch → Evaluate → (Repair → Explore → ...) → Complete
pub struct LoopPipeline;

impl LoopPipeline {
    /// Execute a non-critical stage with error isolation.
    ///
    /// If the stage errors, we log the error and return a no-op `StageOutput`
    /// (current state preserved, empty messages, error summary) so the pipeline
    /// loop continues instead of aborting the entire run on a transient LLM
    /// failure. Used for Explore, Plan, and Repair — these have sensible
    /// downstream fallbacks.
    async fn execute_isolated<S: PipelineStage + ?Sized>(
        stage: &S,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> StageOutput {
        match stage.execute(ctx, cancel).await {
            Ok(output) => output,
            Err(e) => {
                tracing::warn!(
                    stage = stage.name(),
                    error = %e,
                    "Stage {} failed — isolating error, pipeline continues with degraded output",
                    stage.name()
                );
                StageOutput {
                    updated_state: ctx.state.clone(),
                    new_messages: vec![],
                    summary: format!("Stage {} degraded: {}", stage.name(), e),
                }
            }
        }
    }

    /// Execute a critical stage with one retry. If it still fails after retry,
    /// the error propagates and aborts the run. Used for Dispatch and Evaluate
    /// — these are load-bearing and cannot be silently skipped.
    async fn execute_critical<S: PipelineStage + ?Sized>(
        stage: &S,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        match stage.execute(ctx, cancel.clone()).await {
            Ok(output) => Ok(output),
            Err(first_err) => {
                tracing::warn!(
                    stage = stage.name(),
                    error = %first_err,
                    "Critical stage {} failed once — retrying",
                    stage.name()
                );
                stage.execute(ctx, cancel).await.map_err(|retry_err| {
                    tracing::error!(
                        stage = stage.name(),
                        first_error = %first_err,
                        retry_error = %retry_err,
                        "Critical stage {} failed after retry — aborting run",
                        stage.name()
                    );
                    retry_err
                })
            }
        }
    }

    // ── P0 #7: Crash recovery via per-loop checkpoint ──────────────

    /// Deterministic checkpoint path derived from the task description slug.
    fn checkpoint_path(base: &std::path::Path, current_task: &str) -> std::path::PathBuf {
        let slug: String = current_task
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|w| !w.is_empty())
            .take(4)
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("_");
        let slug = if slug.is_empty() { "task" } else { slug.as_str() };
        base.join(format!("checkpoint_{}", slug))
            .join("_checkpoint.json")
    }

    /// Persist pipeline state so a crashed run can be resumed.
    fn save_checkpoint(base: &std::path::Path, state: &crate::types::PipelineState) {
        let path = Self::checkpoint_path(base, &state.current_task);
        if let Some(parent) = path.parent()
            && std::fs::create_dir_all(parent).is_err() {
                return;
            }
        match serde_json::to_string_pretty(state) {
            Ok(json) => {
                if std::fs::write(&path, json).is_err() {
                    tracing::warn!("Failed to write checkpoint to {:?}", path);
                } else {
                    tracing::debug!("Checkpoint saved (loop {}) to {:?}", state.loop_count, path);
                }
            }
            Err(e) => tracing::warn!("Failed to serialize checkpoint: {}", e),
        }
    }

    /// Attempt to load a previous checkpoint for the given task.
    fn load_checkpoint(base: &std::path::Path, current_task: &str) -> Option<crate::types::PipelineState> {
        let path = Self::checkpoint_path(base, current_task);
        let data = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<crate::types::PipelineState>(&data) {
            Ok(state) => {
                tracing::info!(
                    loop_count = state.loop_count,
                    checkpoint = ?path,
                    "Resumed from checkpoint"
                );
                Some(state)
            }
            Err(e) => {
                tracing::warn!("Checkpoint exists at {:?} but is corrupt: {}", path, e);
                None
            }
        }
    }

    /// Run the full loop pipeline. Each loop follows:
    /// 1. Explore: clarify task, gather information
    /// 2. Plan: decompose into sub-tasks with dependencies
    /// 3. Dispatch: execute tasks respecting dependency order
    /// 4. Evaluate: assess completion, decide continue/stop
    /// 5. If failed tasks exist → Repair → back to Explore (next loop)
    /// 6. Repeat until all tasks complete or max loops reached
    ///
    /// `on_progress` is an optional coarse progress callback mirroring
    /// `workflow::Workflow::run_with_progress`. The server passes one in to
    /// bridge the loop pipeline into the same `ProgressMsg` channel used by
    /// the workflow driver, so the right-side progress panel can render both
    /// modes uniformly. Pass `None` for fire-and-forget CLI usage.
    ///
    /// `result_dir` anchors every artifact of the run: dispatched agents run
    /// tools with it as working directory, dispatch persistence and the
    /// per-loop checkpoint live under it. `None` falls back to
    /// `./result/loop-pipeline` (CLI behaviour).
    pub async fn run(
        task: impl Into<String>,
        config: Arc<AppConfig>,
        max_loops: usize,
        cancel: CancellationToken,
        on_progress: Option<ProgressFn>,
        result_dir: Option<std::path::PathBuf>,
    ) -> Result<PipelineState, AgentError> {
        let result_base = result_dir
            .unwrap_or_else(|| miniagent_core::paths::result_root().join("loop-pipeline"));
        if let Err(e) = std::fs::create_dir_all(&result_base) {
            tracing::warn!(path = %result_base.display(), error = %e, "failed to create loop-pipeline result dir");
        }
        // Canonicalize when possible so agents' bash cwd and every relative
        // artifact path resolve inside the task dir regardless of the
        // process CWD the server was launched from.
        let result_base = result_base
            .canonicalize()
            .unwrap_or(result_base);

        let mut ctx = StageContext::new(task, config)
            .with_max_loops(max_loops)
            .with_working_dir(result_base.to_string_lossy().to_string());

        // Stable lowercase stage names so the front-end `renderProgressView`
        // matches loop-pipeline pills with workflow pills without special-casing.
        // The callback lives in `ctx.progress` (shared with stages) so both the
        // coarse phase events here and Dispatch's per-subtask events flow
        // through the same channel.
        ctx = ctx.with_progress(on_progress);
        let phase_slot = ctx.progress.clone();
        let emit = move |name: &str, status: &str, data: Option<&serde_json::Value>| {
            if let Some(slot) = phase_slot.as_ref()
                && let Ok(mut guard) = slot.lock()
                && let Some(cb) = guard.as_mut() {
                    cb(name, status, data);
                }
        };

        // ── P0 #7: Attempt to resume from a previous checkpoint ──
        // If a checkpoint exists for this task, restore its state so a crashed
        // run continues from the last completed loop instead of restarting.
        if let Some(resumed) = Self::load_checkpoint(&result_base, &ctx.state.current_task) {
            // Only resume if the previous run was genuinely in-progress (not completed).
            if !resumed.completed && resumed.loop_count > 0 {
                let resumed_loops = resumed.loop_count;
                ctx.state = resumed;
                tracing::info!("Resuming from loop {} (checkpoint loaded)", resumed_loops);
            }
        }

        let explore = ExploreStage;
        let clarify = ClarifyStage;
        let plan = PlanStage;
        let dispatch = DispatchStage;
        let evaluate = EvaluateStage;
        let repair = RepairStage;

        tracing::info!("Loop Pipeline starting | max_loops={}", ctx.state.max_loops);

        loop {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            if ctx.state.completed {
                break;
            }

            if ctx.state.loop_count >= ctx.state.max_loops {
                tracing::warn!("Max loops ({}) reached, finalizing", ctx.state.max_loops);
                emit("evaluate", "running", None);
                let output = Self::execute_isolated(&evaluate, &ctx, cancel.child_token()).await;
                ctx.state = output.updated_state;
                emit("evaluate", "completed", Some(&serde_json::json!({"summary": output.summary})));
                break;
            }

            let loop_num = ctx.state.loop_count + 1;
            tracing::info!("Loop {}/{}", loop_num, ctx.state.max_loops);

            // Phase 1: Explore (isolated — fallback to default exploration)
            tracing::info!("Explore phase");
            emit("explore", "running", None);
            let output = Self::execute_isolated(&explore, &ctx, cancel.child_token()).await;
            ctx.state = output.updated_state;
            let explore_summary = serde_json::json!({"summary": output.summary});
            ctx.state.stage_outputs.push(crate::types::StageOutputRecord {
                stage: "explore".into(),
                summary: explore_summary.clone(),
            });
            ctx.collect_messages(output.new_messages);
            emit("explore", "completed", Some(&explore_summary));
            tracing::info!("Explore done: {}", output.summary);

            // Phase 1b: Clarify (once per run; optional — asks the user when
            // the task has material ambiguity and an interactive channel is
            // wired; skipped silently otherwise).
            if !ctx.state.clarified {
                tracing::info!("Clarify phase");
                emit("clarify", "running", None);
                let output = Self::execute_isolated(&clarify, &ctx, cancel.child_token()).await;
                ctx.state = output.updated_state;
                let clarify_summary = serde_json::json!({"summary": output.summary});
                ctx.state.stage_outputs.push(crate::types::StageOutputRecord {
                    stage: "clarify".into(),
                    summary: clarify_summary.clone(),
                });
                emit("clarify", "completed", Some(&clarify_summary));
                tracing::info!("Clarify done: {}", output.summary);
            }

            // Phase 2: PLAN (isolated — fallback to single-task plan downstream)
            tracing::info!("Plan phase");
            emit("plan", "running", None);
            let output = Self::execute_isolated(&plan, &ctx, cancel.child_token()).await;
            ctx.state = output.updated_state;
            // Ship the freshly decomposed task list with the stage-completed
            // event so the server can render the plan pill strip *before*
            // dispatch starts (workflow-mode parity) instead of after the run.
            let plan_tasks = ctx.state.plan.as_ref().map(|p| {
                serde_json::Value::Array(p.tasks.iter().map(|t| serde_json::json!({
                    "id": t.id,
                    "handler": t.assigned_role,
                    "tier": t.difficulty,
                    "description": t.description,
                    "sub_tasks": t.depends_on,
                })).collect())
            }).unwrap_or(serde_json::Value::Null);
            let plan_summary = serde_json::json!({
                "summary": output.summary,
                "plan_tasks": plan_tasks,
            });
            ctx.state.stage_outputs.push(crate::types::StageOutputRecord {
                stage: "plan".into(),
                summary: plan_summary.clone(),
            });
            ctx.collect_messages(output.new_messages);
            emit("plan", "completed", Some(&plan_summary));
            tracing::info!("Plan done: {}", output.summary);

            // Phase 3: Dispatch (critical — load-bearing, retry then abort)
            tracing::info!("Dispatch phase");
            emit("dispatch", "running", None);
            let output = match Self::execute_critical(&dispatch, &ctx, cancel.child_token()).await {
                Ok(o) => o,
                Err(e) => {
                    emit("dispatch", "failed", Some(&serde_json::json!({"error": e.to_string()})));
                    return Err(e);
                }
            };
            ctx.state = output.updated_state;
            let dispatch_summary = serde_json::json!({"summary": output.summary});
            ctx.state.stage_outputs.push(crate::types::StageOutputRecord {
                stage: "dispatch".into(),
                summary: dispatch_summary.clone(),
            });
            ctx.collect_messages(output.new_messages);
            emit("dispatch", "completed", Some(&dispatch_summary));
            tracing::info!("Dispatch done: {}", output.summary);

            // Phase 4: Repair (isolated — advisory, pipeline continues without it)
            let failed_count = ctx.state.task_results.iter().filter(|r| !r.success).count();
            if failed_count > 0 {
                tracing::info!("Repair phase ({} failed tasks)", failed_count);
                emit("repair", "running", None);
                let output = Self::execute_isolated(&repair, &ctx, cancel.child_token()).await;
                ctx.state = output.updated_state;
                let repair_summary = serde_json::json!({"summary": output.summary});
                ctx.state.stage_outputs.push(crate::types::StageOutputRecord {
                    stage: "repair".into(),
                    summary: repair_summary.clone(),
                });
                ctx.collect_messages(output.new_messages);
                emit("repair", "completed", Some(&repair_summary));
                tracing::info!("Repair done: {}", output.summary);
            }

            // Phase 5: Evaluate (critical — drives loop control, retry then abort)
            tracing::info!("Evaluate phase");
            emit("evaluate", "running", None);
            let output = match Self::execute_critical(&evaluate, &ctx, cancel.child_token()).await {
                Ok(o) => o,
                Err(e) => {
                    emit("evaluate", "failed", Some(&serde_json::json!({"error": e.to_string()})));
                    return Err(e);
                }
            };
            ctx.state = output.updated_state;
            let eval_summary = serde_json::json!({"summary": output.summary});
            ctx.state.stage_outputs.push(crate::types::StageOutputRecord {
                stage: "evaluate".into(),
                summary: eval_summary.clone(),
            });
            ctx.collect_messages(output.new_messages);
            emit("evaluate", "completed", Some(&eval_summary));
            tracing::info!("Evaluate done: {}", output.summary);

            let progress = ctx.state.evaluations.last()
                .map(|e| e.overall_progress_pct)
                .unwrap_or(0.0);

            // Track no-progress streak: compare current progress to previous loop.
            // Reset on improvement, increment on stagnation.
            let prev_progress = if ctx.state.evaluations.len() >= 2 {
                ctx.state.evaluations[ctx.state.evaluations.len() - 2].overall_progress_pct
            } else {
                0.0
            };

            if progress > prev_progress {
                ctx.state.no_progress_streak = 0;
            } else {
                ctx.state.no_progress_streak += 1;
            }

            tracing::info!(
                "Loop {} complete: {:.0}% progress (no_progress_streak: {})",
                loop_num, progress, ctx.state.no_progress_streak,
            );

            // Safety check: stop after N consecutive stagnant loops.
            // N is configurable via LOOP_NO_PROGRESS_LIMIT in .env.
            let no_progress_limit = ctx.config.loop_no_progress_limit;
            if ctx.state.no_progress_streak >= no_progress_limit && progress < 100.0 && failed_count > 0 {
                tracing::warn!(
                    streak = ctx.state.no_progress_streak,
                    limit = no_progress_limit,
                    progress = progress,
                    "No progress for {} consecutive loops, stopping to avoid infinite loop",
                    no_progress_limit,
                );
                ctx.state.completed = true;
            }

            // ── Cost-control early stop ──
            // If a single loop consumed excessive tokens but progress is negligible,
            // terminate to avoid burning budget on an unproductive pipeline.
            let loop_tokens: usize = ctx.state.task_results.iter()
                .map(|r| r.tokens_used).sum();
            ctx.state.total_tokens_used += loop_tokens;

            let cost_threshold = ctx.config.loop_cost_token_threshold;
            let min_progress = ctx.config.loop_cost_min_progress;
            if loop_tokens > cost_threshold
                && progress < min_progress
                && ctx.state.loop_count > 0
            {
                tracing::warn!(
                    loop_tokens = loop_tokens,
                    total_tokens = ctx.state.total_tokens_used,
                    progress = progress,
                    "Cost-control early stop: loop consumed {} tokens but progress only {:.0}%",
                    loop_tokens, progress,
                );
                ctx.state.completed = true;
            }

            // ── P0 #7: Persist state at end of each loop for crash recovery.
            // If the process dies before the next loop, we can resume from here.
            Self::save_checkpoint(&result_base, &ctx.state);
        }

        // Pipeline finished successfully — clean up the checkpoint so the next
        // run with the same task starts fresh instead of resuming a stale state.
        let cp_path = Self::checkpoint_path(&result_base, &ctx.state.current_task);
        if cp_path.exists() {
            let _ = std::fs::remove_file(&cp_path);
            tracing::debug!("Checkpoint cleaned up: {:?}", cp_path);
        }

        let summary = ctx.state.evaluations.last()
            .map(|e| format!(
                "Progress: {:.0}% | Tasks: {}/{} completed | {} failed after {} loops",
                e.overall_progress_pct,
                e.tasks_completed,
                e.tasks_completed + e.tasks_failed + e.tasks_pending,
                e.tasks_failed,
                ctx.state.loop_count,
            ))
            .unwrap_or_else(|| "Pipeline completed with unknown status".into());

        tracing::info!("Pipeline complete: {summary}");

        Ok(ctx.state)
    }
}
