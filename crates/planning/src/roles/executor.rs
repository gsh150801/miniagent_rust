use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Executor (执行者) — tool invocation, code execution, file operations.
///
/// The Executor is the primary "hands" of the system. It carries out
/// concrete actions: running code, calling APIs, manipulating files.
///
/// Key design: all execution results are persisted to filesystem.
/// If a tool call produces large output, only a summary stays in context;
/// the full output is in executor/output.json.
pub struct ExecutorRole {
    provider: Box<dyn LlmProvider>,
}

impl ExecutorRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for ExecutorRole {
    fn name(&self) -> &str { "executor" }
    fn description(&self) -> &str {
        "Task executor. Runs tools, executes code, manipulates files. \
         All execution results persisted to executor/ directory. \
         Large outputs are truncated in context with full version on disk."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let plan = load_checkpoint(&blackboard.work_dir, "planner", "current_plan.json");
        let prior_output = load_checkpoint(&blackboard.work_dir, "executor", "output.json");

        let continuation = prior_output.as_ref().map(|p| {
            format!("\n## Previous Execution Results\n{p}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Executor** in a multi-agent system.

**Task:** {task}

## Todo
{todo}
{plan_section}{continuation}

## Your Role
You are the "hands" of the system. You:
1. Execute concrete actions (tool calls, code, file operations)
2. Capture ALL output — never truncate or summarize away data
3. Record which tools were used and their results
4. Report errors honestly — do NOT hide failures
5. Persist intermediate results for other agents

## Important Rules
- If a tool call fails, record the FULL error message
- If output is large (>2KB), write it to a file and reference the path
- Always specify what files were created/modified
- Preserve exact command outputs (stdout, stderr, exit codes)

## Output Format (JSON)
{{
  "actions_taken": [
    {{
      "tool": "tool_name",
      "input": "what was given to the tool",
      "output_summary": "brief summary",
      "output_file": "executor/output_toolname.txt (if large)",
      "success": true/false,
      "error": "error message if failed"
    }}
  ],
  "files_created": ["path/to/file1", "path/to/file2"],
  "files_modified": ["path/to/file"],
  "summary": "what was accomplished",
  "errors": ["any errors encountered"],
  "next_steps_hint": "what should happen next based on results"
}}
"#,
            plan_section = plan.map(|p| format!("\n## Plan\n{p}")).unwrap_or_default(),
        );

        let system = "You are a task executor. Run tools and code, capture ALL output. \
                       Never hide errors. Record exact results. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "executor: starting task execution");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist execution results
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "actions": parsed.metadata.get("actions_taken"),
            "files_created": parsed.metadata.get("files_created"),
            "summary": parsed.content,
            "errors": parsed.metadata.get("errors"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "executor", "output.json", &json);

        let md = format!(
            "# Execution Report\n\n## Summary\n{}\n\n## Files Created\n{}\n\n## Errors\n{}",
            parsed.content,
            parsed.metadata.get("files_created").cloned().unwrap_or_default(),
            parsed.metadata.get("errors").cloned().unwrap_or("None".into()),
        );
        persist_output(&blackboard.work_dir, "executor", "report.md", &md);

        append_event(&blackboard.work_dir,
            &format!("executor: task completed, {} files created",
                parsed.output_files.len()));

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for ExecutorRole {
    fn workspace_name(&self) -> &str { "executor" }
}

impl ExecutorRole {
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

        let files_created: Vec<String> = parsed["files_created"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let errors: Vec<String> = parsed["errors"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("actions_taken".into(), serde_json::to_string(&parsed["actions_taken"]).unwrap_or_default());
        metadata.insert("files_created".into(), serde_json::to_string(&parsed["files_created"]).unwrap_or_default());
        metadata.insert("files_modified".into(), serde_json::to_string(&parsed["files_modified"]).unwrap_or_default());
        metadata.insert("errors".into(), serde_json::to_string(&parsed["errors"]).unwrap_or_default());
        if let Some(hint) = parsed["next_steps_hint"].as_str() {
            metadata.insert("next_steps_hint".into(), hint.into());
        }

        let success = errors.is_empty();
        let summary = parsed["summary"].as_str().unwrap_or("").to_string();

        RoleOutput {
            content: summary,
            evidence: vec![],
            confidence: if success { 0.85 } else { 0.4 },
            metadata,
            output_files: {
                let mut files = vec!["executor/output.json".into(), "executor/report.md".into()];
                files.extend(files_created.iter().cloned());
                files
            },
            status: if success { "success".into() } else { "partial".into() },
        }
    }
}
