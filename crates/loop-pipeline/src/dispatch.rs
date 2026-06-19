use std::collections::{HashMap, VecDeque};
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

/// Resolve dependency order: returns groups of task IDs that can run in parallel
fn resolve_execution_order(tasks: &[TaskUnit]) -> Vec<Vec<String>> {
    let mut task_map: HashMap<String, &TaskUnit> = HashMap::new();
    for t in tasks {
        task_map.insert(t.id.clone(), t);
    }

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();

    for t in tasks {
        in_degree.entry(t.id.clone()).or_insert(0);
        adj.entry(t.id.clone()).or_default();
        for dep in &t.depends_on {
            adj.entry(dep.clone()).or_default().push(t.id.clone());
            *in_degree.entry(t.id.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<String> = in_degree.iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut waves: Vec<Vec<String>> = Vec::new();
    let mut visited = 0;

    while !queue.is_empty() {
        let wave: Vec<String> = queue.drain(..).collect();
        visited += wave.len();
        waves.push(wave.clone());

        for id in &wave {
            if let Some(neighbors) = adj.get(id) {
                for next in neighbors {
                    if let Some(deg) = in_degree.get_mut(next) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(next.clone());
                        }
                    }
                }
            }
        }
    }

    if visited != tasks.len() {
        // Cycle detected: execute remaining tasks in a single wave
        let remaining: Vec<String> = tasks.iter()
            .filter(|t| !in_degree.get(&t.id).map_or(false, |d| *d == 0) || !waves.iter().any(|w| w.contains(&t.id)))
            .map(|t| t.id.clone())
            .collect();
        if !remaining.is_empty() {
            waves.push(remaining);
        }
    }

    waves
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
    let type_guide = output_type_critic_guide(&output_type);

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
                .unwrap_or_else(|_| JudgeResult {
                    passed: true,
                    verdict: "Judge evaluation unavailable (parse error)".into(),
                    improvements: vec![],
                })
        }
        Err(e) => JudgeResult {
            passed: true,
            verdict: format!("Judge unavailable: {e}"),
            improvements: vec![],
        },
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

        let waves = resolve_execution_order(&plan.tasks);
        let mut all_results: Vec<TaskResult> = ctx.state.task_results.clone();
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

            // Execute wave tasks in parallel via tokio::spawn
            // Each task reuses the shared Arc<Agent> — no rebuilding
            let mut handles = Vec::new();
            for task in &wave_tasks {
                let task = (*task).clone();
                let agent = ctx.agent.clone();
                let cancel = cancel.child_token();
                let wave_ctx = wave_context.clone();
                let max_tool_iters = ctx.config.loop_dispatch_max_iterations;
                let retrieval_ctx = ctx.state.retrieval_context.clone();
                let decoupled_enabled = ctx.config.loop_dispatch_decoupled;
                let max_retries = ctx.config.loop_dispatch_max_retries;

                handles.push(tokio::spawn(async move {
                    if decoupled_enabled {
                        execute_task_with_escalation(
                            task, agent, cancel, wave_ctx, max_tool_iters,
                            retrieval_ctx.clone(), max_retries,
                        ).await
                    } else {
                        execute_single_task(
                            task, agent, cancel, wave_ctx, max_tool_iters,
                            retrieval_ctx,
                        ).await
                    }
                }));
            }

            // Collect results from all tasks in this wave
            for handle in handles {
                match handle.await {
                    Ok(result) => {
                        if result.success {
                            tracing::info!(task_id = %result.task_id, "task completed");
                        } else {
                            tracing::warn!(task_id = %result.task_id, error = ?result.error.as_ref().map(|s| &s[..s.len().min(80)]), "task failed");
                        }
                        all_results.push(result);
                    }
                    Err(e) => {
                        let err_msg = format!("Task panicked: {e}");
                        tracing::error!(error = %err_msg, "task panicked");
                        all_results.push(TaskResult {
                            task_id: "unknown".into(),
                            success: false,
                            output: String::new(),
                            error: Some(err_msg),
                            tokens_used: 0,
                        });
                    }
                }
            }
        }

        // ── Difficulty-tiered 3-Party Review: Worker → Critic → Judge ──
        // simple: skip review entirely
        // medium: critic only (auto-pass, feedback recorded)
        // hard:   full 3-party (critic + judge)
        let flash_provider = ctx.agent.router().flash();
        let pro_provider = ctx.agent.router().pro();
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
                        flash_provider, ctx.config.loop_critic_max_tokens, cancel.child_token(),
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
                        flash_provider, ctx.config.loop_critic_max_tokens, cancel.child_token(),
                    ).await;

                    let judge_result = run_judge(
                        &result.task_id, &result.output, &critique, desc, expected,
                        pro_provider, ctx.config.loop_judge_max_tokens, cancel.child_token(),
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
            if !entry.judge_passed {
                if let Some(result) = all_results.iter_mut().find(|r| r.task_id == entry.task_id) {
                    result.success = false;
                    result.error = Some(format!("Quality check failed: {}", entry.judge_verdict));
                }
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

        // Persist all task outputs to disk under ./result/loop-pipeline/
        // Each task gets its own subdirectory: {task_id}_{short_description}/
        let result_dir = std::path::PathBuf::from("./result/loop-pipeline");
        std::fs::create_dir_all(&result_dir).ok();
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
            std::fs::create_dir_all(&task_dir).ok();

            let filename = format!("{}.md", if result.success { "ok" } else { "failed" });
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
        let summary_path = result_dir.join(format!("{}_summary.json", ts));
        if let Ok(json) = serde_json::to_string_pretty(&summary_data) {
            std::fs::write(&summary_path, &json).ok();
        }

        let success_count = all_results.iter().filter(|r| r.success).count();
        let fail_count = all_results.iter().filter(|r| !r.success).count();

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

// ── Phase 3: Decoupled Execution Helpers ───────────────────────
// These functions implement the MLEvolve-inspired tactic/strategy
// decoupling with escalation on repeated failures.

/// Execute a single task with Phase 3 decoupled execution:
/// tactic retries → strategy escalation → final tactic attempt.
async fn execute_task_with_escalation(
    task: TaskUnit,
    agent: Arc<Agent>,
    cancel: CancellationToken,
    wave_ctx: String,
    max_tool_iters: usize,
    retrieval_ctx: crate::types::RetrievalContext,
    max_retries: usize,
) -> TaskResult {
    let mut last_result = execute_single_task(
        task.clone(),
        agent.clone(),
        cancel.clone(),
        wave_ctx.clone(),
        max_tool_iters,
        retrieval_ctx.clone(),
    )
    .await;

    let mut retries = 0;
    while !last_result.success && retries < max_retries {
        retries += 1;
        tracing::warn!(
            task_id = %task.id,
            attempt = retries,
            max = max_retries,
            "Phase 3: tactic failed, retrying"
        );
        last_result = execute_single_task(
            task.clone(),
            agent.clone(),
            cancel.clone(),
            wave_ctx.clone(),
            max_tool_iters,
            retrieval_ctx.clone(),
        )
        .await;
    }

    if last_result.success {
        tracing::info!(task_id = %task.id, "Phase 3: success after {} attempts", retries + 1);
        return last_result;
    }

    // All retries exhausted — escalate to strategy replan
    tracing::warn!(task_id = %task.id, "Phase 3: escalating to strategy layer");

    let replanned_task = match strategy_replan(
        &task,
        &last_result,
        &agent,
        &retrieval_ctx,
        max_retries,
        cancel.clone(),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(task_id = %task.id, "Phase 3: strategy replan failed: {}", e);
            return TaskResult {
                task_id: task.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("Strategy replan failed: {e}")),
                tokens_used: 0,
            };
        }
    };

    // Final tactic attempt with replanned task
    tracing::info!(task_id = %task.id, "Phase 3: final tactic attempt with replanned task");
    let final_result = execute_single_task(
        replanned_task,
        agent,
        cancel,
        wave_ctx,
        max_tool_iters,
        retrieval_ctx,
    )
    .await;

    TaskResult {
        task_id: task.id.clone(),
        success: final_result.success,
        output: final_result.output,
        error: final_result.error,
        tokens_used: final_result.tokens_used,
    }
}

/// Execute a single task (tactic layer) — extracted from the original inline code.
async fn execute_single_task(
    task: TaskUnit,
    agent: Arc<Agent>,
    cancel: CancellationToken,
    wave_ctx: String,
    max_tool_iters: usize,
    retrieval_ctx: crate::types::RetrievalContext,
) -> TaskResult {
    let system = new_role_system_prompt(
        &task.assigned_role,
        &task.description,
        &task.expected_output,
    );

    let repair_context = if wave_ctx.is_empty() {
        String::new()
    } else {
        format!("\n\n## Repair Context (apply if relevant)\n{wave_ctx}")
    };

    // Inject memory retrieval context into worker prompt
    let memory_context = if retrieval_ctx.relevant_successes.is_empty()
        && retrieval_ctx.pitfalls.is_empty() {
        String::new()
    } else {
        let mut parts = Vec::new();
        if !retrieval_ctx.relevant_successes.is_empty() {
            let items: Vec<String> = retrieval_ctx.relevant_successes.iter()
                .take(2)
                .map(|s| format!("- [SUCCESS] {}", s.description))
                .collect();
            parts.push(format!("## Past Successes (apply relevant lessons)\n{}", items.join("\n")));
        }
        if !retrieval_ctx.pitfalls.is_empty() {
            let items: Vec<String> = retrieval_ctx.pitfalls.iter()
                .take(2)
                .map(|p| format!("- [PITFALL] {}", p.description))
                .collect();
            parts.push(format!("## Known Pitfalls (avoid these)\n{}", items.join("\n")));
        }
        format!("\n\n## Memory Context\n{}", parts.join("\n\n"))
    };

    let prompt = format!(
        "{repair_context}{memory_context}\n\n\
         ## Task\n{description}\n\
         ## Expected Output\n{expected}\n\n\
         {tool_instructions}\n\
         5. If you have already completed the task, summarize the findings",
        description = task.description,
        expected = task.expected_output,
        tool_instructions = tool_instruction_block(),
    );

    let mut history = vec![Message::user(&prompt)];
    let allowed: Vec<String> = tools_for_role(&task.assigned_role)
        .iter().map(|s| s.to_string()).collect();
    let mut context = RunContext::new(&system)
        .with_complexity(TaskComplexity::Moderate)
        .with_provider(ProviderChoice::Auto)
        .with_allowed_tools(allowed);
    context.max_tool_iterations = max_tool_iters;

    let result = agent.run_with_loop(&mut history, &context, cancel).await;
    match result {
        Ok(delta) => {
            let output: String = history.iter()
                .filter(|m| m.role == miniagent_core::message::MessageRole::Assistant)
                .map(|m| m.text_content())
                .collect::<Vec<_>>()
                .join("\n\n");

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
            let success = has_text && !has_clear_failure
                && (!has_tool_calls || !all_tools_errored);

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
            }
        }
        Err(e) => TaskResult {
            task_id: task.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Agent error: {e}")),
            tokens_used: 0,
        },
    }
}

/// Strategy layer re-planning: called when tactic fails repeatedly.
///
/// Uses memory retrieval context to propose an alternative approach,
/// then returns a new TaskUnit for immediate re-execution.
async fn strategy_replan(
    failed_task: &TaskUnit,
    failed_result: &TaskResult,
    agent: &Arc<Agent>,
    retrieval: &crate::types::RetrievalContext,
    max_retries: usize,
    cancel: CancellationToken,
) -> Result<TaskUnit, AgentError> {
    // Build memory section from retrieval context
    let memory_section = {
        let mut parts = Vec::new();
        if !retrieval.relevant_successes.is_empty() {
            let successes: Vec<String> = retrieval.relevant_successes.iter()
                .take(2)
                .map(|s| format!("- [SUCCESS] {}: {}", s.description, s.lessons.first().unwrap_or(&"".to_string())))
                .collect();
            parts.push(format!("## Relevant Past Successes\n{}", successes.join("\n")));
        }
        if !retrieval.pitfalls.is_empty() {
            let pitfalls: Vec<String> = retrieval.pitfalls.iter()
                .take(2)
                .map(|p| format!("- [PITFALL] {}: {}", p.description, p.lessons.first().unwrap_or(&"".to_string())))
                .collect();
            parts.push(format!("## Historical Pitfalls to Avoid\n{}", pitfalls.join("\n")));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("## Memory Retrieval\n{}", parts.join("\n\n"))
        }
    };

    let prompt = format!(
        r#"You are the **Strategy Layer** of a multi-agent pipeline. A tactic execution has failed repeatedly and needs re-planning.

## Original Task
{description}

## Expected Output
{expected}

## Failure History
{failures}

## Consecutive Failures
{count}

{memory_section}

## Your Role
You are NOT executing the task. You are restructuring the APPROACH:
1. Analyze why previous attempts failed
2. Propose a completely different approach or role assignment
3. Suggest specific tools or techniques that might work better

## Output Format (valid JSON only)
{{
  "new_description": "Revised task description with a different approach",
  "new_role": "researcher|executor|writer|critic|synthesizer|analyst",
  "new_expected_output": "What success looks like for this revised approach",
  "rationale": "Why this approach might work better"
}}"#,
        description = failed_task.description,
        expected = failed_task.expected_output,
        failures = failed_result.error.as_deref().unwrap_or("Unknown error"),
        count = max_retries,
        memory_section = memory_section,
    );

    let mut history = vec![Message::user(&prompt)];
    let mut context = RunContext::new(
        "You are a strategic planner. Analyze failures and propose alternative approaches. Always output valid JSON."
    )
    .with_complexity(TaskComplexity::Moderate)
    .with_provider(ProviderChoice::Auto);

    let _delta = agent.run_with_loop(&mut history, &context, cancel).await?;

    let response_text: String = history.iter()
        .filter(|m| m.role == miniagent_core::message::MessageRole::Assistant)
        .map(|m| m.text_content())
        .collect::<Vec<_>>()
        .join("\n\n");

    // Use the robust extract_and_repair instead of hand-rolled fence stripping
    let json_str = miniagent_core::json_util::extract_and_repair(&response_text);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| AgentError::provider(format!("Strategy parse error: {e}\nRaw: {json_str}")))?;

    let new_description = parsed["new_description"]
        .as_str()
        .unwrap_or(&failed_task.description)
        .to_string();
    let new_role = parsed["new_role"]
        .as_str()
        .unwrap_or("executor")
        .to_string();
    let new_expected = parsed["new_expected_output"]
        .as_str()
        .unwrap_or(&failed_task.expected_output)
        .to_string();

    // Echo guard: if the LLM returned the same description AND same role,
    // the "strategy" is a no-op. Refuse to disguise a retry as a replan.
    let desc_changed = new_description.trim().to_lowercase()
        != failed_task.description.trim().to_lowercase();
    let role_changed = new_role != failed_task.assigned_role;

    if !desc_changed && !role_changed {
        tracing::warn!(
            task_id = %failed_task.id,
            "Phase 3: strategy replan echoed original task - rejecting as no-op"
        );
        return Err(AgentError::provider(
            "Strategy replan produced identical task (no change in description or role)"
        ));
    }

    if !desc_changed && role_changed {
        tracing::info!(
            task_id = %failed_task.id,
            old_role = %failed_task.assigned_role,
            new_role = %new_role,
            "Phase 3: role-only replan (description unchanged)"
        );
    }

    tracing::info!(
        task_id = %failed_task.id,
        new_role = %new_role,
        "Phase 3: strategy replan complete"
    );

    Ok(TaskUnit {
        id: failed_task.id.clone(),
        description: new_description,
        assigned_role: new_role,
        depends_on: Vec::new(),
        expected_output: new_expected,
        difficulty: "hard".into(),
        failed: false,
        error: None,
        output: None,
    })
}
    use super::*;
// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}
