use async_trait::async_trait;
use miniagent_agent::context::RunContext;
use miniagent_core::config::TaskComplexity;
use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use tokio_util::sync::CancellationToken;

use crate::stage::{PipelineStage, StageContext, StageOutput};
use crate::types::{ExplorationResult, StageMessage};
use crate::prompts::{role_system_prompt, tool_instruction_block, tools_for_role};

/// Explore Stage: uses tools to research the task, gathers real information,
/// and produces a refined understanding with findings.
pub struct ExploreStage;

#[async_trait]
impl PipelineStage for ExploreStage {
    fn name(&self) -> &str { "explore" }

    async fn execute(
        &self,
        ctx: &StageContext,
        cancel: CancellationToken,
    ) -> Result<StageOutput, AgentError> {
        let task = if ctx.state.current_task.is_empty() {
            &ctx.state.original_task
        } else {
            &ctx.state.current_task
        };

        let repair_context: String = ctx.state.repair_analyses.iter()
            .filter(|r| r.requires_re_explore)
            .map(|r| format!(
                "- Failed task '{}': root cause: {}. Suggested new approach: {}",
                r.failed_task_id, r.root_cause,
                r.suggested_new_approach.as_deref().unwrap_or("none")
            ))
            .collect::<Vec<_>>()
            .join("\n");

        let prior_exploration: String = if ctx.state.exploration_history.is_empty() {
            String::new()
        } else {
            let all_findings: Vec<String> = ctx.state.exploration_history.iter()
                .enumerate()
                .flat_map(|(i, e)| {
                    let mut items = vec![format!("--- Loop {i} ---")];
                    items.extend(e.findings.iter().map(|f| format!("  - {f}")));
                    items
                })
                .collect();
            format!("## Prior Exploration History ({count} loops)\n{findings}",
                count = ctx.state.exploration_history.len(),
                findings = all_findings.join("\n"),
            )
        };

        // ── MLEvolve Phase 1: Memory Retrieval Context ───────────
        let retrieval_context = &ctx.state.retrieval_context;
        let memory_section = format_memory_section(retrieval_context);
        // ────────────────────────────────────────────────────────

        let prompt = format!(
            r#"You are the **Explorer** in a multi-agent pipeline. Your job is to use tools to research the task and gather real information.

## Original Task
{task}

## Context from Previous Loop
{prior_exploration}
{repair_context}

{memory_section}
## Instructions
1. **Use web_search, web_fetch, and pubmed_search** to research the task.
2. Gather real information — do not rely on your internal knowledge alone.
3. Based on your research, clarify the task requirements.
4. Estimate the complexity (simple / moderate / complex / very complex).
5. Suggest whether the task can be decomposed into parallel sub-tasks.
6. Record key findings from your research.
7. If this is not the first loop, focus on what is still incomplete or needs improvement.

## Output Format (valid JSON only) — place this at the END of your response:
{{
  "clarified_task": "A refined, clear description of the task based on research",
  "findings": ["Key finding 1 from web_search/pubmed", "Key finding 2", ...],
  "estimated_complexity": "simple|moderate|complex|very_complex",
  "needs_decomposition": true|false
}}"#
        );

        // Reuse the shared Agent from context — no rebuilding
        let agent = &ctx.agent;

        let system_prompt = role_system_prompt("explorer", task, "Clarified task understanding with research findings");
        let user_prompt = format!("{}\n\n{}", prompt, tool_instruction_block());

        let mut history = vec![Message::user(&user_prompt)];
        let allowed: Vec<String> = tools_for_role("explorer")
            .iter().map(|s| s.to_string()).collect();
        let mut context = RunContext::new(&system_prompt)
            .with_complexity(TaskComplexity::Moderate)
            .with_allowed_tools(allowed);
        context.max_tool_iterations = ctx.config.loop_explore_max_iterations;
        context.max_tokens = Some(ctx.config.loop_explore_max_tokens);

        // Run the Explorer agent; fallback on error
        let combined = match agent.run_with_loop(&mut history, &context, cancel).await {
            Ok(delta) => {
                let response_text = history.iter()
                    .rev()
                    .find(|m| m.role == miniagent_core::message::MessageRole::Assistant)
                    .map(|m| m.text_content())
                    .unwrap_or_default();

                if delta.new_messages.is_empty() {
                    response_text
                } else {
                    let extra: String = delta.new_messages.iter()
                        .map(|m| m.text_content())
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("{}\n{}", response_text, extra)
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Explorer agent error, using fallback");
                let findings_text: String = ctx.state.repair_analyses.iter()
                    .map(|r| format!("Insight from previous: {}", r.root_cause))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(
                    r#"{{"clarified_task": "{}", "findings": ["Fallback: {e}{insights}"], "estimated_complexity": "moderate", "needs_decomposition": true}}"#,
                    task,
                    insights = if findings_text.is_empty() { String::new() } else { format!(". {}", findings_text) },
                )
            }
        };

        // Parse last JSON block from the response
        let json = miniagent_core::json_util::extract_and_repair(&combined);

        let exploration: ExplorationResult = serde_json::from_str(&json)
            .unwrap_or_else(|_| ExplorationResult {
                clarified_task: task.to_string(),
                findings: vec!["Exploration completed via tool research".into()],
                estimated_complexity: "moderate".into(),
                needs_decomposition: true,
            });

        let mut state = ctx.state.clone();
        state.current_task = exploration.clarified_task.clone();
        state.exploration_history.push(exploration.clone());

        let msg = StageMessage {
            from_stage: "explore".into(),
            to_stage: "plan".into(),
            content: serde_json::to_string(&exploration).unwrap_or_default(),
            task_id: None,
        };

        Ok(StageOutput {
            updated_state: state,
            new_messages: vec![msg],
            summary: format!(
                "Explored task. Complexity: {}, findings: {}, decomposition needed: {}",
                exploration.estimated_complexity,
                exploration.findings.len(),
                exploration.needs_decomposition,
            ),
        })
    }
}

// ── MLEvolve Phase 1: Memory Injection Helpers ─────────────────

/// Format the memory retrieval context into a prompt section.
///
/// Injects:
///   - Relevant successes (what worked for similar tasks)
///   - Pitfalls (what to avoid based on historical failures)
///   - Domain template suggestions (tools, roles, focus)
fn format_memory_section(ctx: &crate::types::RetrievalContext) -> String {
    let mut parts = Vec::new();

    // Relevant successes
    if !ctx.relevant_successes.is_empty() {
        let successes: Vec<String> = ctx.relevant_successes.iter()
            .map(|s| format!("- [SUCCESS] {}: {}", s.description, s.lessons.first().unwrap_or(&"".to_string())))
            .collect();
        parts.push(format!("## Relevant Past Successes\n{}", successes.join("\n")));
    }

    // Pitfalls to avoid
    if !ctx.pitfalls.is_empty() {
        let pitfalls: Vec<String> = ctx.pitfalls.iter()
            .map(|p| format!("- [PITFALL] {}: {}", p.description, p.lessons.first().unwrap_or(&"".to_string())))
            .collect();
        parts.push(format!("## Historical Pitfalls to Avoid\n{}", pitfalls.join("\n")));
    }

    // Confidence indicator
    if ctx.confidence > 0.0 {
        parts.push(format!("## Memory Confidence\n{:.0}% — based on {} relevant experiences",
            ctx.confidence * 100.0,
            ctx.relevant_successes.len() + ctx.pitfalls.len(),
        ));
    }

    if parts.is_empty() {
        return String::new();
    }

    format!("## MLEvolve Memory Retrieval\n{}", parts.join("\n\n"))
}

