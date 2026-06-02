use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            persist_output, load_checkpoint, load_todo, append_event, parse_llm_json};

/// Writer (写作者) — report writing, document formatting.
///
/// The Writer produces final formatted output from all collected
/// research, synthesis, and evaluation data. It reads everything
/// from the filesystem and produces polished documents.
pub struct WriterRole {
    provider: Box<dyn LlmProvider>,
}

impl WriterRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for WriterRole {
    fn name(&self) -> &str { "writer" }
    fn description(&self) -> &str {
        "Report writer. Produces formatted documents from research findings, \
         synthesis, and evaluation. Reads all prior outputs from filesystem. \
         Persists drafts and final reports to writer/ directory."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let findings = load_checkpoint(&blackboard.work_dir, "researcher", "findings.md")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "researcher", "findings.json"))
            .unwrap_or_default();
        let critique = load_checkpoint(&blackboard.work_dir, "critic", "critique.md")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "critic", "critique.json"))
            .unwrap_or_default();
        let synthesis = load_checkpoint(&blackboard.work_dir, "synthesizer", "synthesis.md")
            .or_else(|| load_checkpoint(&blackboard.work_dir, "synthesizer", "synthesis.json"))
            .unwrap_or_default();
        let review = load_checkpoint(&blackboard.work_dir, "reviewer", "review.json");
        let prior_draft = load_checkpoint(&blackboard.work_dir, "writer", "draft.md");

        let review_section = review.map(|r| format!("\n## Reviewer Feedback\n{r}")).unwrap_or_default();
        let draft_section = prior_draft.as_ref().map(|d| {
            format!("\n## Previous Draft (revise based on feedback)\n{d}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Writer** in a multi-agent system.

**Task:** {task}

## Todo
{todo}

## Research Findings
{findings}

## Critic's Evaluation
{critique}

## Synthesis
{synthesis}
{review_section}{draft_section}

## Your Role
Write a comprehensive, well-structured report that:
1. Opens with a clear executive summary
2. Presents findings with proper citations
3. Discusses evidence quality and limitations
4. Draws conclusions supported by the synthesis
5. Suggests future research directions
6. Uses the same language as the task description

## Output Format
Write the full report in Markdown format.
Include proper headers, bullet points, and citation references.

Output JSON:
{{
  "title": "report title",
  "executive_summary": "2-3 sentence summary",
  "report_markdown": "full markdown report",
  "references": ["PMID:xxx", "DOI:xxx"],
  "word_count": 1234
}}
"#
        );

        let system = "You are a scientific writer. Produce clear, well-structured reports \
                       with proper citations. Write in the same language as the task. \
                       Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "writer: composing report");

        // Check budget before LLM call
        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = self.call_llm(&system, &prompt, cancel).await?;
        let raw_parsed = match parse_llm_json(&response) {
            Ok(v) => v,
            Err(e) => return Ok(RoleOutput::failed(self.name(), &e)),
        };

        // Persist draft from raw LLM output (not from metadata — fixes memory doubling)
        if let Some(md) = raw_parsed["report_markdown"].as_str() {
            persist_output(&blackboard.work_dir, "writer", "draft.md", md);
        }

        let parsed = self.parse_response(&response);
        persist_output(&blackboard.work_dir, "writer", "report.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "title": parsed.metadata.get("title"),
                "executive_summary": parsed.content,
                "word_count": parsed.metadata.get("word_count"),
            })).unwrap_or_default());

        append_event(&blackboard.work_dir, "writer: report persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for WriterRole {
    fn workspace_name(&self) -> &str { "writer" }
}

impl WriterRole {
    async fn call_llm(&self, system: &str, prompt: &str, cancel: CancellationToken) -> Result<String, AgentError> {
        let request = CompletionRequest {
            system: system.to_string(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.3),
                max_tokens: Some(8000),
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
        if let Some(title) = parsed["title"].as_str() {
            metadata.insert("title".into(), title.into());
        }
        // Store file reference instead of full report text (fixes memory doubling)
        if parsed["report_markdown"].is_string() {
            metadata.insert("report_markdown_file".into(), "writer/draft.md".into());
        }
        if let Some(wc) = parsed["word_count"].as_u64() {
            metadata.insert("word_count".into(), wc.to_string());
        }
        if let Some(refs) = parsed["references"].as_array() {
            metadata.insert("references".into(),
                refs.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "));
        }

        RoleOutput {
            content: parsed["executive_summary"].as_str().unwrap_or("").to_string(),
            evidence: vec![],
            confidence: 0.85,
            metadata,
            output_files: vec!["writer/draft.md".into(), "writer/report.json".into()],
            status: "success".into(),
        }
    }
}
