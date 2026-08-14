use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::CompletionRequest;
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{EvaluationResult, StageMessage, TaskResult};
use crate::dispatch::outputs_still_exist;

/// Evaluate Stage: assesses task completion status, decides whether to
/// continue to the next loop or finish.
pub struct EvaluateStage;

#[async_trait]
impl PipelineStage for EvaluateStage {
    fn name(&self) -> &str { "evaluate" }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let results = &ctx.state.task_results;
        let loop_count = ctx.state.loop_count;
        let max_loops = ctx.state.max_loops;

        let plan = match &ctx.state.plan {
            Some(p) => p,
            None => return Err(AgentError::invalid_state(String::from("No plan for evaluation"))),
        };

        // 只统计当前 plan 中的任务，避免历史累积干扰（修复 #2 进度失真）
        let plan_task_ids: std::collections::HashSet<&str> =
            plan.tasks.iter().map(|t| t.id.as_str()).collect();

        let relevant_results: Vec<&TaskResult> = results.iter()
            .filter(|r| plan_task_ids.contains(r.task_id.as_str()))
            .collect();

        let mut completed = 0usize;
        let mut failed = 0usize;
        let mut pending = 0usize;

        for task in &plan.tasks {
            if let Some(result) = relevant_results.iter().find(|r| r.task_id == task.id) {
                if result.success {
                    completed += 1;
                } else {
                    failed += 1;
                }
            } else {
                pending += 1;
            }
        }

        let total = plan.tasks.len();
        // completed + failed + pending 必须等于 total（基于当前 plan 精确计算）
        debug_assert_eq!(completed + failed + pending, total,
            "Progress accounting must sum to plan task count");

        let result_summary: String = relevant_results.iter()
            .map(|r| {
                let status = if r.success { "✓" } else { "✗" };
                let preview = r.output.chars().take(200).collect::<String>();
                format!("{status} [{}] {}\n   {}", r.task_id, status, preview)
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let repair_insights: String = ctx.state.repair_analyses.iter()
            .map(|r| format!("- Repair for '{}': root cause: {}", r.failed_task_id, r.root_cause))
            .collect::<Vec<_>>()
            .join("\n");

        let progress_pct = if total > 0 {
            (completed as f64 / total as f64) * 100.0
        } else { 0.0 };

        let failed_ids_json: String = plan.tasks.iter()
            .filter(|t| {
                relevant_results.iter()
                    .any(|r| r.task_id == t.id && !r.success)
            })
            .map(|t| format!("\"{}\"", t.id))
            .collect::<Vec<_>>()
            .join(", ");

        let prompt = format!(
            r#"You are the **Evaluator** in a multi-agent pipeline. Assess task completion and decide next steps.

## Overall Goal
{goal}

## Loop {loop_count}/{max_loops}

## Task Results ({completed}/{total} completed, {failed} failed, {pending} pending)
{result_summary}

## Repair Insights from Previous Loop
{repair_insights}

## Instructions (BE THOROUGH)
1. Assess overall progress toward the goal based on actual outputs
2. For each failed task, determine if it was a critical failure or a minor issue
3. Quality check: even "successful" tasks may have produced incomplete or low-quality outputs
4. Decide whether to continue:
   - Continue if: any critical tasks failed, quality is insufficient, or important aspects remain unexplored
   - Continue if: the loop count is low and progress is meaningful
   - Stop if: ALL tasks completed successfully AND quality is acceptable
   - Stop if: no progress over multiple loops (stuck)
5. If continuing, specify what the next loop should focus on concretely

## Output Format (valid JSON only)
{{
  "tasks_completed": {completed},
  "tasks_failed": {failed},
  "tasks_pending": {pending},
  "overall_progress_pct": {progress_pct},
  "failed_task_ids": [{failed_ids_json}],
  "unmet_goals": ["Unmet goal 1", "Unmet goal 2"],
  "should_continue": true|false,
  "summary": "Overall assessment of what was accomplished and what remains"
}}"#,
            goal = plan.overall_goal,
            loop_count = loop_count,
            max_loops = max_loops,
            completed = completed,
            total = total,
            failed = failed,
            pending = pending,
            progress_pct = progress_pct,
            failed_ids_json = failed_ids_json,
        );

        let provider = ctx.agent.flash_provider();
        let request = CompletionRequest {
            system: format!("You are a thorough evaluator. The current date is {}. Assess task completion objectively. Output ONLY valid JSON.", miniagent_core::context_info::date_hint()),
            messages: vec![Message::user(&prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.2),
                max_tokens: Some(ctx.config.loop_evaluate_max_tokens),
                ..Default::default()
            },
        };

        let response = provider.complete(&request, cancel).await?;
        let text: String = response.content.iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        let cleaned = miniagent_core::json_util::strip_markdown_fences(&text);

        let mut evaluation: EvaluationResult = match serde_json::from_str(&cleaned) {
            Ok(e) => e,
            Err(e) => {
                let preview: String = cleaned.chars().take(200).collect();
                tracing::warn!(error = %e, preview = %preview, "Evaluate JSON parse failed, using computed values");
                EvaluationResult {
                    tasks_completed: completed,
                    tasks_failed: failed,
                    tasks_pending: pending,
                    overall_progress_pct: if total > 0 {
                        (completed as f64 / total as f64) * 100.0
                    } else { 0.0 },
                    failed_task_ids: relevant_results.iter()
                        .filter(|r| !r.success)
                        .map(|r| r.task_id.clone())
                        .collect(),
                    unmet_goals: vec!["Evaluation parse failed, defaulting to continue".into()],
                    should_continue: failed > 0 || loop_count == 0,
                    summary: format!("{}/{} tasks completed. {} failed.", completed, total, failed),
                }
            }
        };

        // Override should_continue based on loop limits and failures.
        // Stop when we hit the loop limit or when every task completed without
        // failures (the high-progress sub-case is subsumed by `failed == 0 &&
        // completed == total`). Keep going on the very first loop before any
        // task has run.
        if loop_count >= max_loops || (failed == 0 && completed == total) {
            evaluation.should_continue = false;
        } else if loop_count == 0 && completed == 0 {
            evaluation.should_continue = true;
        }

        // ── 客观产物校验（缺陷 #3 修复）──────────────────────────────
        // evaluate 不应完全依赖 LLM 主观判断。当 LLM 或 override 判定"无需继续"
        //（即将终止）时，对每个"成功"task 做客观校验：检查 expected_output 提到的
        // 文件产物是否真存在。若有"成功"task 的产物文件缺失，说明该 task 实际未
        // 完成产物——强制 should_continue=true 并把缺失 task 标记为失败，防止
        // 基于错误评估提前终止。
        if !evaluation.should_continue {
            let phantom = check_phantom_failures(
                &plan.tasks, &relevant_results, &ctx.working_dir,
            );
            if !phantom.is_empty() {
                tracing::warn!(
                    phantom_failures = ?phantom,
                    "objective check: {} tasks marked successful but output files missing — \
                     forcing continue to prevent premature termination",
                    phantom.len(),
                );
                evaluation.should_continue = true;
                for id in &phantom {
                    if !evaluation.failed_task_ids.contains(id) {
                        evaluation.failed_task_ids.push(id.clone());
                    }
                }
                evaluation.unmet_goals.push(format!(
                    "Output files missing for tasks: {} — must re-execute to produce them",
                    phantom.join(", "),
                ));
            }
        }
        // ──────────────────────────────────────────────────────────

        let mut state = ctx.state.clone();
        state.evaluations.push(evaluation.clone());
        state.completed = !evaluation.should_continue;

        if !evaluation.should_continue {
            state.final_output = Some(evaluation.summary.clone());
            // Collect successful outputs as final output（只收集当前 plan 中的任务）
            let outputs: Vec<String> = relevant_results.iter()
                .filter(|r| r.success)
                .map(|r| r.output.clone())
                .collect();
            if !outputs.is_empty() {
                state.final_output = Some(outputs.join("\n\n---\n\n"));
            }
        }

        let mut msg = StageMessage {
            from_stage: "evaluate".into(),
            to_stage: if evaluation.should_continue { "explore".into() } else { "__complete__".into() },
            content: serde_json::to_string(&evaluation).unwrap_or_default(),
            task_id: None,
        };

        // If there are failed tasks and we're continuing, route to repair first
        if evaluation.should_continue && !evaluation.failed_task_ids.is_empty() {
            msg.to_stage = "repair".into();

            // Also send a message to explore for re-exploration if needed
            let explore_msg = StageMessage {
                from_stage: "evaluate".into(),
                to_stage: "explore".into(),
                content: format!(
                    "Unmet goals: {}. Failed tasks: {}. Repair analysis will follow.",
                    evaluation.unmet_goals.join("; "),
                    evaluation.failed_task_ids.join(", "),
                ),
                task_id: None,
            };
            state.loop_count += 1;
            let mut new_msgs = vec![msg, explore_msg];

            // Send to plan stage too if there are unmet goals
            if !evaluation.unmet_goals.is_empty() {
                new_msgs.push(StageMessage {
                    from_stage: "evaluate".into(),
                    to_stage: "plan".into(),
                    content: format!("Re-plan needed for: {}", evaluation.unmet_goals.join("; ")),
                    task_id: None,
                });
            }

            let next_loop = state.loop_count;
            return Ok(StageOutput {
                updated_state: state,
                new_messages: new_msgs,
                summary: format!(
                    "Evaluation: {}/{} done. {} failed. Continuing loop {}.",
                    completed, total, failed, next_loop,
                ),
            });
        }

        if evaluation.should_continue {
            state.loop_count += 1;
        }

        let loop_count = state.loop_count;
        let max_loops = state.max_loops;
        Ok(StageOutput {
            updated_state: state,
            new_messages: vec![msg],
            summary: format!(
                "Evaluation: {:.0}% complete. {} failed. {}",
                evaluation.overall_progress_pct, evaluation.tasks_failed,
                if evaluation.should_continue {
                    format!("Continuing loop {}/{}", loop_count, max_loops)
                } else {
                    "Pipeline complete!".into()
                },
            ),
        })
    }
}


/// 客观产物校验：检查 plan 中标记为"成功"的 task，其 expected_output 提到的文件是否真存在。
/// 返回产物文件缺失的 task_id 列表（"幽灵成功"——标记成功但产物不在）。
///
/// 这是缺陷 #3（评估无客观信号）的修复：让 evaluate 不完全依赖 LLM 主观判断，
/// 用客观文件存在性补充。当 LLM 说"全部完成"但实际文件缺失时，此函数捕获这种
/// "幽灵成功"，防止基于错误评估提前终止。
pub fn check_phantom_failures(
    tasks: &[miniagent_core::task_plan::TaskUnit],
    results: &[&TaskResult],
    working_dir: &str,
) -> Vec<String> {
    let mut phantom = Vec::new();
    for task in tasks {
        let was_marked_success = results.iter().any(|r| r.task_id == task.id && r.success);
        if !was_marked_success { continue; }
        if !outputs_still_exist(&task.expected_output, working_dir) {
            phantom.push(task.id.clone());
        }
    }
    phantom
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniagent_core::task_plan::TaskUnit;

    fn task_with_output(id: &str, expected: &str) -> TaskUnit {
        TaskUnit {
            id: id.into(),
            description: format!("task {id}"),
            assigned_role: "writer".into(),
            depends_on: vec![],
            expected_output: expected.into(),
            difficulty: "simple".into(),
            failed: false,
            output: None,
            error: None,
        }
    }

    fn success_result(id: &str) -> TaskResult {
        TaskResult {
            task_id: id.into(),
            success: true,
            output: format!("output of {id}"),
            error: None,
            tokens_used: 100,
        validation_report: None,
        arbiter_decision: None,
        }
    }

    #[test]
    fn test_phantom_check_no_missing_files() {
        // 所有成功 task 的产物文件都在 → 无幽灵
        let dir = std::env::temp_dir().join("loop_eval_phantom_ok");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("report.md"), "# Report").ok();

        let tasks = vec![task_with_output("t1", &format!("write {}", dir.join("report.md").display()))];
        let results = [success_result("t1")];
        let refs: Vec<&TaskResult> = results.iter().collect();

        let phantom = check_phantom_failures(&tasks, &refs, &dir.to_string_lossy());
        assert!(phantom.is_empty(), "file exists → no phantom failure");
    }

    #[test]
    fn test_phantom_check_detects_missing_file() {
        // 成功 task 但文件不存在 → 幽灵失败
        let dir = std::env::temp_dir().join("loop_eval_phantom_missing");
        std::fs::create_dir_all(&dir).ok();

        let tasks = vec![task_with_output("t1", &format!("write {}", dir.join("nonexistent.csv").display()))];
        let results = [success_result("t1")];
        let refs: Vec<&TaskResult> = results.iter().collect();

        let phantom = check_phantom_failures(&tasks, &refs, &dir.to_string_lossy());
        assert_eq!(phantom, vec!["t1".to_string()], "missing file → phantom failure detected");
    }

    #[test]
    fn test_phantom_check_skips_text_only_outputs() {
        // 纯文本输出（无文件路径）→ 不是幽灵（outputs_still_exist 返回 true）
        let dir = std::env::temp_dir().join("loop_eval_phantom_text");
        std::fs::create_dir_all(&dir).ok();

        let tasks = vec![task_with_output("t1", "analysis summary in text form")];
        let results = [success_result("t1")];
        let refs: Vec<&TaskResult> = results.iter().collect();

        let phantom = check_phantom_failures(&tasks, &refs, &dir.to_string_lossy());
        assert!(phantom.is_empty(), "text-only output → not phantom");
    }

    #[test]
    fn test_phantom_check_skips_failed_tasks() {
        // 失败 task 不校验（已由 failed_ids 覆盖）
        let dir = std::env::temp_dir().join("loop_eval_phantom_failed");
        std::fs::create_dir_all(&dir).ok();

        let tasks = vec![task_with_output("t1", &format!("write {}", dir.join("missing.csv").display()))];
        // t1 失败（success=false）
        let results = [TaskResult {
            task_id: "t1".into(), success: false,
            output: String::new(), error: Some("failed".into()), tokens_used: 50,
        validation_report: None,
        arbiter_decision: None,
        }];
        let refs: Vec<&TaskResult> = results.iter().collect();

        let phantom = check_phantom_failures(&tasks, &refs, &dir.to_string_lossy());
        assert!(phantom.is_empty(), "failed task → not checked (already in failed_ids)");
    }

    #[test]
    fn test_phantom_check_mixed_success_and_failure() {
        // t1 成功且文件在，t2 成功但文件缺失 → 只有 t2 是幽灵
        let dir = std::env::temp_dir().join("loop_eval_phantom_mixed");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("a.md"), "").ok();
        // b.csv 不创建

        let tasks = vec![
            task_with_output("t1", &format!("write {}", dir.join("a.md").display())),
            task_with_output("t2", &format!("write {}", dir.join("b.csv").display())),
        ];
        let results = [success_result("t1"), success_result("t2")];
        let refs: Vec<&TaskResult> = results.iter().collect();

        let phantom = check_phantom_failures(&tasks, &refs, &dir.to_string_lossy());
        assert_eq!(phantom, vec!["t2".to_string()], "only t2 should be phantom");
    }
}