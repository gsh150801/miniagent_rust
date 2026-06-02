use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Observer (观察者) — event logging, context compression, state snapshots.
///
/// Inspired by Manus's context engineering principles:
/// - Compresses context by replacing long content with file references
/// - Preserves errors (never hide failures)
/// - Generates incremental summaries that survive context window limits
/// - Maintains the event log for cross-agent awareness
pub struct ObserverRole {
    provider: Box<dyn LlmProvider>,
}

impl ObserverRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for ObserverRole {
    fn name(&self) -> &str { "observer" }
    fn description(&self) -> &str {
        "Context observer. Compresses context, maintains event log, generates state snapshots. \
         Ensures no information is lost during long-running tasks. Persists to observer/ directory."
    }

    async fn execute(
        &self, _task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        // Collect all current state from filesystem
        let event_log = load_checkpoint(&blackboard.work_dir, "", "events.log")
            .unwrap_or_default();
        let todo = load_todo(&blackboard.work_dir);

        // Gather all role artifacts
        let roles = ["supervisor", "planner", "researcher", "critic",
                     "synthesizer", "executor", "writer", "reviewer", "evaluator"];
        let mut artifact_summary = Vec::new();
        for role in &roles {
            let dir = blackboard.work_dir.join(role);
            if dir.exists()
                && let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        artifact_summary.push(format!("{role}/{name} ({} bytes)", size));
                    }
                }
        }

        let prompt = format!(
            r#"You are the **Observer** in a multi-agent system.

## Event Log (last 2000 chars)
{}

## Current Todo
{}

## Current Artifacts
{}

## Your Role
1. Generate a compressed context summary
2. Identify what has been accomplished so far
3. Note any errors or failures that occurred (PRESERVE them)
4. Flag any missing information or stalled progress
5. Produce a state snapshot that survives context compression

## Compression Rules (from Manus best practices)
- Replace long content with "See: role/file.json (N bytes)"
- Keep error messages — they are learning opportunities
- Preserve all file paths — compression only drops content, not references
- Keep the most recent events in full detail
- Summarize older events into a progress timeline

## Output Format (JSON)
{{
  "context_summary": "compressed summary of current state",
  "accomplished": ["task 1 done", "task 2 done"],
  "errors_preserved": ["error that should not be forgotten"],
  "files_summary": {{
    "role/file.json": "brief description of contents"
  }},
  "progress_pct": 0-100,
  "bottlenecks": ["what's slowing us down"],
  "timeline": "compressed event timeline"
}}
"#,
            &event_log[event_log.len().saturating_sub(2000)..],
            todo,
            artifact_summary.join("\n"),
        );

        let system = "You are a context compression specialist. Preserve ALL file paths and errors. \
                       Replace long content with file references. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "observer: generating context snapshot");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let parsed = self.parse_response(&response);

        // Persist context snapshot
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "context_summary": parsed.content,
            "accomplished": parsed.metadata.get("accomplished"),
            "errors_preserved": parsed.metadata.get("errors_preserved"),
            "progress_pct": parsed.metadata.get("progress_pct"),
            "files_summary": parsed.metadata.get("files_summary"),
        })).unwrap_or_default();
        persist_output(&blackboard.work_dir, "observer", "context.json", &json);
        persist_output(&blackboard.work_dir, "observer", "context.md", &parsed.content);

        append_event(&blackboard.work_dir, "observer: context snapshot persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for ObserverRole {
    fn workspace_name(&self) -> &str { "observer" }
}

impl ObserverRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.1),
                max_tokens: Some(2000),
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

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("accomplished".into(),
            serde_json::to_string(&parsed["accomplished"]).unwrap_or_default());
        metadata.insert("errors_preserved".into(),
            serde_json::to_string(&parsed["errors_preserved"]).unwrap_or_default());
        metadata.insert("files_summary".into(),
            serde_json::to_string(&parsed["files_summary"]).unwrap_or_default());
        if let Some(pct) = parsed["progress_pct"].as_u64() {
            metadata.insert("progress_pct".into(), pct.to_string());
        }
        if let Some(arr) = parsed["bottlenecks"].as_array() {
            metadata.insert("bottlenecks".into(),
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "));
        }

        RoleOutput {
            content: parsed["context_summary"].as_str().unwrap_or("").to_string(),
            evidence: vec![],
            confidence: 0.9,
            metadata,
            output_files: vec!["observer/context.json".into(), "observer/context.md".into()],
            status: "success".into(),
        }
    }
}
