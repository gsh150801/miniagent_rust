use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Supervisor (监督者) — task decomposition, delegation, and progress tracking.
///
/// Inspired by Anthropic's Orchestrator-Workers pattern:
/// the supervisor decomposes complex tasks into structured delegations,
/// monitors progress, and synthesizes results.
///
/// Key principle: outputs structured JSON delegations instead of
/// fragile string parsing (fixes `AGENT: xxx | TASK: yyy` bug).
pub struct SupervisorRole {
    provider: Box<dyn LlmProvider>,
}

impl SupervisorRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for SupervisorRole {
    fn name(&self) -> &str { "supervisor" }
    fn description(&self) -> &str {
        "Task supervisor. Decomposes complex tasks into structured delegations, \
         monitors agent progress, and synthesizes final results. \
         All plans and progress are persisted to filesystem."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        // Load context: todo, prior plan, recent events
        let todo = load_todo(&blackboard.work_dir);
        let prior_plan = load_checkpoint(&blackboard.work_dir, "supervisor", "plan.json");
        let progress = load_checkpoint(&blackboard.work_dir, "supervisor", "progress.md");

        let context_section = if prior_plan.is_some() || !todo.is_empty() {
            format!(
                "\n## Current Context\n{}\n{}",
                if todo.is_empty() { String::new() } else { format!("### Todo\n{todo}") },
                prior_plan.map(|p| format!("### Prior Plan\n{p}")).unwrap_or_default(),
            )
        } else {
            String::new()
        };

        let prompt = format!(
            r#"You are the **Supervisor** of a multi-agent research system.

**User Task:** {task}{context_section}
{progress_section}

## Available Agents
- **planner**: Creates and refines execution plans
- **researcher**: Searches literature, extracts facts with citations
- **executor**: Runs tools, executes code, manipulates files
- **critic**: Evaluates quality, identifies flaws
- **synthesizer**: Integrates multiple sources into unified conclusions
- **writer**: Produces formatted reports and documents
- **reviewer**: Final quality check against standards
- **evaluator**: Scores results, suggests iterations

## Your Role
1. Decompose the task into concrete subtasks
2. Assign each subtask to the best-fit agent
3. Define dependencies between subtasks
4. Specify input files and expected outputs
5. Set priority ordering

## Output Format (JSON)
{{
  "plan_id": "unique-id",
  "overall_goal": "what we're trying to achieve",
  "steps": [
    {{
      "id": "step-1",
      "agent": "agent-name",
      "description": "what this agent should do",
      "dependencies": [],
      "input_files": ["path/to/input"],
      "expected_output": "description of expected result",
      "priority": 1,
      "parallel_with": ["step-2"]
    }}
  ],
  "success_criteria": "how to judge if the overall task succeeded",
  "estimated_iterations": 3
}}
"#,
            progress_section = progress.map(|p| format!("\n### Progress\n{p}")).unwrap_or_default(),
        );

        let system = "You are a task decomposition expert. Create structured, dependency-aware plans. \
                       Output only valid JSON. Each step must specify its agent, dependencies, and expected output."
            .to_string();

        append_event(&blackboard.work_dir, "supervisor: starting task decomposition");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist plan to filesystem
        let plan_json = serde_json::to_string_pretty(&serde_json::json!({
            "plan_id": parsed.metadata.get("plan_id").unwrap_or(&"unknown".into()),
            "steps": parsed.metadata.get("steps"),
            "overall_goal": parsed.content.clone(),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "supervisor", "plan.json", &plan_json);

        // Update todo.md (attention anchor)
        let todo_content = format!(
            "# Current Objectives\n\n## Goal\n{}\n\n## Steps\n{}",
            parsed.content,
            parsed.metadata.get("steps_summary").cloned().unwrap_or_default(),
        );
        persist_output(&blackboard.work_dir, "", "todo.md", &todo_content);

        append_event(&blackboard.work_dir, "supervisor: plan created and persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for SupervisorRole {
    fn workspace_name(&self) -> &str { "supervisor" }
}

impl SupervisorRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.1),
                max_tokens: Some(4000),
                ..Default::default()
            },
        };
        let resp = self.provider.complete(&request, cancel).await?;
        Ok(resp.content.iter()
            .filter_map(|b| match b { miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()), _ => None })
            .collect::<Vec<_>>().join(""))
    }

    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let overall_goal = parsed["overall_goal"].as_str().unwrap_or("").to_string();
        let _steps_count = parsed["steps"].as_array().map(|a| a.len()).unwrap_or(0);

        let steps_summary = parsed["steps"].as_array().map(|arr| {
            arr.iter().enumerate().map(|(i, s)| {
                format!("{}. [{}] {} (priority: {})",
                    i + 1,
                    s["agent"].as_str().unwrap_or("?"),
                    s["description"].as_str().unwrap_or(""),
                    s["priority"].as_u64().unwrap_or(0),
                )
            }).collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        if let Some(id) = parsed["plan_id"].as_str() {
            metadata.insert("plan_id".into(), id.into());
        }
        metadata.insert("steps".into(), serde_json::to_string(&parsed["steps"]).unwrap_or_default());
        metadata.insert("steps_summary".into(), steps_summary);
        if let Some(criteria) = parsed["success_criteria"].as_str() {
            metadata.insert("success_criteria".into(), criteria.into());
        }

        RoleOutput {
            content: overall_goal,
            evidence: vec![],
            confidence: 0.85,
            metadata,
            output_files: vec!["supervisor/plan.json".into()],
            status: "success".into(),
        }
    }
}
