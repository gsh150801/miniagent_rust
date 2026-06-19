use std::sync::Arc;
use miniagent_core::error::AgentError;
use miniagent_core::settings::AppConfig;
use miniagent_evolution::{MemoryRetriever, SearchScheduler, SearchStrategy};
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::explore::ExploreStage;
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
    fn checkpoint_path(current_task: &str) -> std::path::PathBuf {
        let slug: String = current_task
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|w| !w.is_empty())
            .take(4)
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("_");
        let slug = if slug.is_empty() { "task" } else { slug.as_str() };
        std::path::PathBuf::from("./result/loop-pipeline")
            .join(format!("checkpoint_{}", slug))
            .join("_checkpoint.json")
    }

    /// Persist pipeline state so a crashed run can be resumed.
    fn save_checkpoint(state: &crate::types::PipelineState) {
        let path = Self::checkpoint_path(&state.current_task);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
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
    fn load_checkpoint(current_task: &str) -> Option<crate::types::PipelineState> {
        let path = Self::checkpoint_path(current_task);
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
    /// `memory_retriever` is an optional MLEvolve-inspired memory backend.
    /// Pass `None` (or use the simpler `run_without_memory` below) to disable.
    pub async fn run(
        task: impl Into<String>,
        config: Arc<AppConfig>,
        max_loops: usize,
        cancel: CancellationToken,
        memory_retriever: Option<Arc<dyn MemoryRetriever>>,
    ) -> Result<String, AgentError> {
        let mut ctx = StageContext::new(task, config)
            .with_max_loops(max_loops);

        // ── P0 #7: Attempt to resume from a previous checkpoint ──
        // If a checkpoint exists for this task, restore its state so a crashed
        // run continues from the last completed loop instead of restarting.
        if let Some(resumed) = Self::load_checkpoint(&ctx.state.current_task) {
            // Only resume if the previous run was genuinely in-progress (not completed).
            if !resumed.completed && resumed.loop_count > 0 {
                let resumed_loops = resumed.loop_count;
                ctx.state = resumed;
                tracing::info!("Resuming from loop {} (checkpoint loaded)", resumed_loops);
            }
        }

        if let Some(ref retriever) = memory_retriever {
            ctx = ctx.with_memory_retriever(retriever.clone());
        }

        let explore = ExploreStage;
        let plan = PlanStage;
        let dispatch = DispatchStage;
        let evaluate = EvaluateStage;
        let repair = RepairStage;

        // ── MLEvolve Phase 4: Search Scheduler ─────────────────
        let mut search_scheduler = if ctx.config.loop_search_scheduler_enabled {
            Some(SearchScheduler::new())
        } else {
            None
        };
        // ───────────────────────────────────────────────────────

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
                let output = Self::execute_isolated(&evaluate, &ctx, cancel.child_token()).await;
                ctx.state = output.updated_state;
                break;
            }

            let loop_num = ctx.state.loop_count + 1;
            tracing::info!("Loop {}/{}", loop_num, ctx.state.max_loops);

            // ── MLEvolve Phase 4: Search Strategy Selection ─────────
            // P2 #5 fix: branch by plan signature so stagnation tracking is
            // per-structure, not global. The old code always passed "main",
            // collapsing all branch-level stagnation into a single counter.
            let branch_key = ctx.state.plan.as_ref()
                .map(|p| {
                    let mut roles: Vec<_> = p.tasks.iter().map(|t| t.assigned_role.clone()).collect();
                    roles.sort();
                    // Hash to keep the key compact and stable.
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    roles.join(",").hash(&mut h);
                    format!("b{:x}", h.finish())
                })
                .unwrap_or_else(|| "main".to_string());

            // Determine search strategy before each loop.
            if let Some(ref mut scheduler) = search_scheduler {
                let strategy = scheduler.select_strategy(&branch_key);
                match strategy {
                    SearchStrategy::EliteExploitation => {
                        tracing::info!("Phase 4: EliteExploitation — using elite plan variants");
                        // Inject elite successes into retrieval_context so Plan stage
                        // can bias candidate generation toward proven role distributions.
                        let elite_ctx = scheduler.elite_context();
                        if !elite_ctx.is_empty() {
                            let elite_summaries: Vec<_> = elite_ctx.iter()
                                .map(|e| miniagent_evolution::ExperienceSummary {
                                    description: format!(
                                        "Elite plan (fitness={:.2}, success_rate={:.2}): {}",
                                        e.fitness, e.success_rate, e.role_signature
                                    ),
                                    lessons: vec![format!(
                                        "Role distribution {} achieved fitness {:.2}",
                                        e.role_signature, e.fitness
                                    )],
                                    node_type: "successpattern".into(),
                                    confidence: e.fitness,
                                })
                                .collect();
                            ctx.state.retrieval_context = crate::types::RetrievalContext {
                                relevant_successes: elite_summaries,
                                pitfalls: vec![],
                                confidence: 0.8,
                            };
                            tracing::info!(
                                elite_count = elite_ctx.len(),
                                "Phase 4: injected {} elite patterns into retrieval context",
                                elite_ctx.len()
                            );
                        }
                    }
                    SearchStrategy::CrossBranchReference => {
                        tracing::info!("Phase 4: CrossBranchReference — injecting elite success patterns");
                        // Same as EliteExploitation but triggered by stagnation
                        let elite_ctx = scheduler.elite_context();
                        if !elite_ctx.is_empty() {
                            let elite_summaries: Vec<_> = elite_ctx.iter()
                                .take(3)
                                .map(|e| miniagent_evolution::ExperienceSummary {
                                    description: format!(
                                        "Cross-branch elite (fitness={:.2}): {}",
                                        e.fitness, e.role_signature
                                    ),
                                    lessons: vec!["Consider this proven approach".into()],
                                    node_type: "successpattern".into(),
                                    confidence: e.fitness,
                                })
                                .collect();
                            ctx.state.retrieval_context = crate::types::RetrievalContext {
                                relevant_successes: elite_summaries,
                                pitfalls: vec![],
                                confidence: 0.6,
                            };
                        }
                    }
                    SearchStrategy::MultiBranchAggregation => {
                        tracing::warn!("Phase 4: MultiBranchAggregation — resetting search");
                        scheduler.reset_stagnation();
                    }
                    SearchStrategy::Normal => {
                        tracing::debug!("Phase 4: Normal search strategy");
                    }
                }
            }
            // ───────────────────────────────────────────────────────

            // ── MLEvolve Phase 1: Memory Retrieval ─────────────────
            // Retrieve relevant experiences before each loop iteration.
            // MERGE with any Phase 4 elite context already in retrieval_context,
            // instead of overwriting it.
            if let Some(ref retriever) = ctx.memory_retriever {
                let retrieval = retriever.retrieve(&ctx.state.current_task).await;

                // Merge: keep Phase 4 elite summaries, append Phase 1's results
                let mut combined = retrieval;
                let existing = &ctx.state.retrieval_context;
                if !existing.relevant_successes.is_empty() {
                    // Phase 4 already injected elite summaries — merge them
                    combined.relevant_successes = existing.relevant_successes
                        .iter().cloned()
                        .chain(combined.relevant_successes.into_iter())
                        .take(5)  // cap total to avoid prompt bloat
                        .collect();
                    combined.confidence = combined.confidence.max(existing.confidence);
                }

                let successes = combined.relevant_successes.len();
                let pitfalls = combined.pitfalls.len();
                ctx.state.retrieval_context = combined;
                tracing::debug!(
                    confidence = ctx.state.retrieval_context.confidence,
                    successes = successes,
                    pitfalls = pitfalls,
                    "Memory retrieval complete (merged with Phase 4 if present)"
                );
            }
            // ───────────────────────────────────────────────────────

            // Phase 1: Explore (isolated — fallback to default exploration)
            tracing::info!("Explore phase");
            let output = Self::execute_isolated(&explore, &ctx, cancel.child_token()).await;
            ctx.state = output.updated_state;
            ctx.collect_messages(output.new_messages);
            tracing::info!("Explore done: {}", output.summary);

            // Phase 2: Plan (isolated — fallback to single-task plan downstream)
            tracing::info!("Plan phase");
            let output = Self::execute_isolated(&plan, &ctx, cancel.child_token()).await;
            ctx.state = output.updated_state;
            ctx.collect_messages(output.new_messages);
            tracing::info!("Plan done: {}", output.summary);

            // Phase 3: Dispatch (critical — load-bearing, retry then abort)
            tracing::info!("Dispatch phase");
            let output = Self::execute_critical(&dispatch, &ctx, cancel.child_token()).await?;
            ctx.state = output.updated_state;
            ctx.collect_messages(output.new_messages);
            tracing::info!("Dispatch done: {}", output.summary);

            // Phase 4: Repair (isolated — advisory, pipeline continues without it)
            let failed_count = ctx.state.task_results.iter().filter(|r| !r.success).count();
            if failed_count > 0 {
                tracing::info!("Repair phase ({} failed tasks)", failed_count);
                let output = Self::execute_isolated(&repair, &ctx, cancel.child_token()).await;
                ctx.state = output.updated_state;
                ctx.collect_messages(output.new_messages);
                tracing::info!("Repair done: {}", output.summary);
            }

            // Phase 5: Evaluate (critical — drives loop control, retry then abort)
            tracing::info!("Evaluate phase");
            let output = Self::execute_critical(&evaluate, &ctx, cancel.child_token()).await?;
            ctx.state = output.updated_state;
            ctx.collect_messages(output.new_messages);
            tracing::info!("Evaluate done: {}", output.summary);

            // ── MLEvolve Phase 1: Record outcome ───────────────────
            // Record per-task outcomes (not just aggregate) for fine-grained
            // failure pattern tracking. Each task gets its own ExperienceGraph
            // node keyed on its description + signature.
            if let Some(ref retriever) = ctx.memory_retriever {
                // Record per-task results
                for result in &ctx.state.task_results {
                    retriever.record(
                        &result.task_id,  // Use task_id as key for fine-grained recall
                        result.success,
                        if result.success { 0.85 } else { 0.15 },
                    );
                }

                // Also record the overall task (for high-level pattern matching)
                let last_eval = ctx.state.evaluations.last();
                let success_rate = last_eval
                    .map(|e| if e.tasks_completed + e.tasks_failed > 0 {
                        e.tasks_completed as f64 / (e.tasks_completed + e.tasks_failed) as f64
                    } else { 0.5 })
                    .unwrap_or(0.5);
                retriever.record(
                    &ctx.state.current_task,
                    success_rate >= 0.5,
                    success_rate,
                );
            }
            // ───────────────────────────────────────────────────────

            // ── MLEvolve Phase 4: Search Scheduler ─────────────────
            // Record branch result and update stagnation tracking.
            if let Some(ref mut scheduler) = search_scheduler {
                let progress = ctx.state.evaluations.last()
                    .map(|e| e.overall_progress_pct)
                    .unwrap_or(0.0);

                // Build a role signature from the current plan for elite tracking
                let role_sig = ctx.state.plan.as_ref()
                    .map(|p| {
                        let mut roles: Vec<_> = p.tasks.iter().map(|t| t.assigned_role.clone()).collect();
                        roles.sort();
                        format!("{:?}", roles)
                    })
                    .unwrap_or_default();

                // P1 #12 fix: fitness (progress) and success_rate (task success
                // ratio) are distinct signals — passing the same value for both
                // made EliteEntry.success_rate meaningless. Now success_rate
                // reflects how many dispatched tasks actually succeeded.
                let last_eval = ctx.state.evaluations.last();
                let task_success_rate = last_eval
                    .map(|e| {
                        let total = e.tasks_completed + e.tasks_failed;
                        if total > 0 { e.tasks_completed as f64 / total as f64 } else { 0.0 }
                    })
                    .unwrap_or(0.0);

                scheduler.record_branch_result(
                    &branch_key,
                    progress / 100.0,
                    task_success_rate,
                    role_sig,
                );

                tracing::debug!(
                    entropy = scheduler.current_entropy(),
                    global_stagnation = scheduler.global_stagnation,
                    elite_size = scheduler.elite_set.len(),
                    "SearchScheduler state"
                );
            }
            // ───────────────────────────────────────────────────────

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
            Self::save_checkpoint(&ctx.state);
        }

        // Pipeline finished successfully — clean up the checkpoint so the next
        // run with the same task starts fresh instead of resuming a stale state.
        let cp_path = Self::checkpoint_path(&ctx.state.current_task);
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

        let final_output = ctx.state.final_output
            .unwrap_or_else(|| "(no final output)".to_string());

        tracing::info!("Pipeline complete: {summary}");

        Ok(final_output)
    }

    /// Convenience wrapper: run without memory retrieval (backward-compatible).
    pub async fn run_without_memory(
        task: impl Into<String>,
        config: Arc<AppConfig>,
        max_loops: usize,
        cancel: CancellationToken,
    ) -> Result<String, AgentError> {
        Self::run(task, config, max_loops, cancel, None).await
    }
}
