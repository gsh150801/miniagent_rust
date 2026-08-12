use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::CompletionRequest;
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{TaskResult, RepairAnalysis, StageMessage};

/// Repair Stage: analyzes failed tasks, determines root causes, and
/// pushes insights back to explore, plan, and dispatch stages.
pub struct RepairStage;

#[async_trait]
impl PipelineStage for RepairStage {
    fn name(&self) -> &str { "repair" }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let failed_results: Vec<&TaskResult> = ctx.state.task_results.iter()
            .filter(|r| !r.success)
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

        for result in &failed_results {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            let task_detail = ctx.state.plan.as_ref()
                .and_then(|p| p.tasks.iter().find(|t| t.id == result.task_id))
                .map(|t| format!(
                    "Role: {}, Description: {}, Expected: {}",
                    t.assigned_role, t.description, t.expected_output,
                ))
                .unwrap_or_default();

            let prompt = format!(
                r#"You are the **Repair Analyst** in a multi-agent pipeline.
Analyze the failed task below and determine root cause and fix.

## Failed Task
Task ID: {task_id}
{task_detail}

## Error Output
{error_output}

## Root Cause Categories
- **tool_error**: The tool failed or returned unexpected results (retry with different parameters)
- **model_error**: The LLM failed to follow instructions (adjust prompt/role assignment)
- **dependency_error**: A dependency task failed (re-plan dependencies)
- **ambiguity_error**: Task description was unclear (requires re-exploration)
- **resource_error**: Missing files, APIs, or permissions (fix environment)
- **timeout_error**: Task exceeded time limit (split into smaller tasks)

## Instructions
1. Classify the root cause category
2. Be specific about what went wrong
3. Suggest concrete fixes, not vague advice
4. If the task description was unclear, mark requires_re_explore = true
5. If dependencies or role assignment was wrong, mark requires_re_plan = true

## Output Format (valid JSON only)
{{
  "failed_task_id": "{task_id}",
  "root_cause": "Specific root cause analysis",
  "suggested_fix": "Concrete steps to fix this",
  "requires_re_explore": true|false,
  "requires_re_plan": true|false,
  "suggested_new_approach": "Alternative approach if needed"
}}"#,
                task_id = result.task_id,
                task_detail = task_detail,
                error_output = result.error.as_deref().unwrap_or("No error details"),
            );

            let provider = ctx.agent.router().pro();
            let request = CompletionRequest {
                system: format!("You are an expert failure analyst. {} Diagnose issues and suggest fixes. Output ONLY valid JSON.", miniagent_core::context_info::date_hint()),
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
                        .unwrap_or_else(|_| RepairAnalysis {
                            failed_task_id: result.task_id.clone(),
                            root_cause: "Unknown failure".into(),
                            suggested_fix: "Retry the task".into(),
                            requires_re_explore: false,
                            requires_re_plan: false,
                            suggested_new_approach: None,
                        })
                }
                Err(e) => RepairAnalysis {
                    failed_task_id: result.task_id.clone(),
                    root_cause: format!("LLM analysis failed: {e}"),
                    suggested_fix: "Retry the task".into(),
                    requires_re_explore: false,
                    requires_re_plan: false,
                    suggested_new_approach: None,
                },
            };

            tracing::info!(task_id = %analysis.failed_task_id, root_cause = %analysis.root_cause.chars().take(80).collect::<String>(), "Repair analysis");

            // Route repair insights to the relevant stages
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

            // Always send to dispatch with the fix suggestion
            messages.push(StageMessage {
                from_stage: "repair".into(),
                to_stage: "dispatch".into(),
                content: format!(
                    "Repair insight for '{}': root cause: {}. Suggested fix: {}",
                    analysis.failed_task_id,
                    analysis.root_cause,
                    analysis.suggested_fix,
                ),
                task_id: Some(analysis.failed_task_id.clone()),
            });

            all_analyses.push(analysis);
        }

        let mut state = ctx.state.clone();
        state.repair_analyses.extend(all_analyses);

        // Route to explore stage if any analysis requires it
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
                "Analyzed {} failed tasks. Re-explore: {}, re-plan: {}.",
                failed_results.len(),
                if has_re_explore { "yes" } else { "no" },
                if has_re_plan { "yes" } else { "no" },
            ),
        })
    }
}
