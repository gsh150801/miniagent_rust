use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{TaskPlan, TaskUnit, StageMessage};

/// Plan Stage: decomposes the clarified task into sub-tasks with dependencies,
/// assigns roles, and defines expected outputs.
pub struct PlanStage;

#[async_trait]
impl PipelineStage for PlanStage {
    fn name(&self) -> &str { "plan" }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let task = &ctx.state.current_task;
        let loop_count = ctx.state.loop_count;

        // ── P-多智能体规划：两阶段——先枚举独立工作项（模型对"列举"的
        // 服从度远高于"完整 JSON 分解"），≥2 项时机械展开为并行 TaskUnit
        // （空 depends_on ⇒ dispatch 单波并行）+ 汇编 writer 任务。
        // 失败/单项时回退到既有 LLM 分解路径。
        if let Some(items) = crate::plan::enumerate_work_items(
            ctx.agent.flash_provider().as_ref(),
            task,
            cancel.child_token(),
        )
        .await
        {
            if items.len() >= 2 {
                let mut tasks: Vec<TaskUnit> = Vec::new();
                for (i, (title, role)) in items.iter().enumerate() {
                    tasks.push(TaskUnit {
                        id: format!("task_{}", i + 1),
                        description: title.clone(),
                        assigned_role: role.clone(),
                        depends_on: vec![],
                        expected_output: format!("完成「{title}」并给出结果"),
                        difficulty: "medium".into(),
                        failed: false,
                        error: None,
                        output: None,
                    });
                }
                let compile_id = format!("task_{}", items.len() + 1);
                tasks.push(TaskUnit {
                    id: compile_id.clone(),
                    description: "汇总以上全部子任务的结果，产出最终交付物".into(),
                    assigned_role: "writer".into(),
                    depends_on: (1..=items.len()).map(|i| format!("task_{i}")).collect(),
                    expected_output: "最终汇总交付物（整合所有子任务结果）".into(),
                    difficulty: "medium".into(),
                    failed: false,
                    error: None,
                    output: None,
                });

                tracing::info!(tasks = tasks.len(), "Enumerated multi-agent plan");
                let goal = format!(
                    "{}（多智能体并行执行：{} 个并行子任务 + 1 个汇编任务）",
                    task,
                    items.len()
                );
                let mut state = ctx.state.clone();
                state.plan = Some(TaskPlan {
                    overall_goal: goal.clone(),
                    tasks,
                    max_loops: ctx.state.max_loops,
                });
                state.current_task = goal.clone();

                let summary = format!(
                    "Enumerated multi-agent plan: {} parallel work items + compiler",
                    items.len()
                );
                let msg = StageMessage {
                    from_stage: "plan".into(),
                    to_stage: "dispatch".into(),
                    content: serde_json::to_string(&state.plan).unwrap_or_default(),
                    task_id: None,
                };
                return Ok(StageOutput {
                    updated_state: state,
                    new_messages: vec![msg],
                    summary,
                });
            }
        }

        let repair_suggestions: String = ctx.state.repair_analyses.iter()
            .filter(|r| r.requires_re_plan)
            .map(|r| format!(
                "- Task '{}': root cause: {}. Suggested fix: {}. New approach: {}",
                r.failed_task_id, r.root_cause, r.suggested_fix,
                r.suggested_new_approach.as_deref().unwrap_or("none")
            ))
            .collect::<Vec<_>>()
            .join("\n");

        let prior_tasks: String = ctx.state.plan.as_ref()
            .map(|p| {
                // 计算每个 task 的成功状态（查 task_results）
                let task_summaries: Vec<String> = p.tasks.iter()
                    .map(|t| {
                        let was_successful = ctx.state.task_results.iter()
                            .any(|r| r.task_id == t.id && r.success);
                        let status = if was_successful { "SUCCESS — REUSE this exact id" }
                            else if t.failed { "FAILED — retry with new approach" }
                            else if t.output.is_some() { "done" }
                            else { "pending" };
                        format!("  - id=\"{}\" | {} (role: {}, deps: {:?}, status: {})",
                            t.id, t.description, t.assigned_role, t.depends_on, status,
                        )
                    })
                    .collect();
                format!(
                    "## Previous Plan\n\
                     IMPORTANT: For tasks marked \"SUCCESS\", you MUST reuse the exact same id.\n\
                     Only generate new ids for FAILED or new tasks.\n\
                     CRITICAL: Never rename a successful task — keep id, description, role, and deps identical.\n{}\n",
                    task_summaries.join("\n")
                )
            })
            .unwrap_or_default();

        let needs_decomposition = ctx.state.exploration_history.last()
            .map(|e| e.needs_decomposition)
            .unwrap_or(false);

        let prompt = build_plan_prompt(task, loop_count, &repair_suggestions, &prior_tasks, needs_decomposition);

        let provider = ctx.agent.flash_provider();

        // Attempt plan generation with retry: if the LLM returns only 1 task
        // when decomposition is needed, retry once with a stronger emphasis.
        let mut plan = match try_generate_plan(provider.as_ref(), &prompt, ctx.config.loop_plan_max_tokens, cancel.clone()).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "Plan generation failed, using fallback");
                TaskPlan {
                    overall_goal: task.to_string(),
                    tasks: vec![TaskUnit {
                        id: "task_1".into(),
                        description: task.to_string(),
                        assigned_role: "executor".into(),
                        depends_on: vec![],
                        expected_output: "Completed task".into(),
                        difficulty: "medium".into(),
                        failed: false,
                        error: None,
                        output: None,
                    }],
                    max_loops: 5,
                }
            }
        };

        // Retry once if decomposition was needed but only 1 task was returned
        if plan.tasks.len() == 1 && needs_decomposition {
            tracing::info!("Plan returned 1 task but decomposition needed — retrying with stronger prompt");
            let retry_prompt = build_plan_prompt_retry(task, loop_count);
            // 升级到 pro 模型重试（flash 在复杂分解上更易偷懒合并）
            if let Ok(retry_plan) = try_generate_plan(ctx.agent.pro_provider().as_ref(), &retry_prompt, ctx.config.loop_plan_max_tokens, cancel.clone()).await
                && retry_plan.tasks.len() > 1 {
                    tracing::info!(tasks = retry_plan.tasks.len(), "Retry succeeded: decomposed into multiple tasks");
                    plan = retry_plan;
                }
        }

        tracing::info!("Plan: {} tasks", plan.tasks.len());
        for (i, t) in plan.tasks.iter().enumerate() {
            tracing::debug!(index = i + 1, role = %t.assigned_role, deps = ?t.depends_on, desc = %t.description, "task");
        }

        let mut state = ctx.state.clone();

        // ── 增量合并：保留上轮已成功任务的 id/output（缺陷 #1 修复）──
        // 区分首次规划 vs 增量修复：
        //   - 首次规划（无旧 plan 或旧 plan 无任务）：直接使用 LLM 生成的 plan
        //   - 增量修复（有旧 plan 且存在成功任务）：merge_plan 保留成功任务的 id/output/role/deps
        let is_first_plan = ctx.state.plan.is_none()
            || ctx.state.plan.as_ref().map(|p| p.tasks.is_empty()).unwrap_or(true);

        let plan = if is_first_plan {
            // 首次规划：直接使用 LLM 生成的 plan
            plan
        } else if let Some(old_plan) = &ctx.state.plan {
            // 增量修复：merge_plan 保留成功任务的完整定义
            merge_plan(plan, old_plan, &ctx.state.task_results)
        } else {
            plan
        };
        // ──────────────────────────────────────────────────────────

        state.plan = Some(plan.clone());

        let msg = StageMessage {
            from_stage: "plan".into(),
            to_stage: "dispatch".into(),
            content: serde_json::to_string(&plan).unwrap_or_default(),
            task_id: None,
        };

        Ok(StageOutput {
            updated_state: state,
            new_messages: vec![msg],
            summary: format!("Planned {} tasks for '{}'", plan.tasks.len(), plan.overall_goal),
        })
    }
}

/// Build the initial planning prompt with few-shot examples.
fn build_plan_prompt(
    task: &str,
    loop_count: usize,
    repair_suggestions: &str,
    prior_tasks: &str,
    needs_decomposition: bool,
) -> String {
    let decomp_hint = if needs_decomposition {
        "IMPORTANT: The explorer determined this task MUST be decomposed into multiple parallel sub-tasks. \
          Create ONE sub-task per enumerated subject/topic, each with empty depends_on, \
          plus a final compilation task depending on all of them."
    } else {
        "Decompose if the task has multiple aspects; otherwise a single well-scoped task is acceptable."
    };

    format!(
        r#"You are a task planner. Decompose the task into multiple sub-tasks that run IN PARALLEL.

## CRITICAL: Create 3-8 sub-tasks
{decomp_hint}
If the task has multiple topics, create ONE sub-task per topic. NEVER output 1 task when the task clearly involves multiple topics.

## Task
{task}

## Loop Iteration
{loop_count}

## Available Roles
- "researcher": web_search, web_fetch, pubmed_search, patent_search, clinical_trials_search (for information gathering)
- "executor": bash, read, write, edit, glob, grep, git, conda (for code execution)
- "writer": read, write, edit (for producing output)
- "critic": read, web_search, web_fetch (for review)
- "synthesizer": read (for integrating sources)
- "analyst": read, grep, glob (for data analysis)

## Repair Context
{repair_suggestions}

{prior_tasks}
## Instructions
1. Break into 3-8 concrete sub-tasks
2. For multi-topic queries: ONE sub-task per topic
3. Parallel tasks have empty depends_on
4. Assign "writer" for final compilation with depends_on on research tasks
5. Assign "researcher" for information gathering

## Few-Shot Example 1: Multi-topic research
Task: "Research A, B, C and write report"
→ 4 tasks: research_A (researcher, deps:[]), research_B (researcher, deps:[]), research_C (researcher, deps:[]), synthesize (writer, deps:[A,B,C])

## Few-Shot Example 2: Code task
Task: "Fetch data, process it, generate chart"
→ 4 tasks: fetch (executor, deps:[]), process (executor, deps:[fetch]), chart (executor, deps:[process]), report (writer, deps:[chart])

## Few-Shot Example 3: Single atomic task
Task: "Write hello world in Python"
→ 1 task: write_code (executor, deps:[])

## Output JSON ONLY. No other text.
{{
  "overall_goal": "the overall goal",
  "tasks": [
    {{
      "id": "task_1",
      "description": "what to do and which tools to use",
      "assigned_role": "researcher|executor|writer|critic|synthesizer|analyst",
      "depends_on": [],
      "expected_output": "what this task produces",
      "difficulty": "simple|medium|hard"
    }}
  ],
  "max_loops": 5
	}}"#,
        decomp_hint = decomp_hint,
        task = task,
        loop_count = loop_count,
        repair_suggestions = repair_suggestions,
        prior_tasks = prior_tasks,
    )
}

/// Build a retry prompt with stronger decomposition emphasis.
fn build_plan_prompt_retry(task: &str, loop_count: usize) -> String {
    format!(
        r#"You previously returned only 1 task. The task MUST be decomposed into multiple sub-tasks.

## Task
{task}

## Loop {loop_count}

## MANDATORY: Output at least 3 sub-tasks.
- If the task involves research, split by topic
- If the task involves code, split by pipeline step
- ALWAYS add a final "writer" or "synthesizer" task that depends on all others

## Output JSON ONLY:
{{
  "overall_goal": "...",
  "tasks": [
    {{"id": "task_1", "description": "...", "assigned_role": "researcher|executor|writer|critic|synthesizer|analyst", "depends_on": [], "expected_output": "...", "difficulty": "simple|medium|hard"}},
    {{"id": "task_2", ...}},
    {{"id": "task_3", ...}}
  ],
  "max_loops": 5
}}"#,
        task = task,
        loop_count = loop_count,
    )
}

/// Call the LLM to generate a plan, returning a parsed TaskPlan.
async fn try_generate_plan(
    provider: &dyn LlmProvider,
    prompt: &str,
    max_tokens: u32,
    cancel: CancellationToken,
) -> Result<TaskPlan, AgentError> {
    let request = CompletionRequest {
        system: format!("You are an expert task planner. {} ALWAYS decompose into multiple sub-tasks. NEVER output just 1 task unless the task is truly atomic. Output ONLY valid JSON.", miniagent_core::context_info::date_hint()),
        messages: vec![Message::user(prompt)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.4),
            max_tokens: Some(max_tokens),
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

    let plan: TaskPlan = serde_json::from_str(&cleaned).map_err(|e| {
        let preview: String = cleaned.chars().take(300).collect();
        tracing::warn!(error = %e, preview = %preview, "Plan JSON parse failed");
        AgentError::invalid_state(format!("Plan parse failed: {e}"))
    })?;

    Ok(plan)
}

/// 增量合并新 plan 与旧 plan：对 id 匹配且上轮成功的 task，保留旧 task 的完整定义
///（含 id/output/role/deps/description），使 dispatch 能安全跳过重跑。
///
/// 逻辑：
///   - 遍历新 plan 的每个 task，若旧 plan 中存在相同 id 且该 id 在 task_results 中有
///     `success==true` 记录，则保留旧 task 的完整定义（完全复用）。
///   - 如果 LLM 调整了 description/role/deps，只保留 output 和成功标记（不完全复用）。
///   - 其余 task 保持新 plan 版本不变。
///
/// 注意：这里不改变新 plan 的 overall_goal 和 max_loops（LLM 可能合理地调整了这些）。
pub fn merge_plan(
    mut new_plan: TaskPlan,
    old_plan: &TaskPlan,
    task_results: &[crate::types::TaskResult],
) -> TaskPlan {
    let old_by_id: std::collections::HashMap<String, TaskUnit> =
        old_plan.tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();

    let mut preserved = 0usize;
    for new_task in &mut new_plan.tasks {
        // 该 task 是否在上轮成功？
        let was_successful = task_results.iter()
            .any(|r| r.task_id == new_task.id && r.success);
        if !was_successful { continue; }

        if let Some(old_task) = old_by_id.get(&new_task.id)
            && old_task.output.is_some() {
                // 关键修复：如果 LLM 未改变关键字段，完全复用旧 task（含 id/output/role/deps）
                // 这确保跨轮 id 稳定，dispatch 能命中跳过逻辑
                if new_task.description == old_task.description
                    && new_task.assigned_role == old_task.assigned_role
                    && new_task.depends_on == old_task.depends_on
                {
                    *new_task = old_task.clone();
                } else {
                    // LLM 调整了描述/角色/依赖，只保留 output 和成功标记
                    new_task.output = old_task.output.clone();
                    new_task.failed = false;
                    new_task.error = None;
                }
                preserved += 1;
                tracing::debug!(
                    task_id = %new_task.id,
                    "merge_plan: preserved successful task output (skip re-execution)"
                );
            }
    }

    if preserved > 0 {
        tracing::info!(
            preserved,
            total = new_plan.tasks.len(),
            "merge_plan: {} successful tasks preserved from previous loop",
            preserved,
        );
    }

    new_plan
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskResult;
    use miniagent_core::task_plan::{TaskPlan, TaskUnit};

    fn task(id: &str) -> TaskUnit {
        TaskUnit {
            id: id.into(),
            description: format!("task {id}"),
            assigned_role: "researcher".into(),
            depends_on: vec![],
            expected_output: "text output".into(),
            difficulty: "simple".into(),
            failed: false,
            error: None,
            output: None,
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
    fn test_merge_plan_first_run_no_merge() {
        // 无旧 plan → 直接用新 plan（首次规划）
        let new_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![task("t1"), task("t2")], max_loops: 3 };
        let merged = merge_plan(new_plan, &TaskPlan { overall_goal: "test".into(), tasks: vec![], max_loops: 3 }, &[]);
        assert_eq!(merged.tasks.len(), 2);
        // output 应为 None（未合并）
        assert!(merged.tasks[0].output.is_none());
    }

    #[test]
    fn test_merge_plan_preserves_successful_output() {
        // 旧 plan 有 t1(output=Some) 和 t2(output=None)
        let mut old_t1 = task("t1");
        old_t1.output = Some("t1 result".into());
        let old_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![old_t1, task("t2")], max_loops: 3 };

        // 新 plan 有相同 id 的 t1 + t2（LLM 复用了 id）
        let new_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![task("t1"), task("t2")], max_loops: 3 };

        // task_results 显示 t1 成功
        let results = vec![success_result("t1")];

        let merged = merge_plan(new_plan, &old_plan, &results);

        // t1 应保留旧 output（让 dispatch 跳过）
        let t1 = merged.tasks.iter().find(|t| t.id == "t1").unwrap();
        assert!(t1.output.is_some(), "successful task should preserve output");
        assert_eq!(t1.output.as_ref().unwrap(), "t1 result");
        assert!(!t1.failed, "successful task should not be marked failed");

        // t2 无成功记录 → output 保持 None
        let t2 = merged.tasks.iter().find(|t| t.id == "t2").unwrap();
        assert!(t2.output.is_none(), "unsuccessful task should not have output");
    }

    #[test]
    fn test_merge_plan_new_id_not_merged() {
        // 旧 plan 有 t1，新 plan 有 t1 和 t3（新任务）
        let mut old_t1 = task("t1");
        old_t1.output = Some("old".into());
        let old_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![old_t1], max_loops: 3 };
        let new_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![task("t1"), task("t3")], max_loops: 3 };
        let results = vec![success_result("t1")];

        let merged = merge_plan(new_plan, &old_plan, &results);

        assert_eq!(merged.tasks.len(), 2);
        // t1 合并了 output
        assert!(merged.tasks.iter().find(|t| t.id == "t1").unwrap().output.is_some());
        // t3 是新任务，无合并
        assert!(merged.tasks.iter().find(|t| t.id == "t3").unwrap().output.is_none());
    }

    #[test]
    fn test_merge_plan_failed_task_not_merged() {
        // t1 有旧 output 但 task_results 显示它失败 → 不应保留 output（需重跑）
        let mut old_t1 = task("t1");
        old_t1.output = Some("old output".into());
        let old_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![old_t1], max_loops: 3 };
        let new_plan = TaskPlan { overall_goal: "test".into(), tasks: vec![task("t1")], max_loops: 3 };
        // t1 失败（不在 success_results 中）
        let results = vec![TaskResult {
            task_id: "t1".into(), success: false,
            output: String::new(), error: Some("failed".into()), tokens_used: 50,
            validation_report: None,
            arbiter_decision: None,
        }];

        let merged = merge_plan(new_plan, &old_plan, &results);
        let t1 = &merged.tasks[0];
        assert!(t1.output.is_none(), "failed task should not preserve output");
    }
}

/// P-多智能体规划：枚举目标的独立并行工作项（标题, 角色）。返回 None
/// 表示枚举失败（回退到既有 LLM 分解路径）；返回空/单项目表示任务
/// 无需拆分。角色从 researcher/executor/analyst/writer 中指派。
pub async fn enumerate_work_items(
    provider: &dyn miniagent_provider::traits::LlmProvider,
    goal: &str,
    cancel: CancellationToken,
) -> Option<Vec<(String, String)>> {
    use miniagent_core::config::InferenceConfig;
    use miniagent_core::event::ContentBlock;
    use miniagent_core::message::Message;
    use miniagent_provider::traits::CompletionRequest;

    let prompt = format!(
        r#"Break this goal into its independent parallel work items. Each item must be
independently completable by one agent and produce its own result.

Goal:
{goal}

Rules:
- 2 to 5 items; one item per independent subject/deliverable
- Do NOT include a final "summary" item (the pipeline adds one)
- role ∈ researcher | executor | analyst

Output ONLY valid JSON:
{{"items":[{{"title":"<work item>","role":"researcher"}}]}}"#
    );
    let request = CompletionRequest {
        system: "You decompose goals into independent parallel work items. Output ONLY valid JSON."
            .into(),
        messages: vec![Message::user(&prompt)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.2),
            max_tokens: Some(2_048),
            ..Default::default()
        },
    };

    let response = provider.complete(&request, cancel.child_token()).await.ok()?;
    let text: String = response
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let cleaned = miniagent_core::json_util::extract_and_repair(&text);

    #[derive(serde::Deserialize)]
    struct Item {
        title: String,
        #[serde(default = "default_role")]
        role: String,
    }
    fn default_role() -> String {
        "researcher".into()
    }
    #[derive(serde::Deserialize)]
    struct Items {
        #[serde(default)]
        items: Vec<Item>,
    }

    let parsed: Items = serde_json::from_str(&cleaned).ok()?;
    let role_ok = ["researcher", "executor", "analyst", "writer"];
    let items: Vec<(String, String)> = parsed
        .items
        .into_iter()
        .filter(|i| !i.title.trim().is_empty())
        .map(|i| {
            let role = if role_ok.contains(&i.role.as_str()) {
                i.role
            } else {
                "researcher".into()
            };
            (i.title, role)
        })
        .collect();
    if items.len() >= 2 { Some(items) } else { None }
}
