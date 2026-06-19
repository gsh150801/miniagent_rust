use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{TaskPlan, TaskUnit, StageMessage};
use miniagent_evolution::SelectionEngine;

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
                let task_summaries: Vec<String> = p.tasks.iter()
                    .map(|t| format!("  - {} (role: {}, deps: {:?}, status: {})",
                        t.description, t.assigned_role, t.depends_on,
                        if t.failed { "failed" } else if t.output.is_some() { "done" } else { "pending" }
                    ))
                    .collect();
                format!("## Previous Plan\n{}\n", task_summaries.join("\n"))
            })
            .unwrap_or_default();

        let needs_decomposition = ctx.state.exploration_history.last()
            .map(|e| e.needs_decomposition)
            .unwrap_or(false);

        // ── MLEvolve Phase 1: Memory Success Patterns ───────────
        let memory_section = {
            let rc = &ctx.state.retrieval_context;
            if rc.relevant_successes.is_empty() {
                String::new()
            } else {
                let successes: Vec<String> = rc.relevant_successes.iter()
                    .map(|s| format!(
                        "- [PAST SUCCESS] {} (confidence={:.2})\n  Lessons: {}",
                        s.description,
                        s.confidence,
                        s.lessons.join("; ")
                    ))
                    .collect();
                format!("\n## MLEvolve: Successful Patterns from Similar Tasks\n{}", successes.join("\n"))
            }
        };
        // ────────────────────────────────────────────────────────

        let prompt = build_plan_prompt(task, loop_count, &repair_suggestions, &prior_tasks, needs_decomposition, &memory_section);

        let provider = ctx.agent.router().flash();

        // Attempt plan generation with retry: if the LLM returns only 1 task
        // when decomposition is needed, retry once with a stronger emphasis.
        let mut plan = match try_generate_plan(provider, &prompt, ctx.config.loop_plan_max_tokens, cancel.clone()).await {
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
            if let Ok(retry_plan) = try_generate_plan(provider, &retry_prompt, ctx.config.loop_plan_max_tokens, cancel.clone()).await {
                if retry_plan.tasks.len() > 1 {
                    tracing::info!(tasks = retry_plan.tasks.len(), "Retry succeeded: decomposed into multiple tasks");
                    plan = retry_plan;
                }
            }
        }

        tracing::info!("Plan: {} tasks", plan.tasks.len());
        for (i, t) in plan.tasks.iter().enumerate() {
            tracing::debug!(index = i + 1, role = %t.assigned_role, deps = ?t.depends_on, desc = %t.description, "task");
        }

        let mut state = ctx.state.clone();

        // ── MLEvolve Phase 2: Tournament Selection ─────────────────
        // If enabled, generate candidate variants and select the best.
        // The SelectionEngine persists on StageContext (behind Mutex) so
        // Elo ratings accumulate across loops.
        let plan = if ctx.config.loop_evolution_enabled {
            let experiences: Vec<_> = ctx.state.retrieval_context.relevant_successes.iter()
                .map(|s| miniagent_evolution::ExperienceSummary {
                    description: s.description.clone(),
                    lessons: s.lessons.clone(),
                    node_type: s.node_type.clone(),
                    confidence: s.confidence,
                })
                .collect();

            // Acquire the persistent engine via Mutex
            let mut guard = ctx.selection_engine.lock().expect("SelectionEngine mutex poisoned");
            if guard.is_none() {
                *guard = Some(SelectionEngine::default().with_experiences(experiences.clone()));
            }
            let engine = guard.as_mut().unwrap();

            // Refresh experience pool every loop so InjectFromExperience
            // sees the latest retrieval results (not frozen at loop 0)
            engine.experience_pool = experiences;

            tracing::info!(
                "Tournament selection: {} tasks, population={}, elo_entries={}",
                plan.tasks.len(),
                engine.population_size,
                engine.elo_ratings.len()
            );

            engine.select(&plan)
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
    memory_section: &str,
) -> String {
    let decomp_hint = if needs_decomposition {
        "IMPORTANT: The explorer determined this task MUST be decomposed into multiple parallel sub-tasks."
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
{memory_section}
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
        memory_section = memory_section,
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
        system: "You are an expert task planner. ALWAYS decompose into multiple sub-tasks. NEVER output just 1 task unless the task is truly atomic like 'write hello world'. Output ONLY valid JSON.".into(),
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
