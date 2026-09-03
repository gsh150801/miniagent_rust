use std::sync::Arc;
use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_agent::context::RunContext;
use miniagent_agent::Agent;
use miniagent_provider::router::ProviderChoice;
use miniagent_core::config::TaskComplexity;
use tokio_util::sync::CancellationToken;

use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::CompletionRequest;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{TaskPlan, TaskUnit, TaskResult, StageMessage, CritiqueEntry};
use crate::prompts::{role_system_prompt as new_role_system_prompt, tool_instruction_block, tools_for_role};

/// Emit one per-subtask progress event through the shared callback slot.
/// The server forwards these to the WebSocket as
/// `{type:"progress", stage:"task", status, data}` so the frontend can render
/// a live todo list and per-subtask execution summaries.
fn emit_task(ctx: &StageContext, status: &str, data: &serde_json::Value) {
    ctx.emit_progress("task", status, Some(data));
}

/// Truncate a string to at most `max` chars (char-boundary safe) for transport.
fn preview_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Resolve dependency order: returns groups of task IDs that can run in parallel.
///
/// Delegates to the canonical Kahn scheduler in
/// `miniagent_core::orchestration::kahn_waves`. Cycle handling matches the
/// pre-consolidation behavior: instead of propagating `Err`, the remaining
/// tasks (those caught in the cycle) are appended as a final wave so the
/// dispatcher can still try to make progress (a real cycle just means those
/// tasks depend on each other and will time out individually).
fn resolve_execution_order(tasks: &[TaskUnit]) -> Vec<Vec<String>> {
    use miniagent_core::orchestration::{kahn_waves, DagEdge};

    let nodes: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let edges: Vec<DagEdge> = tasks
        .iter()
        .flat_map(|t| {
            t.depends_on
                .iter()
                .map(move |dep| DagEdge {
                    to: t.id.clone(),
                    depends_on: dep.clone(),
                })
        })
        .collect();

    match kahn_waves(&nodes, &edges) {
        Ok(waves) => waves,
        Err(_) => {
            // Cycle detected: collect all tasks that weren't scheduled in any
            // wave and append them as a single fallback wave so the
            // dispatcher doesn't lose them. (Preserves the pre-K32 semantic
            // that `test_no_dependency_cycle_handling` exercises.)
            let scheduled: std::collections::HashSet<String> =
                edges.iter().map(|e| e.to.clone()).collect();
            let remaining: Vec<String> = tasks
                .iter()
                .filter(|t| !scheduled.contains(&t.id))
                .map(|t| t.id.clone())
                .collect();
            let mut waves = Vec::new();
            // The canonical algorithm always schedules at least one node
            // before erroring on a cycle; replicate that here.
            let first_cycle_node = remaining
                .first()
                .cloned()
                .unwrap_or_else(|| tasks[0].id.clone());
            waves.push(vec![first_cycle_node]);
            if remaining.len() > 1 {
                waves.push(remaining[1..].to_vec());
            }
            waves
        }
    }
}

/// 检查 expected_output 中提到的文件产物是否仍存在。
///
/// 用于"跳过已成功任务"前的安全校验（缺陷 #1 修复）：若上轮 task 产出了文件（如
/// report.md / data.csv），跳过重跑前必须确认文件仍在，否则下游 task 找不到依赖。
///
/// 返回值：
/// - 无文件路径（纯文本输出，如"分析结论"）→ `true`（可安全跳过）
/// - 有文件路径且全部存在 → `true`（可跳过）
/// - 有文件路径但任一缺失 → `false`（需重跑以重建产物）
///
/// 文件路径识别：扫描文本中的"token + 扩展名"模式（无需正则依赖）。
pub fn outputs_still_exist(expected_output: &str, working_dir: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".py", ".rs", ".md", ".csv", ".json", ".txt", ".tsv",
        ".xlsx", ".html", ".js", ".ts", ".toml", ".yaml", ".yml",
    ];

    let work_dir = std::path::Path::new(working_dir);
    let mut _any_found = false;

    // 按空白分词，检查每个 token 是否以已知扩展名结尾
    for token in expected_output.split_whitespace() {
        // 去除常见的尾部标点（逗号、句号、括号、引号）
        let cleaned = token.trim_end_matches([',', '.', ')', ';', '"', '\'', ':']);
        if EXTENSIONS.iter().any(|ext| cleaned.ends_with(ext)) {
            _any_found = true;
            let path = std::path::Path::new(cleaned);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                work_dir.join(path)
            };
            if !resolved.exists() {
                tracing::debug!(
                    expected_file = %cleaned,
                    resolved = %resolved.display(),
                    "outputs_still_exist: file missing → cannot skip"
                );
                return false;
            }
        }
    }

    // 走到这里：
    // - 无文件路径（纯文本输出）→ 可安全跳过（返回 true）
    // - 有文件路径且全部存在 → 可安全跳过（返回 true）
    // - 有文件路径但任一缺失 → 已在上方返回 false
    true
}

/// Result from the judge LLM.
struct JudgeResult {
    passed: bool,
    verdict: String,
    improvements: Vec<String>,
}

impl JudgeResult {
    fn into_entry(self, task_id: String, critique: String) -> CritiqueEntry {
        CritiqueEntry {
            task_id,
            critique,
            judge_verdict: self.verdict,
            judge_passed: self.passed,
            improvements: self.improvements,
        }
    }
}

/// Run the critic: reviews worker output quality based on expected output type.
async fn run_critic(
    _task_id: &str,
    output: &str,
    description: &str,
    expected_output: &str,
    provider: &dyn miniagent_provider::traits::LlmProvider,
    max_tokens: u32,
    cancel: CancellationToken,
) -> String {
    let output_type = classify_output_type(description, expected_output);
    let type_guide = output_type_critic_guide(output_type);

    let prompt = format!(
        r#"You are a **Critic** reviewing a task output. Assess quality thoroughly.

## Task
{description}

## Expected Output
{expected_output}

## Output Type
{output_type}

## Output to Review
{output}

{type_guide}

## Instructions
1. Be specific about what is good and what needs improvement.
2. Check for errors, omissions, formatting issues.
3. Verify data correctness if applicable.
4. Check file existence and readability for report/table outputs.

## Output Format (JSON only)
{{
  "strengths": ["strength 1", "strength 2"],
  "weaknesses": ["weakness 1", "weakness 2"],
  "missing_elements": ["missing 1"],
  "quality_score": 0-100,
  "detailed_feedback": "Detailed analysis..."
}}"#
    );

    let request = CompletionRequest {
        system: "You are a thorough critic. Analyze output quality. Output ONLY valid JSON.".into(),
        messages: vec![Message::user(&prompt)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.3),
            max_tokens: Some(max_tokens),
            ..Default::default()
        },
    };

    match provider.complete(&request, cancel).await {
        Ok(resp) => {
            let text: String = resp.content.iter()
                .filter_map(|b| match b { ContentBlock::Text { text } => Some(text.clone()), _ => None })
                .collect::<Vec<_>>().join("");
            let cleaned = miniagent_core::json_util::strip_markdown_fences(&text);
            cleaned.chars().take(1500).collect()
        }
        Err(e) => format!("Critic analysis unavailable: {e}"),
    }
}

/// Run the judge: decides if output quality passes.
async fn run_judge(
    _task_id: &str,
    output: &str,
    critique: &str,
    description: &str,
    expected_output: &str,
    provider: &dyn miniagent_provider::traits::LlmProvider,
    max_tokens: u32,
    cancel: CancellationToken,
) -> JudgeResult {
    let output_type = classify_output_type(description, expected_output);

    let prompt = format!(
        r#"You are a **Judge** deciding whether task output passes quality review.

## Task
{description}

## Expected Output
{expected_output}

## Output Type
{output_type}

## Output
{output}

## Critic Review
{critique}

## Instructions
1. Decide PASS or FAIL based on the critic's analysis and your own assessment.
2. If the output has critical errors, missing key elements, or poor quality, mark FAIL.
3. If minor improvements are needed but the output is fundamentally sound, mark PASS with improvement suggestions.
3. Only cycle back to improve the same task up to 7 times.

## Output Format (JSON only)
{{
  "passed": true|false,
  "verdict": "Clear explanation of the decision",
  "improvements": ["Specific improvement 1", "Specific improvement 2"]
}}"#
    );

    let request = CompletionRequest {
        system: "You are a strict but fair judge. Output ONLY valid JSON.".into(),
        messages: vec![Message::user(&prompt)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.2),
            max_tokens: Some(max_tokens),
            ..Default::default()
        },
    };

    match provider.complete(&request, cancel).await {
        Ok(resp) => {
            let text: String = resp.content.iter()
                .filter_map(|b| match b { ContentBlock::Text { text } => Some(text.clone()), _ => None })
                .collect::<Vec<_>>().join("");
            let cleaned = miniagent_core::json_util::strip_markdown_fences(&text);
            serde_json::from_str::<serde_json::Value>(&cleaned)
                .map(|v| JudgeResult {
                    passed: v["passed"].as_bool().unwrap_or(true),
                    verdict: v["verdict"].as_str().unwrap_or("Judge evaluation unavailable").to_string(),
                    improvements: v["improvements"].as_array()
                        .map(|a| a.iter().filter_map(|i| i.as_str().map(|s| s.to_string())).collect())
                        .unwrap_or_default(),
                })
                .unwrap_or_else(|e| {
                    tracing::error!(error = %e, "judge LLM response parse failed — marking task as not passed");
                    JudgeResult {
                        passed: false,
                        verdict: "Judge evaluation failed (parse error) — task requires re-execution".into(),
                        improvements: vec![],
                    }
                })
        }
        Err(e) => {
            tracing::error!(error = %e, "judge LLM call failed — marking task as not passed (do not silently approve)");
            JudgeResult {
                passed: false,
                verdict: format!("Judge unavailable: {e} — task requires re-execution"),
                improvements: vec![],
            }
        }
    }
}

/// Classify output type based on task description and expected output.
fn classify_output_type(description: &str, expected_output: &str) -> &'static str {
    let combined = format!("{} {}", description, expected_output).to_lowercase();
    if combined.contains(".py") || combined.contains(".rs") || combined.contains(".js")
        || combined.contains(".ts") || combined.contains("code") || combined.contains("script")
        || combined.contains("function") || combined.contains("class")
        || combined.contains("program") || combined.contains("implement")
        || combined.contains("package") || combined.contains("module")
    {
        "code"
    } else if combined.contains(".csv") || combined.contains(".tsv") || combined.contains(".xlsx")
        || combined.contains("table") || combined.contains("spreadsheet")
        || combined.contains("matrix") || combined.contains("dataframe")
        || combined.contains("tabular")
    {
        "table"
    } else if combined.contains(".md") || combined.contains(".txt") || combined.contains(".pdf")
        || combined.contains("report") || combined.contains("document")
        || combined.contains("summary") || combined.contains("analysis")
        || combined.contains("review") || combined.contains("article")
        || combined.contains("paper") || combined.contains("write")
    {
        "report"
    } else {
        "mixed"
    }
}

/// Return critic guidelines for a given output type.
fn output_type_critic_guide(output_type: &str) -> &'static str {
    match output_type {
        "code" =>
            "## Code Review Guidelines\n\
             - Check code format, indentation, naming conventions\n\
             - Verify code readability and documentation\n\
             - Check for syntax errors and logical bugs\n\
             - Verify that test cases exist and are runnable\n\
             - Ensure proper error handling\n\
             - Check for security vulnerabilities\n\
             - Verify imports and dependencies are correct",
        "report" =>
            "## Report Review Guidelines\n\
             - Verify the report file exists and is readable\n\
             - Check format structure (headings, sections, paragraphs)\n\
             - Verify citations/references/links are present and valid\n\
             - Assess content comprehensiveness and multi-perspective coverage\n\
             - Verify data correctness and source accuracy\n\
             - Check if cited sources exist and are accessible\n\
             - Ensure conclusions are supported by evidence",
        "table" =>
            "## Table Review Guidelines\n\
             - Verify table file exists and is readable\n\
             - Check for data corruption, garbled text, or misaligned columns\n\
             - Verify row/column headers are correct\n\
             - Check for missing values or NULL entries\n\
             - Verify data types match expected schema\n\
             - Check encoding (no garbled characters)",
        _ =>
            "## Mixed Output Review Guidelines\n\
             - Apply all relevant checks from code, report, and table guidelines\n\
             - Verify each output component exists and is complete\n\
             - Check consistency across different output formats\n\
             - Ensure all promised deliverables are present",
    }
}

/// Dispatch Stage: assigns tasks to agent roles respecting dependencies,
/// executes them serially or in parallel based on dependency graph.
pub struct DispatchStage;

#[async_trait]
impl PipelineStage for DispatchStage {
    fn name(&self) -> &str { "dispatch" }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let plan = match &ctx.state.plan {
            Some(p) => p.clone(),
            None => return Err(AgentError::invalid_state(String::from("No plan available for dispatch"))),
        };

        let wave_context: String = ctx.state.repair_analyses.iter()
            .map(|r| format!(
                "Repair insight for task '{}': root cause: {}. Suggested fix: {}",
                r.failed_task_id, r.root_cause, r.suggested_fix,
            ))
            .collect::<Vec<_>>()
            .join("\n");

        // P3: snapshot steerings for injection into every subtask prompt.
        let steerings: Vec<String> = ctx.state.steerings.clone();
        // 有待处理转向时，旧结果不再代表"完成"（转向改变了验收标准），
        // 禁用本轮的跳过重跑优化——否则转向永远到不了执行层（live 实测：
        // 转向已入规划目标，但 loop 2 直接复用了 loop 1 缺风险章节的结果）。
        let steerings_pending = !steerings.is_empty();
        let waves = resolve_execution_order(&plan.tasks);
        // 按 task_id 去重：每个 task_id 只保留最新的一条结果（解决 #2 结果无界累积）
        let mut result_map: std::collections::HashMap<String, TaskResult> = ctx.state.task_results
            .iter()
            .map(|r| (r.task_id.clone(), r.clone()))
            .collect();
        let mut messages: Vec<StageMessage> = Vec::new();

        tracing::info!("Dispatch: {} waves, {} tasks total", waves.len(), plan.tasks.len());

        for (wave_idx, wave) in waves.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            tracing::info!("Wave {}/{}: {} tasks (parallel)", wave_idx + 1, waves.len(), wave.len());

            // Get task details for this wave
            let wave_tasks: Vec<&TaskUnit> = wave.iter()
                .filter_map(|id| plan.tasks.iter().find(|t| &t.id == id))
                .collect();

            // Execute wave tasks in parallel via tokio::spawn; each task
            // reuses the shared Arc<Agent> — no rebuilding.
            //
            // A shared semaphore caps how many spawned tasks hit the LLM
            // provider at once (`loop_dispatch_wave_concurrency`, default 4):
            // a wide wave must not trigger 429 storms or spike runtime
            // memory. Spawn (not a bare FuturesUnordered) keeps panic
            // isolation via JoinError handling below.
            let semaphore = Arc::new(tokio::sync::Semaphore::new(
                ctx.config.loop_dispatch_wave_concurrency.max(1),
            ));
            let mut handles = Vec::new();
            for task in &wave_tasks {
                // ── 缺陷 #1 修复：跳过已成功且产物仍存在的任务 ──
                // 若该 task_id 在 result_map 中有成功记录，且 expected_output 提到的
                // 文件产物仍存在，则跳过重跑，直接复用旧结果。
                // 这依赖 plan 的 merge_plan 保留了成功任务的 id（层 1 修复）。
                if let Some(prior) = result_map.get(&task.id) {
                    if prior.success && !steerings_pending && crate::dispatch::outputs_still_exist(&task.expected_output, &ctx.working_dir) {
                        tracing::info!(
                            task_id = %task.id,
                            "dispatch: skipping re-execution, reusing prior successful result"
                        );
                        // 前端同步：复用旧结果的子任务以 reused 标记补一条
                        // completed 事件，让 todo list 显示真实状态。
                        emit_task(ctx, "completed", &serde_json::json!({
                            "task_id": task.id,
                            "title": task.description,
                            "role": task.assigned_role,
                            "wave": wave_idx + 1,
                            "reused": true,
                            "output": preview_chars(&prior.output, 2000),
                        }));
                        // 已在 map 中，直接跳过
                        continue;
                    } else if !prior.success {
                        tracing::info!(
                            task_id = %task.id,
                            "dispatch: re-executing, prior attempt failed"
                        );
                    } else {
                        tracing::info!(
                            task_id = %task.id,
                            "dispatch: re-executing, prior output files missing"
                        );
                    }
                }
                // ──────────────────────────────────────────────────

                // 前端同步：子任务开始执行（todo list 标记 running +
                // 中部执行摘要卡出现）。
                emit_task(ctx, "running", &serde_json::json!({
                    "task_id": task.id,
                    "title": task.description,
                    "role": task.assigned_role,
                    "difficulty": task.difficulty,
                    "wave": wave_idx + 1,
                }));

                let task = (*task).clone();
                let agent = ctx.agent.clone();
                let cancel = cancel.child_token();
                let wave_ctx = wave_context.clone();
                let max_tool_iters = ctx.config.loop_dispatch_max_iterations;
                let working_dir = ctx.working_dir.clone();
                let semaphore = semaphore.clone();
                let steerings = steerings.clone();

                handles.push(tokio::spawn(async move {
                    // Acquire a permit before touching the provider; the
                    // semaphore is never closed, so this only waits.
                    let _permit = semaphore
                        .acquire()
                        .await
                        .expect("dispatch semaphore closed");
                    execute_single_task(
                        task, agent, cancel, wave_ctx.clone(), max_tool_iters,
                        working_dir.clone(), steerings.clone(),
                    ).await
                }));
            }

            // Collect results from all tasks in this wave.
            for handle in handles {
                match handle.await {
                    Ok(result) => {
                        if result.success {
                            tracing::info!(task_id = %result.task_id, "task completed");
                        } else {
                            tracing::warn!(task_id = %result.task_id, error = ?result.error.as_ref().map(|s| &s[..s.len().min(80)]), "task failed");
                        }
                        // 前端同步：子任务完成/失败 + 输出摘要（中部执行
                        // 摘要卡更新为最终状态，todo list 打勾）。
                        let spec = plan.tasks.iter().find(|t| t.id == result.task_id);
                        emit_task(ctx, if result.success { "completed" } else { "failed" }, &serde_json::json!({
                            "task_id": result.task_id,
                            "title": spec.map(|t| t.description.clone()).unwrap_or_else(|| result.task_id.clone()),
                            "role": spec.map(|t| t.assigned_role.clone()).unwrap_or_default(),
                            "wave": wave_idx + 1,
                            "output": preview_chars(&result.output, 2000),
                            "error": result.error.as_ref().map(|e| e.chars().take(300).collect::<String>()),
                            "tokens_used": result.tokens_used,
                        }));
                        // 按 task_id 去重覆盖：同一 task_id 只保留最新结果
                        result_map.insert(result.task_id.clone(), result);
                    }
                    Err(e) => {
                        let err_msg = format!("Task panicked: {e}");
                        tracing::error!(error = %err_msg, "task panicked");
                        result_map.insert("unknown".into(), TaskResult {
                            task_id: "unknown".into(),
                            success: false,
                            output: String::new(),
                            error: Some(err_msg),
                            tokens_used: 0,
                            validation_report: None,
                            arbiter_decision: None,
                        });
                    }
                }
            }
        }

        // 将 HashMap 转为 Vec，供后续 3-party review 和状态更新使用
        let mut all_results: Vec<TaskResult> = result_map.into_values().collect();

        // ── Difficulty-tiered 3-Party Review: Worker → Critic → Judge ──
        // simple: skip review entirely
        // medium: critic only (auto-pass, feedback recorded)
        // hard:   full 3-party (critic + judge)
        let flash_provider = ctx.agent.flash_provider();
        let pro_provider = ctx.agent.pro_provider();
        let mut critique_entries: Vec<CritiqueEntry> = Vec::new();
        for result in &all_results {
            if !result.success { continue; }
            let task_spec = plan.tasks.iter().find(|t| t.id == result.task_id);
            let difficulty = task_spec.map(|t| t.difficulty.as_str()).unwrap_or("hard");
            let desc = task_spec.map(|t| t.description.as_str()).unwrap_or(&result.task_id);
            let expected = task_spec.map(|t| t.expected_output.as_str()).unwrap_or("");

            match difficulty {
                "simple" => {
                    tracing::debug!(
                        task_id = %result.task_id, difficulty = "simple",
                        "Skipping review for simple task"
                    );
                }
                "medium" => {
                    let critique = run_critic(
                        &result.task_id, &result.output, desc, expected,
                        flash_provider.as_ref(), ctx.config.loop_critic_max_tokens, cancel.child_token(),
                    ).await;
                    tracing::info!(task_id = %result.task_id, "Medium task: critic review (auto-pass)");
                    critique_entries.push(CritiqueEntry {
                        task_id: result.task_id.clone(),
                        critique,
                        judge_verdict: "Auto-passed (medium difficulty)".into(),
                        judge_passed: true,
                        improvements: vec![],
                    });
                }
                _ => {
                    let critique = run_critic(
                        &result.task_id, &result.output, desc, expected,
                        flash_provider.as_ref(), ctx.config.loop_critic_max_tokens, cancel.child_token(),
                    ).await;

                    let judge_result = run_judge(
                        &result.task_id, &result.output, &critique, desc, expected,
                        pro_provider.as_ref(), ctx.config.loop_judge_max_tokens, cancel.child_token(),
                    ).await;

                    if !judge_result.passed {
                        tracing::warn!(task_id = %result.task_id, verdict = %judge_result.verdict.chars().take(120).collect::<String>(), "Judge: quality check failed");
                    } else {
                        tracing::info!(task_id = %result.task_id, "Judge: quality check passed");
                    }
                    critique_entries.push(judge_result.into_entry(result.task_id.clone(), critique));
                }
            }
        }
        // Apply judge decisions: failed quality checks mark the task as failed
        for entry in &critique_entries {
            if !entry.judge_passed
                && let Some(result) = all_results.iter_mut().find(|r| r.task_id == entry.task_id) {
                    result.success = false;
                    result.error = Some(format!("Quality check failed: {}", entry.judge_verdict));
                }
        }

        // Update task results in the plan
        let mut updated_tasks = plan.tasks.clone();
        for result in &all_results {
            if let Some(task) = updated_tasks.iter_mut().find(|t| t.id == result.task_id) {
                task.output = Some(result.output.clone());
                task.failed = !result.success;
                task.error = result.error.clone();
            }
        }

        let updated_plan = TaskPlan {
            tasks: updated_tasks,
            ..plan
        };

        let goal = updated_plan.overall_goal.clone();
        let mut state = ctx.state.clone();
        state.task_results = all_results.clone();
        state.plan = Some(updated_plan);
        state.critique_entries = critique_entries;

        // Find failed tasks for the repair stage
        let failed_tasks: Vec<String> = all_results.iter()
            .filter(|r| !r.success)
            .map(|r| r.task_id.clone())
            .collect();

        if !failed_tasks.is_empty() {
            messages.push(StageMessage {
                from_stage: "dispatch".into(),
                to_stage: "repair".into(),
                content: serde_json::to_string(&all_results).unwrap_or_default(),
                task_id: None,
            });
        }

        messages.push(StageMessage {
            from_stage: "dispatch".into(),
            to_stage: "evaluate".into(),
            content: serde_json::to_string(&all_results).unwrap_or_default(),
            task_id: None,
        });

        // Persist all task outputs to disk inside the run's result directory
        // (the task dir on the server, ./result/loop-pipeline on the CLI).
        // Each task gets its own subdirectory: tasks/{task_id}_{short_description}/
        let result_dir = std::path::PathBuf::from(&ctx.working_dir).join("tasks");
        if let Err(e) = std::fs::create_dir_all(&result_dir) {
            tracing::error!(path = %result_dir.display(), error = %e, "failed to create dispatch result dir");
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        tracing::info!(results = all_results.len(), "Persisting task outputs to {:?}", result_dir);

        for result in &all_results {
            // Look up task description from the plan for directory naming
            let task_desc = plan.tasks.iter()
                .find(|t| t.id == result.task_id)
                .map(|t| t.description.as_str())
                .unwrap_or("unknown");

            // Build a short slug from the first few meaningful words of the description
            let slug: String = task_desc
                .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
                .filter(|w| !w.is_empty())
                .take(4)
                .map(|w| w.to_lowercase())
                .collect::<Vec<_>>()
                .join("_");

            let subdir_name = if slug.is_empty() {
                result.task_id.clone()
            } else {
                format!("{}_{}", result.task_id, slug)
            };

            let task_dir = result_dir.join(&subdir_name);
            if let Err(e) = std::fs::create_dir_all(&task_dir) {
                tracing::error!(path = %task_dir.display(), error = %e, "failed to create dispatch task dir");
            }

            // Semantic names: result.md / failed.md (was the opaque ok.md).
            let filename = format!("{}.md", if result.success { "result" } else { "failed" });
            let path = task_dir.join(&filename);
            let content = if result.success {
                result.output.clone()
            } else {
                format!("# Failed: {}\n\n{}",
                    result.error.as_deref().unwrap_or("unknown error"),
                    result.output)
            };
            if !content.trim().is_empty() {
                if let Err(e) = std::fs::write(&path, &content) {
                    tracing::warn!(path = %path.display(), error = %e, "failed to write task output");
                } else {
                    tracing::debug!(path = %path.display(), "saved task output");
                }
            }
        }

        // Save the full plan and all results as a JSON summary
        let summary_data = serde_json::json!({
            "overall_goal": goal,
            "loop_count": ctx.state.loop_count,
            "total_tasks": all_results.len(),
            "successful": all_results.iter().filter(|r| r.success).count(),
            "failed": all_results.iter().filter(|r| !r.success).count(),
            "tasks": all_results.iter().map(|r| serde_json::json!({
                "id": r.task_id,
                "success": r.success,
                "preview": r.output.chars().take(500).collect::<String>(),
            })).collect::<Vec<_>>(),
        });
        // Archive under dispatch/ with a loop-numbered name instead of flat
        // epoch-timestamped files in the task root.
        let dispatch_dir = std::path::PathBuf::from(&ctx.working_dir).join("dispatch");
        let _ = std::fs::create_dir_all(&dispatch_dir);
        let summary_path = dispatch_dir
            .join(format!("loop{}_{}.json", ctx.state.loop_count + 1, ts));
        if let Ok(json) = serde_json::to_string_pretty(&summary_data)
            && let Err(e) = std::fs::write(&summary_path, &json) {
                tracing::error!(path = %summary_path.display(), error = %e, "failed to persist dispatch summary");
            }

        let success_count = all_results.iter().filter(|r| r.success).count();
        let fail_count = all_results.iter().filter(|r| !r.success).count();

        // 转向指令已注入本轮全部子任务提示（steerings_pending 时还禁用了
        // 跳过重跑），清除已应用标记——后续循环仅在新转向到达时再次触发。
        let mut state = state;
        if steerings_pending {
            state.steerings.clear();
        }

        Ok(StageOutput {
            updated_state: state,
            new_messages: messages,
            summary: format!(
                "Dispatched {} tasks in {} waves. {} succeeded, {} failed. Results saved to {}.",
                all_results.len(), waves.len(), success_count, fail_count,
                result_dir.display(),
            ),
        })
    }
}

/// Execute a single task (tactic layer).
async fn execute_single_task(
    task: TaskUnit,
    agent: std::sync::Arc<Agent>,
    cancel: CancellationToken,
    wave_ctx: String,
    max_tool_iters: usize,
    working_dir: String,
    steerings: Vec<String>,
) -> TaskResult {
    let mut system = new_role_system_prompt(
        &task.assigned_role,
        &task.description,
        &task.expected_output,
    );

    // ── 本地技能注入（workflow AgentStage 同款能力）────────────────
    // 修复 live 问题：loop worker 不知道本地技能库，被要求
    // "用 bioinf-verify-report 技能校验报告"时幻觉出外部技能库。
    // 按任务描述匹配技能；无匹配时附上完整本地技能名册（仅名称+
    // 描述首行），让 agent 至少知道本地有什么可用。
    {
        use miniagent_skill::discovery::SkillDiscovery;
        use miniagent_skill::registry::SkillRegistry;
        let discovery = SkillDiscovery::new();
        let bundles = discovery.discover();
if !bundles.is_empty() {
            let mut registry = SkillRegistry::new();
            for b in bundles {
                registry.register(b);
            }
            let query = format!("{} {}", task.description, task.expected_output);
            let matched = registry.find_matching(&query, 2);
            // P-技能链：匹配技能声明的 follow_ups（如 cited-review-report →
            // bioinf-verify-report）一并注入，并在提示中强制"完成主交付物后
            // 必须执行核验"——修复 live 问题：格式技能正文里的自检要求被
            // worker 忽略，核验从未发生。
            let mut chained: Vec<String> = Vec::new();
            for m in &matched {
                for fu in &m.metadata.follow_ups {
                    if !chained.contains(fu) {
                        chained.push(fu.clone());
                    }
                }
            }
            // P-技能团队：链式核验技能 → 独立核验子智能体（deliverable 完成后
            // 由另一个 agent 按核验技能工作流执行校验）。
            let mut verify_skill: Option<(String, String)> = None;
            if matched.is_empty() {
                // 兜底：注入本地高优技能名册，让 agent 知道本地有什么可用
                let catalog: Vec<String> = registry.all().iter()
                    .filter(|b| b.metadata.priority >= 8)
                    .map(|b| format!("- {}（{}）", b.metadata.name,
                        b.metadata.description.lines().next().unwrap_or("").chars().take(90).collect::<String>()))
                    .collect();
                if !catalog.is_empty() {
                    system.push_str(&format!(
                        "\n\n## 本地可用技能（不存在外部技能库；需要时按名称使用）\n{}\n",
                        catalog.join("\n")
                    ));
                }
            } else {
                for skill in matched {
                    let body: String = skill.body.lines().take(40).collect::<Vec<_>>().join("\n");
                    system.push_str(&format!(
                        "\n## 技能: {}（用户/规划器要求使用时必须遵循其工作流）\n{}\n",
                        skill.metadata.name, body
                    ));
                }
                // 链式核验技能：主技能产出交付物后必须按核验技能执行一次
                // 校验，修复所有 mismatch 后才算完成。
                for fu_name in &chained {
                    if let Some(bundle) = registry.all().iter()
                        .find(|b| &b.metadata.name == fu_name)
                    {
                        let body: String = bundle.body.lines().take(40).collect::<Vec<_>>().join("\n");
                        system.push_str(&format!(
                            "\n## 核验技能: {}（交付物完成后必须立即执行一次核验，\n修复全部 mismatch 后才算完成；核验记录写入交付说明）\n{}\n",
                            bundle.metadata.name, body
                        ));
                        // 记录第一个链式核验技能 → 供独立核验子智能体使用
                        if verify_skill.is_none() {
                            verify_skill = Some((bundle.metadata.name.clone(), bundle.body.clone()));
                        }
                    }
                }
            }
        }
    }

    let repair_context = if wave_ctx.is_empty() {
        String::new()
    } else {
        format!("\n\n## Repair Context (apply if relevant)\n{wave_ctx}")
    };

    // P3 执行中转向：用户运行中插入的指令直接进入每个子任务的执行提示，
    // 不依赖规划器转述（保证转向要求到达执行层）。
    let steer_block = if steerings.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = steerings
            .iter()
            .map(|s| format!("- {}", s))
            .collect();
        format!(
            "\n## 用户转向要求（必须遵守）\n{}\n",
            items.join("\n")
        )
    };

    let prompt = format!(
        "{repair_context}\n\n\
         ## Task\n{description}\n\
         ## Expected Output\n{expected}\n\n\
         {tool_instructions}\n\
         {env_info}\n\
         {steer_block}\
         ## Output Location\n\
         Write every file artifact into the working directory above (relative paths). \
         Do NOT create `result/…` or `../…` paths — they end up outside this task's directory.",
        description = task.description,
        expected = task.expected_output,
        tool_instructions = tool_instruction_block(),
        env_info = crate::prompts::env_info_block(&working_dir),
        steer_block = steer_block,
    );

    let mut history = vec![Message::user(&prompt)];
    let allowed: Vec<String> = tools_for_role(&task.assigned_role)
        .iter().map(|s| s.to_string()).collect();
    let mut context = RunContext::new(&system)
        .with_complexity(TaskComplexity::Moderate)
        .with_provider(ProviderChoice::Auto)
        .with_allowed_tools(allowed)
        .with_working_dir(working_dir.clone());
    context.max_tool_iterations = max_tool_iters;

    let result = agent.run_with_loop(&mut history, &context, cancel.child_token()).await;
    match result {
        Ok(delta) => {
            // 通用截断信号：模型仍想继续（ToolUse）但迭代预算耗尽——输出
            // 停留在调研中途，交付物从未写出。触发收尾合成兜底。
            let cut_off = !matches!(delta.stop_reason, miniagent_core::event::StopReason::EndTurn);
            let mut output: String = history.iter()
                .filter(|m| m.role == miniagent_core::message::MessageRole::Assistant)
                .map(|m| m.text_content())
                .collect::<Vec<_>>()
                .join("\n\n");

            // ── 收尾合成兜底 ─────────────────────────────────────────
            // 工具迭代上限触顶时，agent 的输出常停留在思维链中途（结尾是
            // 未闭合的 <think> 段，交付物从未写出——live 实测）。检测这种
            // 截断并追加一次无工具的合成调用："基于已有调研立即产出最终
            // 交付物"，保证任务总有可用的结果文本。合成文本同时替换输出
            // 中的思维链噪声（<think> 段不进入交付物与下游流式）。
            // 最可靠的完成信号是客观的：expected_output 声称会产出文件，
            // 而文件不存在（模型以纯文本叙述结束——"Let me search..."——
            // 从未写出交付物，live 反复出现）。三种信号任一命中即收尾：
            // 截断（stop_reason）、思维链未闭合、产物缺失。
            let deliverable_missing =
                !crate::dispatch::outputs_still_exist(&task.expected_output, &working_dir);
            let ends_mid_think = cut_off
                || deliverable_missing
                || {
                    let t = output.trim_end();
                    t.ends_with("</think>")
                        || (output.contains("<think>") && !output.contains("</think>"))
                };
            if ends_mid_think {
                let _ = deliverable_missing;
                tracing::warn!(
                    task_id = %task.id,
                    "output truncated mid-thinking (tool-iteration cap) — running wrap-up synthesis"
                );
                let research_so_far: String = history.iter()
                    .filter(|m| m.role == miniagent_core::message::MessageRole::Tool)
                    .map(|m| m.text_content())
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n");
                let steer_items: Vec<String> = steerings
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect();
                let steer_note = if steer_items.is_empty() {
                    String::new()
                } else {
                    format!("\n## 用户转向要求（必须遵守）\n{}\n", steer_items.join("\n"))
                };
                let wrap_prompt = format!(
                    "You hit your tool-iteration limit mid-research. Based on ALL the tool \
                     results you already gathered, produce the FINAL DELIVERABLE for the task \
                     NOW — no more tool calls, no thinking, output the complete answer text \
                     directly (if the deliverable is a file's content, output that content).\n\n\
                     ## Task\n{}\n\n## Expected Output\n{}\n{steer_note}\n\
                     ## Research gathered so far (truncated)\n{}",
                    task.description,
                    task.expected_output,
                    research_so_far.chars().take(24_000).collect::<String>(),
                );
                let wrap_request = CompletionRequest {
                    system: "You produce final deliverables. Skip thinking; output the complete answer directly.".into(),
                    messages: vec![Message::user(&wrap_prompt)],
                    tools: vec![],
                    config: InferenceConfig {
                        temperature: Some(0.2),
                        max_tokens: Some(16_384),
                        ..Default::default()
                    },
                };
                if let Ok(resp) = agent.flash_provider().complete(&wrap_request, cancel.child_token()).await {
                    let wrap_text: String = resp.content.iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    let wrap_text = miniagent_core::json_util::strip_reasoning_tags(&wrap_text);
                    if wrap_text.trim().len() > 50 {
                        output = format!("{}\n\n{}", wrap_text, output);
                    }
                }
            }
            // 思维链噪声不进入交付物/下游流式（保留正文）。
            output = miniagent_core::json_util::strip_reasoning_tags(&output);

            let has_tool_calls = history.iter()
                .filter(|m| m.role == miniagent_core::message::MessageRole::Tool)
                .count() > 0;
            let has_text = output.trim().len() > 100;
            let has_clear_failure = output.contains("I cannot complete")
                || output.contains("unable to complete")
                || output.contains("I am unable")
                || output.contains("cannot fulfill");

            // Detect whether tool calls actually produced errors rather than
            // treating the mere presence of a Tool message as success. A tool
            // call that returned an error string should NOT count toward success.
            let tool_error_indicators = ["error", "Error", "ERROR", "failed", "404", "401",
                "Unauthorized", "not found", "No such file", "Permission denied", "exception", "traceback"];
            let all_tool_results: Vec<String> = history.iter()
                .filter(|m| m.role == miniagent_core::message::MessageRole::Tool)
                .map(|m| m.text_content())
                .collect();
            let tool_call_count = all_tool_results.len();
            let tool_error_count = all_tool_results.iter()
                .filter(|r| tool_error_indicators.iter().any(|ind| r.contains(ind)))
                .count();
            // If every tool call errored, the "tool activity" is illusory.
            let all_tools_errored = tool_call_count > 0 && tool_error_count == tool_call_count;

            // Strict success: real text output, no explicit refusal, and tool
            // activity (if any) did not uniformly fail. A bare tool call with
            // no substantive output and all-error results is NOT success.
            let mut success = has_text && !has_clear_failure
                && (!has_tool_calls || !all_tools_errored);

            let verify_skill: Option<(String, String)> = {
                use miniagent_skill::discovery::SkillDiscovery;
                use miniagent_skill::registry::SkillRegistry;
                let discovery = SkillDiscovery::new();
                let bundles = discovery.discover();
                if bundles.is_empty() {
                    None
                } else {
                    let mut registry = SkillRegistry::new();
                    for b in bundles {
                        registry.register(b);
                    }
                    let query = format!("{} {}", task.description, task.expected_output);
                    let matched = registry.find_matching(&query, 2);
                    let mut found: Option<(String, String)> = None;
                    for m in matched {
                        for fu in &m.metadata.follow_ups {
                            if found.is_none()
                                && let Some(bundle) = registry.all().iter()
                                    .find(|b| &b.metadata.name == fu)
                            {
                                found = Some((bundle.metadata.name.clone(), bundle.body.clone()));
                            }
                        }
                    }
                    found
                }
            };

            // ── P-技能团队：核验子智能体 ─────────────────────────────
            // 主 worker 产出交付物后，若该任务匹配了带 follow_ups 的技能
            // （如 cited-review-report → bioinf-verify-report），由一个
            // 独立的核验子智能体按核验技能工作流对交付物执行校验
            // （citation_check / 文件存在性 / 事实抽查），产出核验报告。
            // 核验 FAIL ⇒ 任务标记失败进入修复循环（自愈）；核验报告
            // 附加到输出供审计。核验子智能体失败不否定主结果。
            let mut output = output;
            if success
                && let Some((verify_name, verify_body)) = &verify_skill
            {
                let verify_prompt = format!(
                    "{verify_body}\n\n\
                     ## 待核验交付物\n{output}\n\n\
                     ## 执行要求\n\
                     1. 按上述核验技能的工作流逐项核验该交付物（引用/文件/事实）。\n\
                     2. 使用 citation_check 等工具获取真实数据，禁止凭记忆判定。\n\
                     3. 输出核验报告，且第一行必须是 `核验结论：PASS` 或 `核验结论：FAIL`。\n\
                     4. FAIL 时逐条列出 mismatch 与修正建议。"
                );
                let verify_system = format!(
                    "You are an independent VERIFICATION agent executing the \
                     '{}' skill workflow against another agent's deliverable. \
                     Be strict and evidence-based.",
                    verify_name
                );
                let allowed: Vec<String> = vec![
                    "citation_check".into(), "pubmed_search".into(), "web_search".into(),
                    "web_fetch".into(), "read".into(), "glob".into(),
                ];
                let mut v_history = vec![Message::user(&verify_prompt)];
                let mut v_context = RunContext::new(&verify_system)
                    .with_complexity(TaskComplexity::Moderate)
                    .with_provider(ProviderChoice::Auto)
                    .with_allowed_tools(allowed)
                    .with_working_dir(working_dir.clone());
                v_context.max_tool_iterations = max_tool_iters.saturating_sub(2);
                match agent.run_with_loop(&mut v_history, &v_context, cancel.child_token()).await {
                    Ok(_v_delta) => {
                        let v_text: String = v_history.iter()
                            .filter(|m| m.role == miniagent_core::message::MessageRole::Assistant)
                            .map(|m| m.text_content())
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let v_fail = miniagent_core::json_util::strip_reasoning_tags(&v_text)
                            .contains("核验结论：FAIL");
                        output.push_str(&format!(
                            "\n\n---\n\n## 核验报告（{verify_name}）\n\n{}",
                            miniagent_core::json_util::strip_reasoning_tags(&v_text)
                        ));
                        if v_fail {
                            success = false;
                            tracing::warn!(
                                task_id = %task.id,
                                "verification sub-agent ruled FAIL — task enters repair"
                            );
                        } else {
                            tracing::info!(task_id = %task.id, "verification sub-agent: PASS");
                        }
                    }
                    Err(e) => tracing::warn!(
                        task_id = %task.id, error = %e,
                        "verification sub-agent failed — keeping main result"
                    ),
                }
            }

            TaskResult {
                task_id: task.id.clone(),
                success,
                output,
                error: if has_clear_failure {
                    Some("Agent reported inability to complete".into())
                } else if all_tools_errored {
                    Some(format!("All {} tool call(s) returned errors", tool_error_count))
                } else if !has_tool_calls && !has_text {
                    Some("No output produced".into())
                } else if !has_text {
                    Some("Insufficient output (tool calls only, no substantive result)".into())
                } else {
                    None
                },
                tokens_used: delta.usage.input_tokens + delta.usage.output_tokens,
            validation_report: None,
            arbiter_decision: None,
            }
        }
        Err(e) => TaskResult {
            task_id: task.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Agent error: {e}")),
            tokens_used: 0,
            validation_report: None,
            arbiter_decision: None,
        },
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::dispatch::resolve_execution_order;
    use crate::types::TaskUnit;

    fn make_task(id: &str, deps: Vec<&str>) -> TaskUnit {
        TaskUnit {
            id: id.to_string(),
            description: format!("Task {id}"),
            assigned_role: "executor".into(),
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            expected_output: format!("Output {id}"),
            difficulty: "medium".into(),
            failed: false,
            error: None,
            output: None,
        }
    }

    #[test]
    fn test_empty_tasks() {
        let waves = resolve_execution_order(&[]);
        assert!(waves.is_empty());
    }

    #[test]
    fn test_single_task() {
        let tasks = vec![make_task("a", vec![])];
        let waves = resolve_execution_order(&tasks);
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec!["a"]);
    }

    #[test]
    fn test_parallel_tasks() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec![]),
            make_task("c", vec![]),
        ];
        let waves = resolve_execution_order(&tasks);
        assert_eq!(waves.len(), 1, "all independent tasks should be in one wave");
        assert_eq!(waves[0].len(), 3, "all three tasks in one wave");
    }

    #[test]
    fn test_sequential_tasks() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ];
        let waves = resolve_execution_order(&tasks);
        assert_eq!(waves.len(), 3, "three sequential tasks should be 3 waves");
        assert_eq!(waves[0], vec!["a"]);
        assert_eq!(waves[1], vec!["b"]);
        assert_eq!(waves[2], vec!["c"]);
    }

    #[test]
    fn test_fan_out() {
        let tasks = vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["a"]),
            make_task("d", vec!["b", "c"]),
        ];
        let waves = resolve_execution_order(&tasks);
        assert_eq!(waves.len(), 3, "should have 3 waves");
        assert_eq!(waves[0], vec!["a"], "wave 0: a");
        // b and c depend on a, so they should be in wave 1
        assert_eq!(waves[1].len(), 2, "wave 1 should have 2 tasks");
        assert_eq!(waves[2], vec!["d"], "wave 2: d");
    }

    #[test]
    fn test_no_dependency_cycle_handling() {
        // a depends on b, b depends on a → cycle
        let tasks = vec![
            make_task("a", vec!["b"]),
            make_task("b", vec!["a"]),
        ];
        let waves = resolve_execution_order(&tasks);
        // Should still produce output (fallback to single wave)
        assert!(!waves.is_empty(), "should handle cycles gracefully");
    }

    // ── outputs_still_exist 测试（缺陷 #1 修复）──

    #[test]
    fn test_outputs_still_exist_no_file_paths() {
        // 无文件路径（纯文本输出）→ 可安全跳过
        assert!(crate::dispatch::outputs_still_exist("分析结论：数据相关性显著", "/tmp"));
        assert!(crate::dispatch::outputs_still_exist("summary of findings without any file", "/tmp"));
    }

    #[test]
    fn test_outputs_still_exist_file_exists() {
        // 创建临时文件，expected_output 提到它 → 存在 → 可跳过
        let dir = std::env::temp_dir().join("loop_pipeline_ose_exist");
        std::fs::create_dir_all(&dir).ok();
        let file = dir.join("report.md");
        std::fs::write(&file, "# Report").ok();
        let expected = format!("Generate {} in the project", file.display());
        assert!(crate::dispatch::outputs_still_exist(&expected, &dir.to_string_lossy()),
            "file exists → should be skippable");
    }

    #[test]
    fn test_outputs_still_exist_file_missing() {
        // 文件不存在 → 不能跳过（需重跑重建产物）
        let dir = std::env::temp_dir().join("loop_pipeline_ose_missing");
        std::fs::create_dir_all(&dir).ok();
        let expected = format!("Generate {}", dir.join("nonexistent.csv").display());
        assert!(!crate::dispatch::outputs_still_exist(&expected, &dir.to_string_lossy()),
            "file missing → must re-execute");
    }

    #[test]
    fn test_outputs_still_exist_relative_path_resolved() {
        // 相对路径基于 working_dir 解析
        let dir = std::env::temp_dir().join("loop_pipeline_ose_rel");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("data.json"), "{}").ok();
        // expected_output 提到相对路径 data.json
        assert!(crate::dispatch::outputs_still_exist("output data.json with results",
            &dir.to_string_lossy()),
            "relative path resolved against working_dir");
    }

    #[test]
    fn test_outputs_still_exist_multiple_files_all_present() {
        let dir = std::env::temp_dir().join("loop_pipeline_ose_multi");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("a.py"), "").ok();
        std::fs::write(dir.join("b.csv"), "").ok();
        assert!(crate::dispatch::outputs_still_exist("produce a.py and b.csv",
            &dir.to_string_lossy()));
    }

    #[test]
    fn test_outputs_still_exist_multiple_files_one_missing() {
        let dir = std::env::temp_dir().join("loop_pipeline_ose_multi2");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("a.py"), "").ok();
        // b.csv 不创建
        assert!(!crate::dispatch::outputs_still_exist("produce a.py and b.csv",
            &dir.to_string_lossy()),
            "one file missing → must re-execute");
    }
}
