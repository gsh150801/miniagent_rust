use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::CompletionRequest;
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{TaskResult, RepairAnalysis, StageMessage};
use crate::dispatch::{emit_task, execute_single_task, upstream_context};

/// 每个失败任务允许的 repair 重试上限。超过后不再原地重试（避免无限
/// 烧钱），转为把分析结论提交给 plan/dispatch 在下一轮换思路处理。
const MAX_RETRIES_PER_TASK: usize = 2;

/// Repair Stage: 分析失败子任务 → 生成修正提示词 → **立即重试** →
/// 把重试结果与分析结论提交给下一环节（evaluate 读取更新后的
/// task_results；plan/dispatch 通过 StageMessage 拿到分析结论）。
///
/// 此前 repair 只产出"分析报告"不执行任何修复（live: b3337de9 任务
/// task_2 连续 4 轮失败，每轮都是同样的提示词原样重跑）。现在失败
/// 任务在本阶段内完成"诊断 → 修正 → 重试"闭环，成功后 evaluate
/// 直接看到 ✓，不再触发整轮全量重跑。
pub struct RepairStage;

#[async_trait]
impl PipelineStage for RepairStage {
    fn name(&self) -> &str { "repair" }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let failed_results: Vec<TaskResult> = ctx.state.task_results.iter()
            .filter(|r| !r.success)
            .cloned()
            .collect();

        if failed_results.is_empty() {
            return Ok(StageOutput {
                updated_state: ctx.state.clone(),
                new_messages: vec![],
                summary: "No failed tasks to repair.".into(),
            });
        }

        let mut all_analyses: Vec<RepairAnalysis> = Vec::new();
        let mut messages: Vec<StageMessage> = Vec::new();
        let mut state = ctx.state.clone();

        // 供 upstream_context / 重试结果替换使用
        let mut result_map: std::collections::HashMap<String, TaskResult> = state.task_results
            .iter()
            .map(|r| (r.task_id.clone(), r.clone()))
            .collect();

        let mut retry_outcomes: Vec<String> = Vec::new();

        for result in &failed_results {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            let task_unit = state.plan.as_ref()
                .and_then(|p| p.tasks.iter().find(|t| t.id == result.task_id).cloned());

            let task_detail = task_unit.as_ref()
                .map(|t| format!(
                    "Role: {}, Description: {}, Expected: {}",
                    t.assigned_role, t.description, t.expected_output,
                ))
                .unwrap_or_default();

            let prior_error = result.error.clone().unwrap_or_else(|| "No error details".into());

            let prompt = format!(
                r#"You are the **Repair Analyst** in a multi-agent pipeline.
Analyze the failed task below and design a concrete, corrected retry.

## Failed Task
Task ID: {task_id}
{task_detail}

## Failure Evidence
Error: {prior_error}
Output before failure (may be partial/empty):
{output_preview}

## Root Cause Categories
- **tool_error**: The tool failed or returned unexpected results (retry with different parameters)
- **model_error**: The LLM failed to follow instructions (adjust prompt/role assignment)
- **dependency_error**: A dependency task failed (re-plan dependencies)
- **ambiguity_error**: Task description was unclear (requires re-exploration)
- **resource_error**: Missing files, APIs, or permissions (fix environment)
- **timeout_error**: Task exceeded time limit (narrow the scope, split the work)

## Instructions
1. Classify the root cause category
2. Be specific about what went wrong
3. Write `revised_prompt`: the corrected instructions for the retry — what to change,
   what to avoid, the corrected approach. This is executed as-is, so be concrete.
4. If the task description was unclear, mark requires_re_explore = true
5. If dependencies or role assignment was wrong, mark requires_re_plan = true

## Output Format (valid JSON only)
{{
  "failed_task_id": "{task_id}",
  "root_cause": "Specific root cause analysis",
  "suggested_fix": "Concrete steps to fix this",
  "revised_prompt": "Corrected instructions for the retry attempt",
  "requires_re_explore": true|false,
  "requires_re_plan": true|false,
  "suggested_new_approach": "Alternative approach if needed"
}}"#,
                task_id = result.task_id,
                task_detail = task_detail,
                prior_error = prior_error,
                output_preview = crate::dispatch::preview_chars(&result.output, 800),
            );

            let provider = ctx.agent.pro_provider();
            let request = CompletionRequest {
                system: format!("You are an expert failure analyst. {} Diagnose issues and design concrete fixes. Output ONLY valid JSON.", miniagent_core::context_info::date_hint()),
                messages: vec![Message::user(&prompt)],
                tools: vec![],
                config: InferenceConfig {
                    temperature: Some(0.3),
                    max_tokens: Some(ctx.config.loop_repair_max_tokens),
                    ..Default::default()
                },
            };

            let response = provider.complete(&request, cancel.child_token()).await;
            let analysis = match response {
                Ok(resp) => {
                    let text: String = resp.content.iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    let cleaned = miniagent_core::json_util::strip_markdown_fences(&text);
                    serde_json::from_str::<RepairAnalysis>(&cleaned)
                        .ok()
                        .or_else(|| serde_json::from_str::<RepairAnalysis>(
                            &miniagent_core::json_util::extract_and_repair(&text),
                        ).ok())
                        .unwrap_or_else(|| RepairAnalysis {
                            failed_task_id: result.task_id.clone(),
                            root_cause: "Unknown failure".into(),
                            suggested_fix: "Retry the task".into(),
                            requires_re_explore: false,
                            requires_re_plan: false,
                            suggested_new_approach: None,
                            revised_prompt: None,
                            retry_attempt: 0,
                        })
                }
                Err(e) => RepairAnalysis {
                    failed_task_id: result.task_id.clone(),
                    root_cause: format!("LLM analysis failed: {e}"),
                    suggested_fix: "Retry the task".into(),
                    requires_re_explore: false,
                    requires_re_plan: false,
                    suggested_new_approach: None,
                    revised_prompt: None,
                    retry_attempt: 0,
                },
            };

            tracing::info!(task_id = %analysis.failed_task_id, root_cause = %analysis.root_cause.chars().take(80).collect::<String>(), "Repair analysis");

            // ── 立即重试（核心修复：repair 不再只是"提建议"）──────────
            let attempt = state.repair_retries.get(&result.task_id).copied().unwrap_or(0);
            let can_retry = attempt < MAX_RETRIES_PER_TASK && task_unit.is_some();

            if can_retry {
                let mut retry_task = task_unit.clone().unwrap();
                let mut guidance = format!(
                    "\n\n## 重试指引（第 {} 次重试，前次失败原因分析）\n- 前次失败根因: {}\n- 修正方案: {}",
                    attempt + 1,
                    analysis.root_cause,
                    analysis.suggested_fix,
                );
                if let Some(rp) = analysis.revised_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
                    guidance.push_str(&format!("\n- 修正后的执行要求: {rp}"));
                }
                if let Some(na) = analysis.suggested_new_approach.as_deref().filter(|s| !s.trim().is_empty()) {
                    guidance.push_str(&format!("\n- 备选思路: {na}"));
                }
                retry_task.description.push_str(&guidance);

                emit_task(ctx, "running", &serde_json::json!({
                    "task_id": retry_task.id,
                    "title": retry_task.description,
                    "role": retry_task.assigned_role,
                    "difficulty": retry_task.difficulty,
                    "retry": attempt + 1,
                }));

                let upstream_block = upstream_context(&retry_task, &result_map);
                let retry_result = execute_single_task(
                    retry_task.clone(),
                    ctx.agent.clone(),
                    cancel.child_token(),
                    guidance.clone(),
                    ctx.config.loop_dispatch_max_iterations,
                    ctx.working_dir.clone(),
                    state.steerings.clone(),
                    upstream_block,
                ).await;

                let ok = retry_result.success;
                state.repair_retries.insert(result.task_id.clone(), attempt + 1);
                result_map.insert(retry_result.task_id.clone(), retry_result.clone());

                emit_task(ctx, if ok { "completed" } else { "failed" }, &serde_json::json!({
                    "task_id": retry_result.task_id,
                    "title": retry_task.description,
                    "role": retry_task.assigned_role,
                    "retry": attempt + 1,
                    "output": crate::dispatch::preview_chars(&retry_result.output, 2000),
                    "error": retry_result.error,
                }));

                retry_outcomes.push(format!(
                    "- task '{}': retry #{} → {}{}",
                    result.task_id,
                    attempt + 1,
                    if ok { "SUCCESS" } else { "STILL FAILING" },
                    retry_result.error.as_deref()
                        .map(|e| format!(" ({})", e.chars().take(120).collect::<String>()))
                        .unwrap_or_default(),
                ));

                // 更新后的结果（成功或最新失败）替换旧条目——evaluate
                // 按 task_id 取第一条匹配，必须替换旧失败记录。
                let updated_analysis = RepairAnalysis {
                    retry_attempt: attempt + 1,
                    ..analysis
                };
                all_analyses.push(updated_analysis);

                if let Some(pos) = state.task_results.iter().position(|r| r.task_id == result.task_id) {
                    state.task_results[pos] = retry_result;
                } else {
                    state.task_results.push(retry_result);
                }
                continue;
            }

            // 重试预算耗尽或无任务定义：只做分析上报
            all_analyses.push(analysis);
        }

        if !retry_outcomes.is_empty() {
            // 重试结论提交给 evaluate（下一步）：更新后的 task_results
            // 已反映重试结果，evaluate 据此重新判断循环控制。
            let content = format!(
                "Repair retries executed:\n{}\nUpdated task_results reflect the retry outcomes — re-evaluate before deciding loop control.",
                retry_outcomes.join("\n"),
            );
            messages.push(StageMessage {
                from_stage: "repair".into(),
                to_stage: "evaluate".into(),
                content,
                task_id: None,
            });
        }

        // Route repair insights to the relevant stages
        for analysis in &all_analyses {
            if analysis.requires_re_explore {
                messages.push(StageMessage {
                    from_stage: "repair".into(),
                    to_stage: "explore".into(),
                    content: format!(
                        "Re-explore required for '{}': {}. Suggested new approach: {}",
                        analysis.failed_task_id,
                        analysis.root_cause,
                        analysis.suggested_new_approach.as_deref().unwrap_or("none"),
                    ),
                    task_id: Some(analysis.failed_task_id.clone()),
                });
            }

            if analysis.requires_re_plan {
                messages.push(StageMessage {
                    from_stage: "repair".into(),
                    to_stage: "plan".into(),
                    content: format!(
                        "Re-plan required for '{}': {}. Suggested fix: {}. New approach: {}",
                        analysis.failed_task_id,
                        analysis.root_cause,
                        analysis.suggested_fix,
                        analysis.suggested_new_approach.as_deref().unwrap_or("none"),
                    ),
                    task_id: Some(analysis.failed_task_id.clone()),
                });
            }
        }

        state.repair_analyses.extend(all_analyses);

        let still_failing = state.task_results.iter().filter(|r| !r.success).count();
        let has_re_explore = state.repair_analyses.iter().any(|r| r.requires_re_explore);
        let has_re_plan = state.repair_analyses.iter().any(|r| r.requires_re_plan);

        // If no analyses triggered re-explore or re-plan, still send a generic
        // message to ensure the cycle continues properly
        if !has_re_explore && !has_re_plan {
            messages.push(StageMessage {
                from_stage: "repair".into(),
                to_stage: "explore".into(),
                content: "Repair analysis complete. Some tasks failed but no re-exploration or re-planning specifically requested. Re-evaluating overall task.".into(),
                task_id: None,
            });
        }

        Ok(StageOutput {
            updated_state: state,
            new_messages: messages,
            summary: format!(
                "Repair: {} retry attempt(s) executed this round, {} task(s) still failing. Re-explore: {}, re-plan: {}.",
                retry_outcomes.len(),
                still_failing,
                if has_re_explore { "yes" } else { "no" },
                if has_re_plan { "yes" } else { "no" },
            ),
        })
    }
}
