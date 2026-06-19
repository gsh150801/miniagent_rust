use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::CompletionRequest;
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{EvaluationResult, StageMessage};

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

        let total = plan.tasks.len();
        let completed = results.iter().filter(|r| r.success).count();
        let failed = results.iter().filter(|r| !r.success).count();
        let pending = total - completed - failed;

        let result_summary: String = results.iter()
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

        let failed_ids_json: String = if failed > 0 {
            results.iter()
                .filter(|r| !r.success)
                .map(|r| format!("\"{}\"", r.task_id))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            String::new()
        };

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

        let provider = ctx.agent.router().flash();
        let request = CompletionRequest {
            system: "You are a thorough evaluator. Assess task completion objectively. Output ONLY valid JSON.".into(),
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
                    failed_task_ids: results.iter().filter(|r| !r.success).map(|r| r.task_id.clone()).collect(),
                    unmet_goals: vec!["Evaluation parse failed, defaulting to continue".into()],
                    should_continue: failed > 0 || loop_count == 0,
                    summary: format!("{}/{} tasks completed. {} failed.", completed, total, failed),
                }
            }
        };

        // Override should_continue based on loop limits and failures
        if loop_count >= max_loops {
            evaluation.should_continue = false;
        } else if failed == 0 && completed == total && evaluation.overall_progress_pct >= 90.0 {
            evaluation.should_continue = false;
        } else if failed == 0 && completed == total {
            evaluation.should_continue = false;
        } else if loop_count == 0 && completed == 0 {
            evaluation.should_continue = true;
        }

        let mut state = ctx.state.clone();
        state.evaluations.push(evaluation.clone());
        state.completed = !evaluation.should_continue;

        if !evaluation.should_continue {
            state.final_output = Some(evaluation.summary.clone());
            // Collect successful outputs as final output
            let outputs: Vec<String> = results.iter()
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
