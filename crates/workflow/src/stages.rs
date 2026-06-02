use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::stage::{StageContext, StageError, StageHandler, StageMetadata, StageOutput};

// ── File-based stage communication ───────────────────────────

/// Legacy default (used when task_dir is not provided).
pub const WORKFLOW_DIR: &str = "./result/.workflow";

/// Resolve the task-specific workflow directory.
/// Reads `task_dir` from the input JSON; falls back to WORKFLOW_DIR.
fn task_workflow_dir(ctx: &StageContext) -> std::path::PathBuf {
    let base = ctx.input["task_dir"].as_str().unwrap_or(WORKFLOW_DIR);
    let dir = std::path::PathBuf::from(base);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn ensure_workflow_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(WORKFLOW_DIR);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Save stage output to a JSON file in the task-specific workflow directory.
fn save_stage_output(ctx: &StageContext, filename: &str, data: &serde_json::Value) {
    let dir = task_workflow_dir(ctx);
    let path = dir.join(filename);
    if let Ok(json) = serde_json::to_string_pretty(data) {
        let _ = std::fs::write(&path, json);
    }
}

/// Read a stage's output from the task-specific workflow directory.
fn load_stage_output(ctx: &StageContext, filename: &str) -> Option<serde_json::Value> {
    let base = ctx.input["task_dir"].as_str().unwrap_or(WORKFLOW_DIR);
    let path = std::path::PathBuf::from(base).join(filename);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

// ── Agent Stage: executes via Agent::run_with_loop ────────────

use miniagent_agent::context::RunContext;
use miniagent_agent::Agent;
use miniagent_core::message::Message;
use miniagent_core::config::TaskComplexity;
use miniagent_provider::router::ProviderChoice;
use tokio_util::sync::CancellationToken;

pub struct AgentStage {
    agent: Arc<Agent>,
    max_iterations: usize,
    max_tokens: Option<u32>,
}

impl AgentStage {
    pub fn new(agent: Arc<Agent>) -> Self {
        Self { agent, max_iterations: 35, max_tokens: None }
    }

    pub fn with_limits(mut self, max_iterations: usize, max_tokens: u32) -> Self {
        self.max_iterations = max_iterations;
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Build system prompt augmented with relevant skills
    fn build_system_prompt(&self, base_system: &str, user_prompt: &str) -> String {
        use miniagent_skill::discovery::SkillDiscovery;
        use miniagent_skill::registry::SkillRegistry;

        let mut augmented = base_system.to_string();

        // Discover and match skills
        let discovery = SkillDiscovery::new();
        let bundles = discovery.discover();
        if bundles.is_empty() {
            return augmented;
        }

        let mut registry = SkillRegistry::new();
        for b in bundles {
            registry.register(b);
        }

        let matches = registry.find_matching(user_prompt, 3);
        if matches.is_empty() {
            return augmented;
        }

        // Log matched skills
        let skill_names: Vec<&str> = matches.iter().map(|s| s.metadata.name.as_str()).collect();
        eprintln!("   🧩 Matched skills: {}", skill_names.join(", "));

        augmented.push_str("\n\n## Available Skills for this Task\n");
        augmented.push_str("You have access to the following specialized skills. \
                            Follow their methodology when relevant:\n\n");

        for skill in matches {
            let body_preview: String = skill.body
                .lines()
                .take(30)
                .collect::<Vec<_>>()
                .join("\n");
            augmented.push_str(&format!(
                "### Skill: {}\n{}\n\n",
                skill.metadata.name,
                body_preview,
            ));
        }

        augmented.push_str("\nUse the skill methodologies above to guide your approach. \
                           Execute the skill's steps using the available tools.\n");

        augmented
    }
}

#[async_trait]
impl StageHandler for AgentStage {
    fn name(&self) -> &str { "agent" }
    fn description(&self) -> &str { "Execute task via AI agent with tool access" }

    async fn execute(&self, ctx: &StageContext) -> Result<StageOutput, StageError> {
        let prompt = ctx.input["prompt"].as_str().unwrap_or("");
        let system = ctx.input["system"].as_str().unwrap_or(
            "You are an AI agent with direct access to system tools. \
             Use tools for actions — NEVER simulate or describe tool output."
        );

        // Augment system prompt with matching skills
        let augmented_system = self.build_system_prompt(system, prompt);

        let complexity = match ctx.input["complexity"].as_str().unwrap_or("moderate") {
            "simple" => TaskComplexity::Simple,
            "complex" => TaskComplexity::Complex,
            "deep" => TaskComplexity::DeepResearch,
            _ => TaskComplexity::Moderate,
        };
        let provider = match ctx.input["provider"].as_str().unwrap_or("flash") {
            "pro" => ProviderChoice::Pro,
            _ => ProviderChoice::Flash,
        };

        let mut history: Vec<Message> = Vec::new();
        history.push(Message::user(prompt));

        let mut context = RunContext::new(augmented_system)
            .with_complexity(complexity)
            .with_provider(provider);
        context.max_tool_iterations = self.max_iterations;
        context.max_tokens = self.max_tokens;
        let cancel = CancellationToken::new();

        match self.agent.run_with_loop(&mut history, &context, cancel).await {
            Ok(delta) => {
                // run_with_loop puts final text in history, not delta.new_messages
                let text = if !delta.new_messages.is_empty() {
                    delta.new_messages.iter()
                        .map(|m| m.text_content())
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    // Extract the last assistant message from history
                    history.iter()
                        .rev()
                        .find(|m| m.role == miniagent_core::message::MessageRole::Assistant)
                        .map(|m| m.text_content())
                        .unwrap_or_default()
                };

                // Also collect tool results from history
                let tool_results: Vec<String> = history.iter()
                    .filter(|m| m.role == miniagent_core::message::MessageRole::Tool)
                    .map(|m| m.text_content())
                    .collect();

                // Persist full output to disk so downstream stages get complete context
                let output_data = serde_json::json!({
                    "response": text,
                    "tool_calls": tool_results.len(),
                    "tool_results": tool_results,
                    "tokens_in": delta.usage.input_tokens,
                    "tokens_out": delta.usage.output_tokens,
                    "stop_reason": format!("{:?}", delta.stop_reason),
                });
                save_stage_output(ctx, "research_output.json", &output_data);

                Ok(StageOutput {
                    data: output_data,
                    metadata: StageMetadata {
                        duration_ms: 0,
                        items_processed: tool_results.len() + 1,
                        success: true,
                        error: None,
                    },
                })
            }
            Err(e) => Err(StageError::Failed(format!("Agent error: {e}"))),
        }
    }
}

// ── Collect Papers Stage ─────────────────────────────────────

pub struct CollectPapersStage {
    pub sources: Vec<PaperSource>,
    pub max_papers: usize,
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaperSource {
    Arxiv,
    PubMed,
    SemanticScholar,
    WebSearch,
}

#[async_trait]
impl StageHandler for CollectPapersStage {
    fn name(&self) -> &str { "collect_papers" }
    fn description(&self) -> &str { "Collect papers from academic sources" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, StageError> {
        // Use the research pipeline instead: miniagent research -q "..." -n N
        Err(StageError::Skipped(
            "Use 'miniagent research' command for literature search. \
             Workflow stages use the full agent pipeline.".into()
        ))
    }
}

// ── Summarize Stage ──────────────────────────────────────────

pub struct SummarizeStage {
    pub template: SummaryTemplate,
    pub max_tokens_per_paper: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SummaryTemplate {
    Scientific,
    Brief,
    CodeReview,
}

#[async_trait]
impl StageHandler for SummarizeStage {
    fn name(&self) -> &str { "summarize" }
    fn description(&self) -> &str { "Generate structured summaries for each paper" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, StageError> {
        Err(StageError::Skipped(
            "Use 'miniagent research' command for summarization. \
             Workflow stages use the full agent pipeline.".into()
        ))
    }
}

// ── Synthesize Stage ─────────────────────────────────────────

pub struct SynthesizeStage {
    pub strategy: SynthesisStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SynthesisStrategy {
    CrossReference,
    ThematicCluster,
    Chronological,
    MethodComparison,
}

#[async_trait]
impl StageHandler for SynthesizeStage {
    fn name(&self) -> &str { "synthesize" }
    fn description(&self) -> &str { "Cross-reference and synthesize findings across papers" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, StageError> {
        Err(StageError::Skipped(
            "Use 'miniagent research' command for synthesis. \
             Workflow stages use the full agent pipeline.".into()
        ))
    }
}

// ── Hypothesize Stage ────────────────────────────────────────

pub struct HypothesizeStage {
    pub max_hypotheses: usize,
    pub require_mechanistic: bool,
}

#[async_trait]
impl StageHandler for HypothesizeStage {
    fn name(&self) -> &str { "hypothesize" }
    fn description(&self) -> &str { "Generate research hypotheses from synthesis" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, StageError> {
        Err(StageError::Skipped(
            "Use 'miniagent research' command for hypothesis generation. \
             Workflow stages use the full agent pipeline.".into()
        ))
    }
}

// ── Experiment Design Stage ──────────────────────────────────

pub struct ExperimentDesignStage {
    pub for_each_hypothesis: bool,
    pub include_statistical_plan: bool,
}

#[async_trait]
impl StageHandler for ExperimentDesignStage {
    fn name(&self) -> &str { "design_experiments" }
    fn description(&self) -> &str { "Design validation experiments for each hypothesis" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, StageError> {
        Err(StageError::Skipped(
            "Use 'miniagent research' command for experiment design. \
             Workflow stages use the full agent pipeline.".into()
        ))
    }
}

// ── Data Analysis Stage ──────────────────────────────────────

pub struct DataAnalysisStage {
    pub analysis_type: AnalysisType,
    pub script: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnalysisType {
    Descriptive,
    Inferential,
    MachineLearning,
    Custom,
}

#[async_trait]
impl StageHandler for DataAnalysisStage {
    fn name(&self) -> &str { "data_analysis" }
    fn description(&self) -> &str { "Run data analysis via Python runtime" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, StageError> {
        Err(StageError::Skipped(
            "Python runtime integration not yet available. \
             Use 'miniagent run' with data analysis tools.".into()
        ))
    }
}

// ── Multi-Agent: Critic Stage ──────────────────────────────────

use miniagent_provider::traits::LlmProvider;

/// A pure-LLM stage that critiques the previous stage's output.
/// Used in multi-agent pipelines after the research AgentStage.
pub struct CriticStage {
    pub provider: Box<dyn LlmProvider>,
    pub model_name: String,
}

impl CriticStage {
    pub fn new(provider: Box<dyn LlmProvider>, model_name: impl Into<String>) -> Self {
        Self { provider, model_name: model_name.into() }
    }
}

#[async_trait]
impl StageHandler for CriticStage {
    fn name(&self) -> &str { "critic" }
    fn description(&self) -> &str { "Review research findings for gaps, errors, and weaknesses" }

    async fn execute(&self, ctx: &StageContext) -> Result<StageOutput, StageError> {
        // Read full research output from disk (complete context, not truncated)
        let research_data = load_stage_output(ctx, "research_output.json");

        let research = research_data
            .as_ref()
            .and_then(|d| d["response"].as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                ctx.previous_outputs.values()
                    .find_map(|v| v["response"].as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();

        let tool_results: Vec<String> = research_data
            .as_ref()
            .and_then(|d| d["tool_results"].as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .or_else(|| {
                ctx.previous_outputs.values()
                    .find_map(|v| v["tool_results"].as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            })
            .unwrap_or_default();

        let tool_count = research_data
            .as_ref()
            .and_then(|d| d["tool_calls"].as_u64())
            .unwrap_or(tool_results.len() as u64);

        // Build research context from full tool results (no arbitrary truncation)
        let tool_context: String = tool_results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("### Tool #{i}\n{r}\n"))
            .collect::<Vec<_>>()
            .join("\n---\n");

        // Truncate only if total exceeds LLM context budget
        let tool_context = if tool_context.chars().count() > 30_000 {
            let preview: String = tool_context.chars().take(30_000).collect();
            format!("{preview}\n\n[... remaining tool results truncated for length ...]")
        } else {
            tool_context
        };

        let system = "You are a rigorous scientific reviewer. \
                      Critically evaluate the research findings. \
                      Identify gaps, missing hypotheses, overstatements, \
                      and areas needing more evidence. Be specific and constructive.";

        let prompt = if research.is_empty() && !tool_context.is_empty() {
            format!(
                "Review this research. {tool_count} tool calls were made.\n\n\
                 ## Full Tool Results\n{tool_context}\n\n\
                 ## Your Review\n\
                 1. Key findings from the tool results\n\
                 2. Missing perspectives or hypotheses\n\
                 3. What additional searches or analysis would strengthen this\n\
                 4. Suggested structure for the final output"
            )
        } else {
            format!(
                "Review this research output. {tool_count} tool calls were made.\n\n\
                 ## Research Summary\n{research}\n\n\
                 ## Full Tool Results\n{tool_context}\n\n\
                 ## Your Review\n\
                 1. Key strengths\n\
                 2. Missing perspectives or hypotheses\n\
                 3. Overstatements or unsupported claims\n\
                 4. Suggested additions"
            )
        };

        let request = CompletionRequest {
            system: system.into(),
            messages: vec![Message::user(prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.7),
                max_tokens: Some(4096),
                ..Default::default()
            },
        };

        let cancel = CancellationToken::new();
        let response = self.provider.complete(&request, cancel).await
            .map_err(|e| StageError::Failed(format!("Critic LLM error: {e}")))?;

        let text = response.content.iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Persist critique to disk
        let dir = task_workflow_dir(ctx);
        let _ = std::fs::write(dir.join("critique.md"), &text);

        Ok(StageOutput {
            data: serde_json::json!({
                "critique": text,
                "tokens_in": response.usage.input_tokens,
                "tokens_out": response.usage.output_tokens,
            }),
            metadata: StageMetadata {
                duration_ms: 0,
                items_processed: 1,
                success: true,
                error: None,
            },
        })
    }
}

// ── Multi-Agent: Synthesizer Stage ─────────────────────────────

/// A pure-LLM stage that synthesizes research + critique into final output.
pub struct SynthesizerStage {
    pub provider: Box<dyn LlmProvider>,
    pub model_name: String,
}

impl SynthesizerStage {
    pub fn new(provider: Box<dyn LlmProvider>, model_name: impl Into<String>) -> Self {
        Self { provider, model_name: model_name.into() }
    }
}

#[async_trait]
impl StageHandler for SynthesizerStage {
    fn name(&self) -> &str { "synthesizer" }
    fn description(&self) -> &str { "Synthesize research and critique into final polished output" }

    async fn execute(&self, ctx: &StageContext) -> Result<StageOutput, StageError> {
        // Read full research output and critique from disk
        let research_data = load_stage_output(ctx, "research_output.json");

        let research = research_data
            .as_ref()
            .and_then(|d| d["response"].as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                ctx.previous_outputs.values()
                    .find_map(|v| v["response"].as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();

        let tool_results: Vec<String> = research_data
            .as_ref()
            .and_then(|d| d["tool_results"].as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .or_else(|| {
                ctx.previous_outputs.values()
                    .find_map(|v| v["tool_results"].as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            })
            .unwrap_or_default();

        // Read critique from disk file for full content
        let critique = std::fs::read_to_string(
            task_workflow_dir(ctx).join("critique.md")
        ).ok().or_else(|| {
            ctx.previous_outputs.values()
                .find_map(|v| v["critique"].as_str().map(|s| s.to_string()))
        }).unwrap_or_default();

        // Build tool context from full results
        let tool_context: String = tool_results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("### Search #{i}\n{r}\n"))
            .collect::<Vec<_>>()
            .join("\n---\n");

        // Truncate only if total exceeds LLM budget (30K chars ~ 10K tokens for context)
        let tool_context = if tool_context.chars().count() > 30_000 {
            let preview: String = tool_context.chars().take(30_000).collect();
            format!("{preview}\n\n[... additional results omitted for length ...]")
        } else {
            tool_context
        };

        let system = "You are an expert scientific synthesizer. \
                      Take research findings and critical review, then produce \
                      a comprehensive, polished final output. \
                      Address the gaps identified in the critique. \
                      Use the same language as the original user request.";

        let prompt = format!(
            "Synthesize the final output using the research and critique below.\n\n\
             ## Research Summary\n{research}\n\n\
             ## Full Tool Results\n{tool_context}\n\n\
             ## Critical Review\n{critique}\n\n\
             ## Instructions\n\
             Produce a comprehensive final output that:\n\
             1. Incorporates all key findings from the research and tool results\n\
             2. Addresses gaps and concerns from the review\n\
             3. Is well-structured and complete\n\
             4. Uses appropriate scientific depth"
        );

        let request = CompletionRequest {
            system: system.into(),
            messages: vec![Message::user(prompt)],
            tools: vec![],
            config: InferenceConfig {
                max_tokens: Some(16_000),
                enable_thinking: true,
                thinking_budget: Some(8_000),
                ..Default::default()
            },
        };

        let cancel = CancellationToken::new();
        let response = self.provider.complete(&request, cancel).await
            .map_err(|e| StageError::Failed(format!("Synthesizer LLM error: {e}")))?;

        let text = response.content.iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Persist synthesis to disk
        let dir = task_workflow_dir(ctx);
        let _ = std::fs::write(dir.join("synthesis.md"), &text);

        Ok(StageOutput {
            data: serde_json::json!({
                "response": text,
                "tokens_in": response.usage.input_tokens,
                "tokens_out": response.usage.output_tokens,
            }),
            metadata: StageMetadata {
                duration_ms: 0,
                items_processed: 1,
                success: true,
                error: None,
            },
        })
    }
}

// Bring in types needed by the new stages
use miniagent_core::config::InferenceConfig;
use miniagent_core::event::ContentBlock;
use miniagent_provider::traits::CompletionRequest;

// ── Generic LLM Stage ─────────────────────────────────────────

/// A configurable pure-LLM stage. Similar to CriticStage but with a
/// custom system prompt and role name, allowing dynamic workflow generation
/// to create arbitrary processing stages.
pub struct GenericLlmStage {
    provider: Box<dyn miniagent_provider::traits::LlmProvider>,
    role_name: String,
    system_prompt: String,
}

impl GenericLlmStage {
    pub fn new(
        provider: Box<dyn miniagent_provider::traits::LlmProvider>,
        role_name: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            role_name: role_name.into(),
            system_prompt: system_prompt.into(),
        }
    }
}

#[async_trait]
impl StageHandler for GenericLlmStage {
    fn name(&self) -> &str { &self.role_name }
    fn description(&self) -> &str { "Configurable LLM processing stage" }

    async fn execute(&self, ctx: &StageContext) -> Result<StageOutput, StageError> {
        // Gather upstream outputs
        let upstream: String = ctx.previous_outputs.values()
            .filter_map(|v| v["response"].as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        let mut prompt = String::new();
        if !upstream.is_empty() {
            prompt.push_str("## Input from previous stages\n\n");
            prompt.push_str(&upstream);
            prompt.push_str("\n\n---\n\n");
        }
        prompt.push_str("Process the above input according to your role. Produce a comprehensive output.");

        let request = CompletionRequest {
            system: self.system_prompt.clone(),
            messages: vec![Message::user(&prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.3),
                max_tokens: Some(16000),
                ..Default::default()
            },
        };

        let cancel = CancellationToken::new();
        let response = self.provider.complete(&request, cancel)
            .await
            .map_err(|e| StageError::Failed(format!("LLM call failed: {e}")))?;

        let text: String = response.content.iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        // Persist to workflow dir
        let filename = format!("{}.md", self.role_name);
        let dir = task_workflow_dir(ctx);
        let _ = std::fs::write(dir.join(&filename), &text);

        Ok(StageOutput {
            data: serde_json::json!({
                "response": text,
                "tokens_in": response.usage.input_tokens,
                "tokens_out": response.usage.output_tokens,
            }),
            metadata: StageMetadata {
                duration_ms: 0,
                items_processed: 1,
                success: true,
                error: None,
            },
        })
    }
}

// ── Planner Stage ─────────────────────────────────────────────

/// Analyzes user prompt and produces a WorkflowSpec via LLM judgment.
/// Built-in patterns are provided as reference templates the LLM can adopt or ignore.
pub struct PlannerStage {
    provider: Box<dyn miniagent_provider::traits::LlmProvider>,
}

impl PlannerStage {
    pub fn new(provider: Box<dyn miniagent_provider::traits::LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl StageHandler for PlannerStage {
    fn name(&self) -> &str { "planner" }
    fn description(&self) -> &str { "Analyze task and plan workflow stages via LLM" }

    async fn execute(&self, ctx: &StageContext) -> Result<StageOutput, StageError> {
        let prompt = ctx.input["prompt"].as_str().unwrap_or("");

        if prompt.is_empty() {
            let spec = crate::builder::WorkflowSpec::single_agent();
            return Ok(spec_to_output(&spec));
        }

        eprintln!("   📋 Planning workflow...");

        let planner_prompt = format!(
            r#"Analyze this task and design a workflow to accomplish it.

## Task
{prompt}

## Available Stage Types
- "agent": Full AI agent with tool access (web_search, web_fetch, pubmed_search, bash, read, write, edit, grep, glob). Can do multi-turn research. Suitable for information gathering, code writing, data collection.
- "critic": Reviews previous stage output for gaps, errors, weaknesses, overstatements. Produces structured critique.
- "synthesizer": Integrates all previous outputs into polished final result with deep reasoning. Best for final report generation.
- "llm": Generic LLM processing stage with custom system prompt. Use for domain-specific transformation (e.g., translation, formatting, domain analysis).

## Reference Workflow Patterns (adopt if appropriate, or design your own)
1. **Simple task** (code, math, quick Q&A): 1 agent stage only.
2. **Research & synthesis** (literature review, survey, report): agent → critic → synthesizer.
3. **Writing** (draft, essay, proposal): agent → synthesizer with writing-focused system_prompt.
4. **Analysis & recommendation** (policy, strategy): agent → llm(analyst) → synthesizer.
5. **Multi-perspective** (debate, comparison): multiple agent stages → critic → synthesizer.

## Design Rules
- Use 1-5 stages. Simple tasks need only 1 agent stage.
- First stage should almost always be "agent" (to gather information with tools).
- Last stage should typically be "synthesizer" or "llm" (for polished output).
- Model tier: "flash" for search/tool-heavy stages, "pro" for complex reasoning/writing/synthesis.
- enable_skills: true only for agent stages.
- edges define execution order as [from_stage_name, to_stage_name].
- You may design custom workflows not listed above if the task demands it.

## Output Format (JSON only, no markdown)
{{
  "task_type": "descriptive_name",
  "stages": [
    {{
      "name": "stage_name",
      "handler_type": "agent|critic|synthesizer|llm",
      "system_prompt": "Custom instructions for this stage (empty string for default behavior)",
      "tools": ["web_search", "web_fetch"],
      "model_tier": "flash|pro",
      "max_iterations": 50,
      "enable_skills": true
    }}
  ],
  "edges": [["from_name", "to_name"]]
}}"#
        );

        let request = CompletionRequest {
            system: "You are a workflow planner. Analyze the task, decide the best workflow structure, and output ONLY valid JSON. No markdown fences, no explanation.".into(),
            messages: vec![Message::user(&planner_prompt)],
            tools: vec![],
            config: InferenceConfig {
                temperature: Some(0.1),
                max_tokens: Some(2000),
                ..Default::default()
            },
        };

        let cancel = CancellationToken::new();
        let response = match self.provider.complete(&request, cancel).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("   ⚠️ Planner LLM failed: {e}, using single-agent fallback");
                let spec = crate::builder::WorkflowSpec::single_agent();
                return Ok(spec_to_output(&spec));
            }
        };

        let text: String = response.content.iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        let cleaned = strip_markdown_fences(&text);
        let spec: crate::builder::WorkflowSpec = match serde_json::from_str(&cleaned) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("   ⚠️ Planner output parse error: {e}, using single-agent fallback");
                let spec = crate::builder::WorkflowSpec::single_agent();
                return Ok(spec_to_output(&spec));
            }
        };

        eprintln!("   📋 Workflow: {} ({} stages)", spec.task_type, spec.stages.len());
        Ok(spec_to_output(&spec))
    }
}

fn spec_to_output(spec: &crate::builder::WorkflowSpec) -> StageOutput {
    StageOutput {
        data: serde_json::json!({
            "workflow_spec": spec,
        }),
        metadata: StageMetadata {
            duration_ms: 0,
            items_processed: spec.stages.len(),
            success: true,
            error: None,
        },
    }
}

fn strip_markdown_fences(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let without_start = trimmed.trim_start_matches("```json")
            .trim_start_matches("```JSON")
            .trim_start_matches("```");
        without_start.trim_end_matches("```").trim().to_string()
    } else {
        s.to_string()
    }
}
