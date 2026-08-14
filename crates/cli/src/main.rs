use std::sync::Arc;
use clap::{Parser, Subcommand};
use miniagent_agent::Agent;
use miniagent_core::config::TaskComplexity;
use miniagent_core::message::Message;
use miniagent_core::secrets::ApiKey;
use miniagent_core::settings::AppConfig;
use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};
use miniagent_provider::stepfun::StepFunFlash;
use miniagent_provider::minimax::MiniMaxFlash;
use miniagent_provider::router::ProviderChoice;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

/// 根据 `config.is_stepfun()` 构造 (flash, pro) provider 对。
///
/// 所有 CLI 命令应通过此函数获取 provider，而非硬编码 `DeepSeekFlash`/`DeepSeekPro`。
/// 这样 `PROVIDER=stepfun`（.env）能正确路由到 StepFun provider，而非用占位符
/// DeepSeek key 调 API 导致 401。
///
/// StepFun 当前只有单一模型（`step-3.7-flash`），故 flash 和 pro 都用 StepFunFlash
/// （`ProviderRouter` 会按 complexity 选其中一个，StepFun 不区分 flash/pro 也没问题）。
fn make_providers(config: &AppConfig) -> (Box<dyn LlmProvider>, Box<dyn LlmProvider>) {
    if config.is_stepfun() {
        let key = config.require_stepfun_key()
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                std::process::exit(1);
            });
        (
            Box::new(StepFunFlash::new(key)),
            Box::new(StepFunFlash::new(key)),
        )
    } else if config.is_minimax() {
        let key = config.require_minimax_key()
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                std::process::exit(1);
            });
        // MiniMax exposes one M-series model; both tiers map to it.
        (
            Box::new(MiniMaxFlash::new(key)),
            Box::new(MiniMaxFlash::new(key)),
        )
    } else {
        let key = config.require_deepseek_key()
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                std::process::exit(1);
            });
        (
            Box::new(DeepSeekFlash::new(key)),
            Box::new(DeepSeekPro::new(key)),
        )
    }
}


// ── CLI ──────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "miniagent", version, about = "High-performance AI agent for long-running research tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single agent turn with a prompt
    Run {
        /// The prompt to send to the agent
        #[arg(short = 'p', long)]
        prompt: String,

        /// System prompt (optional)
        #[arg(short = 's', long)]
        system: Option<String>,

        /// Provider: flash, pro
        #[arg(short = 'P', long, default_value = "flash")]
        provider: String,

        /// Task complexity: simple, moderate, complex, deep-research
        #[arg(short = 'c', long, default_value = "moderate")]
        complexity: String,

        /// DeepSeek API key (overrides DEEPSEEK_API_KEY env var)
        #[arg(long)]
        api_key: Option<String>,

        /// Enable streaming output
        #[arg(long)]
        stream: bool,

        /// Continue conversation (read history from stdin as JSON)
        #[arg(long)]
        continue_: bool,
    },

    /// Demo the self-improvement system internals
    SelfImprove {},

    /// Research: search → KG → hypotheses for a scientific topic
    Research {
        /// Research topic or question
        #[arg(short = 'q', long)]
        query: String,

        /// Max papers to retrieve and analyze (default 20, PubMed max 500)
        #[arg(short = 'n', long, default_value = "20")]
        max_papers: usize,

        /// Skip hypothesis generation (KG + link prediction only)
        #[arg(long)]
        kg_only: bool,

        /// Generate structured validation plans (data-analysis tasks + wet-lab
        /// protocols) for the top hypotheses. Completes goal 3 of the pipeline.
        #[arg(long)]
        validate: bool,

        /// Execute the data-analysis tasks end-to-end (requires --validate).
        /// Without --data, tasks with no local data run as dry-runs (script +
        /// plan only). Completes goal 4 of the pipeline.
        #[arg(long)]
        analyze: bool,

        /// Local data file (CSV/TSV/etc.) to feed into the data-analysis tasks.
        #[arg(long)]
        data: Option<String>,

        /// Number of top-ranked hypotheses to validate (default 3).
        #[arg(long, default_value = "3")]
        top_n: usize,

        /// Enrich the KG from an external biomedical triple file (TSV/CSV).
        /// Format: `head <delim> tail [<delim> score]` with a fixed relation
        /// (gene→disease AssociatedWith by default). Use `--enrich-relation` to
        /// override. Supports DisGeNET / OMIM / custom exports.
        #[arg(long)]
        enrich_file: Option<String>,

        /// Delimiter for `--enrich-file` (default `,`).
        #[arg(long, default_value = ",")]
        enrich_delim: char,

        /// Relation type for `--enrich-file` (default `associated_with`).
        /// One of: associated_with, interacts_with, regulates, activates, inhibits.
        #[arg(long, default_value = "associated_with")]
        enrich_relation: String,

        /// Debate, cross-compare, and refine the ranked hypotheses before
        /// validation planning (closes goal 2). Defaults to ON when --validate
        /// is set; pass --no-debate to skip. Costs extra LLM calls.
        #[arg(long, default_missing_value = "true", num_args = 0..=1, require_equals = false)]
        debate: Option<bool>,

        /// Directory to hold the unified, auditable `project.json` manifest and
        /// all derived artifacts. Defaults to ./result/research/<timestamp>_<slug>.
        #[arg(long)]
        project_dir: Option<String>,

        /// Earliest publication year for retrieved papers (default 2023).
        /// Previously hard-coded; now surfaced so long-range literature reviews
        /// are possible.
        #[arg(long, default_value = "2023")]
        min_year: String,
    },

    /// Run a literature review workflow (collect → summarize → synthesize → hypothesize)
    LiteratureReview {
        /// Research query
        #[arg(short = 'q', long)]
        query: String,

        /// Maximum papers to collect (PubMed max 500)
        #[arg(short = 'n', long, default_value = "20")]
        max_papers: usize,

        /// Enable hypothesis generation with KG
        #[arg(long)]
        generate_hypotheses: bool,
    },

    /// List, search, or run skills
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },

    /// Plan: decompose a task into steps and execute
    Plan {
        /// Task to plan and execute
        #[arg(short = 'q', long)]
        query: String,
    },

    /// Multi-agent orchestration for a scientific task
    Orchestrate {
        /// Research question for the agent team
        #[arg(short = 'q', long)]
        query: String,

        /// Orchestration pattern: chain, parallel, debate, hierarchical
        #[arg(short = 'p', long, default_value = "chain")]
        pattern: String,
    },

    /// Scientific debate: structured multi-agent critique and synthesis
    Debate {
        #[arg(short = 'q', long)]
        query: String,
        #[arg(short = 'r', long, default_value = "2")]
        rounds: usize,
    },

    /// Team-based research using StateGraph pipeline
    Team {
        #[arg(short = 'q', long)]
        query: String,
    },

    /// Full orchestration: tool binding + profiles + blackboard + control shell
    /// Run the cyclic loop pipeline: Explore → Plan → Dispatch → Evaluate → Repair → ...
    Loop {
        /// The task or goal to accomplish
        #[arg(short = 'q', long)]
        query: String,

        /// Maximum number of loops (default 10)
        #[arg(short = 'n', long, default_value = "10")]
        max_loops: usize,
    },

    /// Demo the hook/interception system
    /// Show telemetry metrics
    Metrics,

    /// Show current configuration
    Config,

    /// Project management
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    Create { name: String },
    List,
}

#[derive(Subcommand)]
enum SkillAction {
    /// List all discovered skills
    List,
    /// Show a specific skill's details
    Show { name: String },
    /// Search skills matching a query
    Search { query: String },
    /// Run a skill chain on input
    Run {
        /// Skill names (comma-separated for chains)
        #[arg(short = 's', long)]
        skills: String,
        /// Input to pass to the skill
        #[arg(short = 'i', long)]
        input: String,
    },
}

// ── Main ─────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    miniagent_telemetry::init("warn");
    let config = Arc::new(AppConfig::load());
    let cli = Cli::parse();

    // Probe search backend health in background — won't block command execution.
    let needs_search = matches!(cli.command,
        Commands::Run { .. } | Commands::Research { .. } | Commands::LiteratureReview { .. }
        | Commands::Loop { .. } | Commands::Orchestrate { .. } | Commands::Debate { .. }
        | Commands::Team { .. } | Commands::Plan { .. }
    );
    if needs_search {
        tokio::spawn(async {
            miniagent_tool::probe_all_backends().await;
        });
    }

    match cli.command {
        Commands::Run {
            prompt,
            system,
            provider,
            complexity,
            api_key,
            stream: _stream,
            continue_,
        } => {
            run_command(prompt, system, provider, complexity, api_key, continue_, &config).await;
        }
        Commands::SelfImprove {} => {
            demo_self_improve();
        }
        Commands::Research {
            query,
            max_papers,
            kg_only,
            validate,
            analyze,
            data,
            top_n,
            enrich_file,
            enrich_delim,
            enrich_relation,
            debate,
            project_dir,
            min_year,
        } => {
            // --debate defaults to ON when --validate is set; --no-debate forces off.
            let debate = debate.unwrap_or(validate);
            research_pipeline(
                &query,
                max_papers,
                kg_only,
                validate,
                analyze,
                data.as_deref(),
                top_n,
                enrich_file.as_deref(),
                enrich_delim,
                enrich_relation.as_str(),
                debate,
                project_dir.as_deref(),
                &min_year,
                &config,
            )
            .await;
        }
        Commands::LiteratureReview {
            query,
            max_papers,
            generate_hypotheses,
        } => {
            literature_review(&query, max_papers, generate_hypotheses, &config).await;
        }
        Commands::Skill { action } => match action {
            SkillAction::List => skill_list(),
            SkillAction::Show { name } => skill_show(&name),
            SkillAction::Search { query } => skill_search(&query),
            SkillAction::Run { skills, input } => skill_run(&skills, &input).await,
        },
        Commands::Plan { query } => {
            plan_command(&query, &config).await;
        }
        Commands::Orchestrate { query, pattern } => {
            orchestrate_command(&query, &pattern, &config).await;
        }
        Commands::Debate { query, rounds } => {
            debate_command(&query, rounds, &config).await;
        }
        Commands::Loop { query, max_loops } => {
            loop_command(&query, max_loops, &config).await;
        }
        Commands::Team { query } => {
            team_command(&query, &config).await;
        }
        Commands::Metrics => {
            show_metrics();
        }
        Commands::Config => {
            show_config(&config);
        }
        Commands::Project { action } => match action {
            ProjectAction::Create { name } => {
                println!("Project '{name}' created. Use 'miniagent research' or 'miniagent run' within this project.");
            }
            ProjectAction::List => {
                println!("Projects: (use filesystem-based organization under ./projects/)");
            }
        },
    }
}

// ── Config command ───────────────────────────────────────────

fn show_config(config: &AppConfig) {
    println!("Current configuration:");
    println!();
    println!("  DEEPSEEK_API_KEY:  {}", config.deepseek_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  DEEPSEEK_BASE_URL: {}", config.deepseek_base_url);
    if let Some(ref m) = config.deepseek_model_name {
        println!("  DEEPSEEK_MODEL_NAME: {m}");
    }
    println!("  BOCHA_API_KEY:     {}", config.bocha_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  TAVILY_API_KEY:    {}", config.tavily_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  SERPAPI_API_KEY:   {}", config.serpapi_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  SERPER_API_KEY:    {}", config.serper_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  LANGSEARCH_API_KEY: {}", config.langsearch_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  ANYSEARCH_API_KEY: {}", config.anysearch_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!("  PUBMED_API_KEY:    {}", config.pubmed_api_key.as_ref().map(|k| k.masked()).unwrap_or("(not set)".into()));
    println!();
    println!("  MAX_ITERATIONS:    {}", config.max_iterations);
    println!("  MAX_TOKENS:        {}", config.max_tokens);
    println!("  LOOP_MAX_LOOPS:    {}", config.loop_max_loops);
    println!();
    if config.deepseek_api_key.is_none() {
        println!("⚠  DEEPSEEK_API_KEY not set — add it to .env or use --api-key flag");
    }
}

// ── Skill commands ────────────────────────────────────────────

fn skill_list() {
    use miniagent_skill::discovery::SkillDiscovery;
    let discovery = SkillDiscovery::new();
    let bundles = discovery.discover();

    if bundles.is_empty() {
        println!("No skills discovered. Add SKILL.md files to skills/<name>/SKILL.md");
        println!("Default search path: ./skills/");
        return;
    }

    println!("🧩 Discovered {} skills:\n", bundles.len());
    for bundle in &bundles {
        println!("  📋 {} (v{}) — priority: {}",
            bundle.metadata.name,
            bundle.metadata.version,
            bundle.metadata.priority,
        );
        println!("     {}", bundle.metadata.description);
        if !bundle.metadata.triggers.is_empty() {
            println!("     Triggers: {}", bundle.metadata.triggers.join(", "));
        }
        if !bundle.metadata.tools_needed.is_empty() {
            println!("     Tools: {}", bundle.metadata.tools_needed.join(", "));
        }
        println!();
    }
}

fn skill_show(name: &str) {
    use miniagent_skill::discovery::SkillDiscovery;
    use miniagent_skill::registry::SkillRegistry;

    let discovery = SkillDiscovery::new();
    let bundles = discovery.discover();
    let mut registry = SkillRegistry::new();
    for b in bundles { registry.register(b); }

    match registry.get_by_name(name) {
        Some(bundle) => {
            println!("🧩 Skill: {}\n", bundle.metadata.name);
            println!("  Version:     {}", bundle.metadata.version);
            println!("  Priority:    {}", bundle.metadata.priority);
            println!("  Actionable:  {}", bundle.metadata.actionable);
            println!("  Description: {}", bundle.metadata.description);
            if !bundle.metadata.triggers.is_empty() {
                println!("  Triggers:    {}", bundle.metadata.triggers.join(", "));
            }
            if !bundle.metadata.tools_needed.is_empty() {
                println!("  Tools:       {}", bundle.metadata.tools_needed.join(", "));
            }
            println!("\n─── Skill Body ───\n");
            println!("{}", bundle.body);
        }
        None => {
            eprintln!("Skill '{name}' not found");
        }
    }
}

fn skill_search(query: &str) {
    use miniagent_skill::discovery::SkillDiscovery;
    use miniagent_skill::registry::SkillRegistry;

    let discovery = SkillDiscovery::new();
    let bundles = discovery.discover();
    let mut registry = SkillRegistry::new();
    for b in bundles { registry.register(b); }

    let matches = registry.find_matching(query, 10);
    if matches.is_empty() {
        println!("No skills match '{}'", query);
        return;
    }

    println!("🔍 Skills matching '{}':\n", query);
    for bundle in matches {
        println!("  📋 {} — {}", bundle.metadata.name, bundle.metadata.description);
    }
}

async fn skill_run(skills: &str, input: &str) {
    use miniagent_skill::discovery::SkillDiscovery;
    use miniagent_skill::registry::SkillRegistry;
    use miniagent_skill::executor::SkillChain;
    use std::sync::Arc;

    let discovery = SkillDiscovery::new();
    let bundles = discovery.discover();
    let mut registry = SkillRegistry::new();
    for b in bundles { registry.register(b); }

    let skill_names: Vec<String> = skills.split(',').map(|s| s.trim().to_string()).collect();
    let registry = Arc::new(registry);

    match SkillChain::new(skill_names.clone(), registry).build_prompt(input) {
        Ok(prompt) => {
            println!("⚡ Skill chain: {} → running...\n", skill_names.join(" → "));
            println!("{}", prompt);
            println!("\n─── Execute the above prompt with: miniagent run -p \"...\" -P pro -c complex");
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

fn show_metrics() {
    use miniagent_telemetry::metrics;
    let m = metrics::snapshot();
    println!("📊 Miniagent Telemetry Metrics\n");
    println!("  Agent:");
    println!("    Runs:        {}", m.agent_runs);
    println!("    Failures:    {}", m.agent_failures);
    println!("    Avg Latency: {:.0}ms", m.avg_latency_ms);
    println!("    Input Tokens:  {}", m.total_input_tokens);
    println!("    Output Tokens: {}", m.total_output_tokens);
    println!();
    println!("  Tools:");
    println!("    Total Calls:   {}", m.tool_calls);
    println!("    Failures:      {}", m.tool_failures);
    println!("    Web Search:    {}", m.web_search_calls);
    println!("    PubMed:        {}", m.pubmed_calls);
    println!("    Fetch:         {}", m.fetch_calls);
    println!();
    println!("  Provider:");
    println!("    Calls:    {}", m.provider_calls);
}

// ── Run command ──────────────────────────────────────────────

async fn run_command(
    prompt: String,
    system: Option<String>,
    provider: String,
    complexity: String,
    api_key_override: Option<String>,
    continue_: bool,
    config: &Arc<AppConfig>,
) {
    // Resolve API key: CLI override takes precedence over config（尊重 PROVIDER 配置）
    let key: ApiKey = if let Some(k) = api_key_override {
        ApiKey::new(k)
    } else {
        match config.require_active_key() {
            Ok(k) => k.clone(),
            Err(e) => {
                eprintln!("Error: {e}");
                return;
            }
        }
    };

    let system_prompt = system.unwrap_or_else(|| {
        "You are an AI agent with direct access to system tools. You MUST use tools for actions — NEVER simulate or describe tool output.\n\
         \n\
         CRITICAL RULES:\n\
         1. To create/edit files → use write/edit tool. NEVER output file content as text.\n\
         2. To read files → use read tool. NEVER guess file contents.\n\
         3. To search papers → use pubmed_search. NEVER pretend to know paper titles.\n\
         4. To execute commands → use bash tool. NEVER simulate command output.\n\
         5. To search the web → use web_search. NEVER fabricate URLs or results.\n\
         \n\
         If a user asks you to create a file, use the write tool IMMEDIATELY.\n\
         If you don't have a tool for something, say so honestly.\n\
         Available tools: pubmed_search, web_search, web_fetch, read, write, edit, glob, grep, bash.".into()
    });

    let system_prompt_for_workflow = system_prompt.clone();

    let complexity = match complexity.as_str() {
        "simple" => TaskComplexity::Simple,
        "moderate" => TaskComplexity::Moderate,
        "complex" => TaskComplexity::Complex,
        "deep-research" | "deep" => TaskComplexity::DeepResearch,
        _ => {
            eprintln!("Unknown complexity '{}'. Using moderate.", complexity);
            TaskComplexity::Moderate
        }
    };

    let _prompt_for_file = prompt.clone();
    let (agent, _) = build_full_agent(&provider, Some(key.clone()), config);

    let mut history: Vec<Message> = Vec::new();

    if continue_ {
        let mut stdin = std::io::stdin();
        let mut input = String::new();
        std::io::Read::read_to_string(&mut stdin, &mut input).ok();
        if !input.trim().is_empty()
            && let Ok(msgs) = serde_json::from_str::<Vec<Message>>(&input) {
                history = msgs;
            }
    }

    history.push(Message::user(&prompt));

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    let provider_name = match provider.as_str() {
        "flash" => "DeepSeek Flash",
        "pro" => "DeepSeek Pro (reasoning)",
        _ => "Unknown",
    };

    let tool_count = agent
        .tool_executor()
        .and_then(|guard| guard.as_ref().map(|e| e.registry().len()))
        .unwrap_or(0);

    eprintln!("🤖 Agent running with {provider_name}");
    eprintln!("   Complexity: {complexity:?} | Tools: {tool_count} | Max iterations: {} | Max tokens: {}\n",
        config.max_iterations, config.max_tokens);

    // Build and execute workflow through the DAG engine
    use miniagent_workflow::stage::StageHandler as _;
    use miniagent_workflow::stages::PlannerStage;
    use miniagent_workflow::builder::{WorkflowSpec, WorkflowBuilder, StageSpec};

    let agent_arc = Arc::new(agent);

    // Generate task-specific output directory: result/{id}_{brief}
    let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let task_brief = sanitize_task_brief(&prompt);
    let task_dir_name = format!("{}_{}", task_id, task_brief);
    let task_dir = std::path::PathBuf::from("./result").join(&task_dir_name);
    let task_workflow_dir = task_dir.join(".workflow");
    let _ = std::fs::create_dir_all(&task_workflow_dir);

    // Use dynamic planner for all tasks（根据 PROVIDER 配置选择 provider）
    let workflow = {
        let planner_flash: Box<dyn LlmProvider> = if config.is_stepfun() {
            Box::new(StepFunFlash::new(&key))
        } else if config.is_minimax() {
            Box::new(MiniMaxFlash::new(&key))
        } else {
            Box::new(DeepSeekFlash::new(&key))
        };
        let planner = PlannerStage::new(planner_flash);
        let plan_ctx = miniagent_workflow::stage::StageContext::new(
            miniagent_core::types::StageId::new(),
            serde_json::json!({ "prompt": prompt }),
            std::collections::HashMap::new(),
            tokio_util::sync::CancellationToken::new(),
        );

        let plan_output = planner.execute(&plan_ctx).await
            .unwrap_or_else(|e| {
                eprintln!("   ⚠️ Planner failed: {e}, using single-agent");
                miniagent_workflow::stage::StageOutput {
                    data: serde_json::json!({ "workflow_spec": WorkflowSpec {
                        task_type: "single_agent".into(),
                        stages: vec![StageSpec {
                            name: "agent".into(),
                            handler_type: "agent".into(),
                            system_prompt: String::new(),
                            tools: vec![],
                            model_tier: "flash".into(),
                            max_iterations: 50,
                            enable_skills: true,
                            description: String::new(),
                            sub_tasks: vec![],
                        }],
                        edges: vec![],
                    } }),
                    metadata: miniagent_workflow::stage::StageMetadata {
                        duration_ms: 0,
                        items_processed: 0,
                        success: true,
                        error: None,
                    },
                }
            });

        let spec: WorkflowSpec = serde_json::from_value(
            plan_output.data["workflow_spec"].clone()
        ).unwrap_or_else(|_| WorkflowSpec {
            task_type: "single_agent".into(),
            stages: vec![StageSpec {
                name: "agent".into(),
                handler_type: "agent".into(),
                system_prompt: String::new(),
                tools: vec![],
                model_tier: "flash".into(),
                max_iterations: 50,
                enable_skills: true,
                description: String::new(),
                sub_tasks: vec![],
            }],
            edges: vec![],
        });

        eprintln!("   Workflow: {} ({} stages)", spec.task_type, spec.stages.len());
        for (i, s) in spec.stages.iter().enumerate() {
            eprintln!("     {}. {} [{}] ({})", i + 1, s.name, s.handler_type, s.model_tier);
        }

        let builder = WorkflowBuilder::new(agent_arc.clone(), config.clone())
            .with_task_dir(task_workflow_dir.to_string_lossy());
        builder.build(&spec, &prompt, &system_prompt_for_workflow)
            .unwrap_or_else(|e| {
                eprintln!("   ⚠️ Workflow build failed: {e}, using single-agent");
                let fallback = WorkflowSpec {
                    task_type: "single_agent".into(),
                    stages: vec![StageSpec {
                        name: "agent".into(),
                        handler_type: "agent".into(),
                        system_prompt: String::new(),
                        tools: vec![],
                        model_tier: "flash".into(),
                        max_iterations: 50,
                        enable_skills: true,
                        description: String::new(),
                        sub_tasks: vec![],
                    }],
                    edges: vec![],
                };
                WorkflowBuilder::new(agent_arc.clone(), config.clone())
                    .with_task_dir(task_workflow_dir.to_string_lossy())
                    .build(&fallback, &prompt, &system_prompt_for_workflow)
                    .expect("single-agent fallback should always build")
            })
    };

    // Execute via workflow engine
    match workflow.run(None, cancel).await {
        Ok(result) => {
            for output in result.stage_outputs.values() {
                let data = &output.data;

                // Print critique output
                if let Some(critique) = data["critique"].as_str()
                    && !critique.is_empty() {
                        println!("\n\x1b[33m── 🔍 Critical Review ──\x1b[0m");
                        println!("{critique}");
                    }

                // Print agent response (research or synthesis)
                if let Some(response) = data["response"].as_str()
                    && !response.is_empty() {
                        println!("{}", response);
                    }

                // Print tool results (research stage only)
                if let Some(tool_results) = data["tool_results"].as_array()
                    && !tool_results.is_empty() {
                        println!("\n\x1b[90m── Tool Results ({}) ──\x1b[0m", tool_results.len());
                        for result in tool_results.iter().rev().take(5) {
                            if let Some(text) = result.as_str() {
                                let preview: String = text.chars().take(200).collect();
                                println!("\x1b[90m{}\x1b[0m", preview);
                                if text.len() > 200 {
                                    println!("\x1b[90m... ({} more chars)\x1b[0m", text.len() - 200);
                                }
                            }
                        }
                    }

                // Print stats
                let tokens_in = data["tokens_in"].as_u64().unwrap_or(0);
                let tokens_out = data["tokens_out"].as_u64().unwrap_or(0);
                let stage_label = if data["critique"].is_string() {
                    "Critic"
                } else if data["tool_results"].is_array() {
                    "Research"
                } else if data["response"].is_string() {
                    "Synthesizer"
                } else {
                    "Stage"
                };
                let stop_reason = data["stop_reason"].as_str().unwrap_or("");

                if tokens_in > 0 {
                    eprintln!(
                        "\n📊 [{stage_label}] Tokens: {tokens_in} in / {tokens_out} out{}",
                        if stop_reason.is_empty() { String::new() } else { format!(" | Stop: {stop_reason}") }
                    );
                }
            }

            // Persist final output to disk
            let final_content = result.stage_outputs.values()
                .find_map(|o| o.data["response"].as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty());

            if let Some(content) = final_content {
                let output_filename = format!("{}.md", task_brief);
                let filepath = task_dir.join(&output_filename);
                match std::fs::write(&filepath, &content) {
                    Ok(_) => eprintln!("\n📄 Final output: {}", filepath.display()),
                    Err(e) => eprintln!("\n\x1b[33mWarning: Could not write output file: {e}\x1b[0m"),
                }
            }

            eprintln!("📁 Workflow artifacts: {}", task_workflow_dir.display());
        }
        Err(e) => {
            eprintln!("\x1b[31mError: {}\x1b[0m", e);
            std::process::exit(1);
        }
    }
}

/// Try to extract a filename from a Chinese/English prompt.
/// e.g. "新建一个AD_hypothesis文件" → "AD_hypothesis.md"
///      "create a file called foo.txt" → "foo.txt"
/// Generate a filesystem-safe brief from a prompt for use as directory name.
/// Takes the first ~30 chars, replaces non-alphanumeric with underscore.
fn sanitize_task_brief(prompt: &str) -> String {
    let brief: String = prompt
        .chars()
        .take(30)
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let brief = brief.trim_end_matches('_');
    // Avoid empty
    if brief.is_empty() { "task".into() } else { brief.into() }
}

/// Build agent wired with full ToolExecutor, Memory, and Skills
fn build_full_agent(
    provider: &str,
    api_key: Option<ApiKey>,
    config: &Arc<AppConfig>,
) -> (Agent, Option<ProviderChoice>) {
    use miniagent_tool::tools;
    use miniagent_tool::approval::AutoApprove;
    use miniagent_tool::executor::ToolExecutor;
    use miniagent_skill::discovery::SkillDiscovery;
    use miniagent_memory::manager::MemoryManager;

    let key = api_key.unwrap_or_else(|| {
        // 用 config 的 active key（尊重 PROVIDER=stepfun 配置）
        match config.require_active_key() {
            Ok(k) => k.clone(),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    });

    // Build tool registry with all built-in tools
    let tool_registry = tools::defaults();

    // Discover and load skills
    let skill_discovery = SkillDiscovery::new();
    let skill_bundles = skill_discovery.discover();
    let skill_count = skill_bundles.len();

    // Build agent with providers（根据 PROVIDER 配置选择 StepFun 或 DeepSeek）
    let (flash, pro) = make_providers(config);
    let _ = &key; // key 已在 make_providers 内部解析（StepFun/DeepSeek 各自取）
    let choice = match provider {
        "pro" => Some(ProviderChoice::Pro),
        _ => Some(ProviderChoice::Flash),
    };
    let agent = Agent::new(flash, pro).with_config(config.clone());

    // Wire up tool executor with auto-approve policy
    let executor = ToolExecutor::new(tool_registry, Box::new(AutoApprove));

    // Wire up in-memory memory manager
    let memory = MemoryManager::new_in_memory().unwrap_or_else(|_| {
        MemoryManager::new_in_memory().expect("in-memory SQLite should always work")
    });

    let agent = agent
        .with_tools(executor)
        .with_memory(memory);

    if skill_count > 0 {
        eprintln!("   Skills loaded: {skill_count}");
    }

    (agent, choice)
}

// ── Literature Review command ─────────────────────────────────

async fn literature_review(
    query: &str,
    max_papers: usize,
    generate_hypotheses: bool,
    config: &Arc<AppConfig>,
) {
    // Delegate to the real research pipeline
    let kg_only = !generate_hypotheses;
    research_pipeline(query, max_papers, kg_only, false, false, None, 3, None, ',', "associated_with", false, None, "2023", config).await;
}

// ── Self-Improvement demo ─────────────────────────────────────

fn demo_self_improve() {
    use miniagent_self_improve::SelfImprover;

    println!("🧠 Self-Improvement System Demo\n");

    let mut improver = SelfImprover::new();

    // 1. Q-Learning Router
    println!("1. Q-Learning Router:");
    let state = improver.decide_routing(7, 60);
    let decision = improver.q_router.decide(&state);
    println!("   Task complexity=7, budget=60%");
    println!("   Model: {:?}", decision.model);
    println!("   Search: {:?}", decision.search_strategy);
    println!("   Retrieval: {:?}", decision.retrieval_depth);
    println!("   Stats: {} entries after {} steps", improver.q_router.stats().total_entries, improver.q_router.stats().total_steps);

    // Simulate learning with reward feedback
    for i in 0..50 {
        let s = improver.decide_routing(i % 10, 100 - i);
        let d = improver.q_router.decide(&s);
        // Simulate reward: flash gets higher reward for simple tasks, pro for complex
        let reward = match d.model {
            miniagent_self_improve::online::q_router::RouterAction::UseFlash if s.complexity_level < 5 => 1.0,
            miniagent_self_improve::online::q_router::RouterAction::UsePro if s.complexity_level >= 5 => 1.0,
            _ => 0.2,
        };
        let next_s = improver.decide_routing((i + 1) % 10, 100 - i - 1);
        improver.q_router.update(&s, d.model, reward, &next_s);
        improver.q_router.decay_exploration();
    }
    println!("   After 50 iterations: {} Q-table entries, {} steps, epsilon={:.3}",
        improver.q_router.stats().total_entries,
        improver.q_router.stats().total_steps,
        improver.q_router.stats().epsilon,
    );

    // 2. Experience Graph
    println!("\n2. Experience Graph:");
    improver.experience_graph.add_experience(
        miniagent_self_improve::offline::experience_graph::NodeType::SuccessPattern,
        "Using Flash for simple queries reduced latency by 60%",
        &["Route simple tasks to Flash".to_string()],
        &[0.1, 0.2, 0.1],
    );
    improver.experience_graph.add_experience(
        miniagent_self_improve::offline::experience_graph::NodeType::FailurePattern,
        "Pro model hallucinated on factual query about gene names",
        &["Verify gene names against database before reporting".to_string()],
        &[0.2, 0.3, 0.1],
    );
    println!("   Nodes: {}, Edges: {}", improver.experience_graph.node_count(), improver.experience_graph.edge_count());

    // 3. Skill Manager
    println!("\n3. Skill Manager:");
    let skill_id = {
        let skill = improver.skill_manager.create_skill(
            "paper_summarization",
            "Use structured template: Background → Methods → Findings → Limitations → Contributions",
            &[],
        );
        skill.id
    };
    improver.skill_manager.record_usage(&skill_id, 0.9);
    improver.skill_manager.record_usage(&skill_id, 0.85);
    improver.skill_manager.record_usage(&skill_id, 0.92);
    improver.skill_manager.record_usage(&skill_id, 0.88);
    improver.skill_manager.record_usage(&skill_id, 0.91);
    let skill = improver.skill_manager.all_skills().iter().find(|s| s.id == skill_id).unwrap();
    println!("   Skill '{}': avg={:.2}, status={:?}",
        skill.name,
        skill.performance.average,
        skill.status,
    );
    println!("   Meta-skill: {}", improver.skill_manager.meta_skill_content());

    // 4. Tool Tracker
    println!("\n4. Tool Tracker:");
    improver.tool_tracker.record_success("web_search", 250);
    improver.tool_tracker.record_success("web_search", 180);
    improver.tool_tracker.record_failure("web_search", "timeout");
    improver.tool_tracker.record_success("read", 5);
    improver.tool_tracker.record_success("grep", 15);
    for tool in improver.tool_tracker.all() {
        println!("   {}: success_rate={:.2}, avg_latency={:.0}ms, calls={}",
            tool.tool_name, tool.success_rate, tool.avg_latency_ms, tool.call_count);
    }

    // 5. Lifecycle Guard
    println!("\n5. Lifecycle Guard:");
    let guard_result = improver.guard_skill(8, uuid::Uuid::new_v4(), 0.85, 25);
    println!("   Skill with score 0.85/25 uses → {:?}", guard_result);
    let guard_result = improver.guard_skill(10, uuid::Uuid::new_v4(), 0.25, 30);
    println!("   Skill with score 0.25/30 uses → {:?}", guard_result);

    println!("\n✅ Self-improvement system demo complete.");
    println!("   Total stats: {:?}", improver.stats());
}

// ── Research Pipeline ─────────────────────────────────────────

async fn research_pipeline(
    query: &str,
    max_papers: usize,
    kg_only: bool,
    validate: bool,
    analyze: bool,
    data: Option<&str>,
    top_n: usize,
    enrich_file: Option<&str>,
    enrich_delim: char,
    enrich_relation: &str,
    debate: bool,
    project_dir: Option<&str>,
    min_year: &str,
    config: &Arc<AppConfig>,
) {
    use miniagent_kg::embedding::KgeModel;
    use miniagent_kg::link_prediction::LinkPredictionScorer;
    use miniagent_kg::schema::{RelationType};
    use miniagent_kg::KnowledgeGraph;
    
    use miniagent_tool::tools::{PubMedTool};
    use miniagent_tool::traits::{Tool, ToolContext};
    use miniagent_hypothesis::generator::HypothesisGenerator;
    use miniagent_hypothesis::ranking::HypothesisRanker;
    use tokio_util::sync::CancellationToken;
    use std::time::Instant;

    let _key = match config.require_active_key() {
        Ok(k) => k.clone(),
        Err(e) => { eprintln!("{e}"); return; }
    };

    // Unified, auditable project directory + manifest (goal 1: traceability).
    let project_dir: std::path::PathBuf = match project_dir {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let slug: String = query
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>()
                .trim_matches('_')
                .chars()
                .take(30)
                .collect();
            std::path::PathBuf::from("result").join("research").join(format!("{ts}_{slug}"))
        }
    };
    let _ = std::fs::create_dir_all(&project_dir);

    // Resume support (goal 1: long-running tasks). When `project.json` already
    // exists in the project dir, reload it and skip stages already completed —
    // each stage persists its artifacts under the project dir for this purpose.
    let mut manifest = if project_dir.join(miniagent_research::MANIFEST_FILENAME).exists() {
        match miniagent_research::ProjectManifest::load(&project_dir) {
            Ok(m) => {
                println!("   ↻ resume: {} completed stage(s) loaded from existing project.json",
                    m.completed_stage_names().len());
                let mut m = m;
                m.log_event("pipeline_resumed", format!("dir={}", project_dir.display()));
                m
            }
            Err(e) => {
                eprintln!("   ⚠ resume failed ({e}); starting a fresh manifest");
                miniagent_research::ProjectManifest::new(query, project_dir.clone())
            }
        }
    } else {
        miniagent_research::ProjectManifest::new(query, project_dir.clone())
    };
    if manifest.query != query {
        manifest.log_event("query_changed", format!("{} => {}", manifest.query, query));
    }

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  miniagent Research Pipeline                                 ║");
    println!("║  Query: {:<52}║", truncate(query, 52));
    println!("║  Max papers: {:<47}║", max_papers);
    println!("║  KG only: {:<50}║", kg_only);
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let cancel = CancellationToken::new();
    let ctx = ToolContext::new(std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(), "research".to_string());
    let start = Instant::now();
    // Shared flash provider handle for fan-out stages (goal 1: performance) —
    // `complete` takes `&self`, so one client is shared across parallel calls.
    let flash: std::sync::Arc<dyn LlmProvider> = make_providers(config).0.into();

    // ── Phases 1–2: Literature Search + Abstracts (resumable) ──────
    let papers_path = project_dir.join("papers.json");
    let mut paper_texts: Vec<(String, String)> = Vec::new();
    let resumed_papers: Option<Vec<(String, String)>> = if manifest.is_stage_done("abstracts") {
        std::fs::read(&papers_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    } else {
        None
    };
    let (phase1_dur, phase2_dur) = if let Some(p) = resumed_papers.filter(|p| !p.is_empty()) {
        paper_texts = p;
        println!("━━━ Phase 1–2: ↻ resumed — {} abstracts from {} ━━━",
            paper_texts.len(), papers_path.display());
        (std::time::Duration::default(), std::time::Duration::default())
    } else {
    // ── Phase 1: Translate query to PubMed syntax if needed ──────
    let pubmed_query = if has_non_english(query) {
        let translation_prompt = format!(
            "Convert this research question into a PubMed search query.\n\
             Use English terms with boolean operators (AND/OR/NOT).\n\
             Prefer broad text-word searches over restrictive MeSH tags.\n\
             Include synonyms and variant spellings with OR.\n\
             Return ONLY the PubMed query string, nothing else.\n\n\
             Research question: {query}\n\n\
             PubMed query:"
        );
        let request = miniagent_provider::traits::CompletionRequest {
            system: "You are a PubMed search expert. Output ONLY the query string.".into(),
            messages: vec![miniagent_core::message::Message::user(&translation_prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0), max_tokens: Some(100), ..Default::default()
            },
        };
        match flash.complete(&request, cancel.child_token()).await {
            Ok(resp) => {
                let translated = resp.content.iter()
                    .filter_map(|b| match b {
                        miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    }).collect::<Vec<_>>().join("").trim().to_string();
                eprintln!("   Query translated: {query} → {translated}");
                translated
            }
            Err(_) => query.to_string(),
        }
    } else {
        query.to_string()
    };

    // ── Phase 1b: Search PubMed (multi-batch pagination) ──────────
    let phase_start = Instant::now();
    println!("━━━ Phase 1: Literature Search ━━━");
    println!("   PubMed query: {pubmed_query}");

    let pubmed = PubMedTool::new();
    let page_size = 200usize; // reliable PubMed batch size (ESummary URL limit)
    let mut all_pmids: Vec<String> = Vec::new();
    let mut total_hits = 0usize;
    let batches_needed = max_papers.div_ceil(page_size);

    for batch in 0..batches_needed {
        let offset = batch * page_size;
        let remaining = max_papers.saturating_sub(all_pmids.len());
        let batch_size = remaining.min(page_size);

        let pubmed_result = pubmed.execute(
            serde_json::json!({
                "query": pubmed_query,
                "max_results": batch_size,
                "offset": offset,
                "min_year": min_year
            }),
            &ctx, cancel.child_token(),
        ).await.unwrap_or_else(|e| miniagent_tool::traits::ToolOutput {
            content: format!("PubMed error: {e}"), metadata: None,
        });

        let batch_pmids: Vec<String> = pubmed_result.content.lines()
            .filter_map(|l| l.strip_prefix("   PMID: "))
            .filter_map(|s| s.split(' ').next())
            .map(|s| s.to_string())
            .collect();

        if total_hits == 0 {
            total_hits = pubmed_result.content.lines()
                .find(|l| l.starts_with("Total results:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.split('|').next())
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
        }

        all_pmids.extend(batch_pmids);

        if batches_needed > 1 {
            eprintln!("   Batch {}/{}: {} PMIDs (total: {})",
                batch + 1, batches_needed, all_pmids.len(), all_pmids.len());
        }

        if all_pmids.len() >= max_papers { break; }
        // Rate limit: PubMed allows 3 requests/sec without API key, 10/sec with
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }

    let pmids = all_pmids;
    println!("   PubMed: {total_hits} total, {} retrieved ({} batches)",
        pmids.len(), batches_needed);
    let phase1_dur = phase_start.elapsed();
    manifest.record_stage(
        "search",
        miniagent_research::StageStatus::Completed,
        phase1_dur,
        vec![],
        Some(serde_json::json!({ "retrieved": pmids.len() })),
    );

    // ── Phase 2: Fetch Abstracts via PubMed E-utilities (parallel batches) ─
    let phase_start = Instant::now();
    println!("\n━━━ Phase 2: Fetch Abstracts ({} papers) ━━━", pmids.len());

    let pubmed_key = std::env::var("PUBMED_API_KEY").unwrap_or_default();
    let client = reqwest::Client::builder()
        .user_agent("miniagent/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("failed to build HTTP client");
    let client = std::sync::Arc::new(client);
    let chunk_size = 20; // efetch batch size
    for chunk in pmids.chunks(chunk_size) {
        let batch: Vec<_> = chunk.iter().map(|pmid| {
            let client = client.clone();
            let pmid = pmid.clone();
            let key = pubmed_key.clone();
            let cancel = cancel.child_token();
            tokio::spawn(async move {
                // Use PubMed E-utilities efetch for clean abstract text
                let mut url = format!(
                    "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi?db=pubmed&id={pmid}&rettype=abstract&retmode=text"
                );
                if !key.is_empty() {
                    url.push_str(&format!("&api_key={key}"));
                }
                match tokio::select! {
                    _ = cancel.cancelled() => None,
                    r = client.get(&url).send() => r.ok(),
                } {
                    Some(resp) => {
                        match resp.text().await {
                            Ok(body) => {
                                let text: String = body
                                    .lines()
                                    .filter(|l| !l.trim().is_empty())
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                // Filter: skip papers without real abstract text
                                let clean = text.trim().to_lowercase();
                                let word_count = text.split_whitespace().count();
                                if word_count < 30           // too short for an abstract
                                    || clean.contains("no abstract")
                                    || clean.contains("javascript")
                                    || clean.starts_with("<")
                                    || clean.contains("pubmed central")
                                    || clean.contains("nih public access")
                                {
                                    None // Not a usable abstract
                                } else {
                                    Some((pmid, text))
                                }
                            }
                            Err(_) => None,
                        }
                    }
                    None => None,
                }
            })
        }).collect();

        for task in batch {
            if let Some(paper) = task.await.unwrap_or(None) {
                paper_texts.push(paper);
            }
        }

        let pct = (paper_texts.len() * 100 / pmids.len().min(max_papers)).min(100);
        eprintln!("   Progress: {}/{} ({}%)", paper_texts.len(), pmids.len().min(max_papers), pct);

        if paper_texts.len() >= max_papers { break; }
    }

    println!("   Fetched {} abstracts", paper_texts.len());
    // Persist the fetched corpus for resume + audit (goal 1).
    if let Ok(json) = serde_json::to_vec(&paper_texts) {
        let _ = std::fs::write(&papers_path, json);
    }
    let phase2_dur = phase_start.elapsed();
    manifest.record_stage(
        "abstracts",
        miniagent_research::StageStatus::Completed,
        phase2_dur,
        vec![papers_path.clone()],
        Some(serde_json::json!({ "fetched": paper_texts.len() })),
    );
    let _ = manifest.save();
    (phase1_dur, phase2_dur)
    };

    // ── Phase 2b: Relevance Filter ────────────────────────────────
    // PubMed keyword recall is broad; a single off-topic abstract can
    // dominate link prediction with hub entities unrelated to the query
    // (observed: one sarcopenia paper redirected every hypothesis away from
    // the queried disease). A cheap flash-model filter keeps the corpus
    // on-topic; rejections are persisted for audit.
    if !manifest.is_stage_done("relevance_filter") && !paper_texts.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 2b: Relevance Filter ━━━");
        let (kept, rejected) =
            filter_irrelevant_papers(flash.clone(), query, &paper_texts, cancel.child_token()).await;
        println!(
            "   kept {} / {} (rejected {} as off-topic)",
            kept.len(),
            paper_texts.len(),
            rejected.len()
        );
        if !rejected.is_empty() {
            let dump: Vec<serde_json::Value> = rejected
                .iter()
                .map(|(pmid, reason)| serde_json::json!({"pmid": pmid, "reason": reason}))
                .collect();
            if let Ok(json) = serde_json::to_vec_pretty(&dump) {
                let _ = std::fs::write(project_dir.join("papers_rejected.json"), json);
                println!("      → {}", project_dir.join("papers_rejected.json").display());
            }
        }
        if !kept.is_empty() {
            paper_texts = kept;
            // papers.json is the resume artifact — persist the filtered corpus.
            if let Ok(json) = serde_json::to_vec(&paper_texts) {
                let _ = std::fs::write(&papers_path, json);
            }
        }
        manifest.record_stage(
            "relevance_filter",
            miniagent_research::StageStatus::Completed,
            phase_start.elapsed(),
            vec![papers_path.clone()],
            Some(serde_json::json!({
                "kept": paper_texts.len(),
                "rejected": rejected.len(),
            })),
        );
        let _ = manifest.save();
    }

    // ── Phase 3: KG Extraction (resumable, parallel) ──────────────
    let phase_start = Instant::now();
    println!("\n━━━ Phase 3: Knowledge Graph Extraction ━━━");

    let kg_path = project_dir.join("kg.json");
    let mut kg = load_kg(&kg_path).filter(|_| manifest.is_stage_done("kg_extraction"));

    if let Some(ref loaded) = kg {
        println!("   ↻ resumed KG: {} entities, {} relations",
            loaded.entity_count(), loaded.relation_count());
    } else {
        // Bounded-concurrency LLM extraction (goal 1: performance): one shared
        // flash provider, several papers in flight at once.
        let concurrency = 6usize;
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut jobs = Vec::with_capacity(paper_texts.len());
        for (i, (pmid, text)) in paper_texts.iter().enumerate() {
            let flash = flash.clone();
            let sem = sem.clone();
            let cancel = cancel.child_token();
            let pmid = pmid.clone();
            let text = text.clone();
            jobs.push((
                i,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    extract_paper_entities(flash, &pmid, &text, cancel).await
                }),
            ));
        }
        let mut results: Vec<Option<miniagent_kg::extraction::ExtractionResult>> =
            (0..paper_texts.len()).map(|_| None).collect();
        for (i, job) in jobs {
            match job.await {
                Ok(Ok(extraction)) => results[i] = Some(extraction),
                Ok(Err(e)) => eprintln!("   ⚠ Paper {} extraction error: {e}", i + 1),
                Err(e) => eprintln!("   ⚠ Paper {} extraction task failed: {e}", i + 1),
            }
        }
        let mut merged = KnowledgeGraph::new();
        let mut total_merged_entities = 0usize;
        let mut total_dangling = 0usize;
        for (i, extraction) in results.into_iter().enumerate() {
            if let Some(extraction) = extraction {
                // Alias-aware canonical merge: remaps relation endpoints to
                // canonical entity ids (the old name-only merge left dangling
                // edges when duplicate entity names were skipped).
                let stats = miniagent_kg::extraction::merge_extraction_canonical(&mut merged, extraction);
                total_merged_entities += stats.entities_merged;
                total_dangling += stats.relations_dropped;
                println!(
                    "   Paper {} — {} entities (+{} merged into existing), {} relations",
                    i + 1,
                    stats.entities_added,
                    stats.entities_merged,
                    stats.relations_added
                );
            }
        }
        if total_merged_entities > 0 || total_dangling > 0 {
            println!("   (alias-merged {total_merged_entities} duplicate entities; dropped {total_dangling} unresolved relations)");
        }
        kg = Some(merged);
        // Persist the KG (ids preserved) for resume + audit (goal 1).
        if let Err(e) = save_kg(kg.as_ref().unwrap(), &kg_path) {
            eprintln!("   ⚠ failed to persist KG: {e}");
        }
    }
    let mut kg = kg.unwrap_or_else(KnowledgeGraph::new);

    println!("\n   📊 KG: {} entities, {} relations", kg.entity_count(), kg.relation_count());

    // Print KG as Mermaid
    println!("\n   ── Knowledge Graph (Mermaid) ──");
    println!("```mermaid\ngraph TD");
    for entity in kg.all_entities() {
        let etype = format!("{:?}", entity.entity_type);
        let safe_name = entity.name.replace([' ', '-'], "_");
        println!("    {safe_name}[\"{etype}\n{name}\"]", name = entity.name);
    }
    for rel in kg.all_relations().iter().take(30) {
        let from_name = kg.get_entity(&rel.from_id).map(|e| e.name.replace([' ', '-'], "_")).unwrap_or_default();
        let to_name = kg.get_entity(&rel.to_id).map(|e| e.name.replace([' ', '-'], "_")).unwrap_or_default();
        let rt = format!("{:?}", rel.relation_type);
        if !from_name.is_empty() && !to_name.is_empty() {
            println!("    {from_name} --\"{rt}\"--> {to_name}");
        }
    }
    println!("```");

    let phase3_dur = phase_start.elapsed();
    manifest.record_stage(
        "kg_extraction",
        miniagent_research::StageStatus::Completed,
        phase3_dur,
        vec![kg_path.clone()],
        Some(serde_json::json!({
            "entities": kg.entity_count(),
            "relations": kg.relation_count(),
        })),
    );
    let _ = manifest.save();

    // ── Optional: External KG Enrichment ──────────────────────────
    // Merge triples from a biomedical KG export (DisGeNET/OMIM/custom TSV)
    // to broaden link prediction beyond PubMed-extracted edges. (Goal 2)
    if let Some(path) = enrich_file {
        println!("\n━━━ KG Enrichment: {path} ━━━");
        let rel = miniagent_kg::schema::RelationType::parse(enrich_relation)
            .unwrap_or(miniagent_kg::schema::RelationType::AssociatedWith);
        let source_label = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("external");
        match miniagent_kg::external::load_fixed_relation_tsv(
            path,
            enrich_delim,
            miniagent_kg::schema::EntityType::Gene,
            rel.clone(),
            miniagent_kg::schema::EntityType::Disease,
            source_label,
        ) {
            Ok(triples) => {
                let n = triples.len();
                let stats = miniagent_kg::merge_external(&mut kg, &triples);
                println!(
                    "   loaded {n} triples ({:?}): +{} edges, +{} entities ({} duplicates skipped)",
                    rel, stats.edges_added, stats.entities_created, stats.edges_skipped_duplicate
                );
                println!("   📊 KG after enrichment: {} entities, {} relations", kg.entity_count(), kg.relation_count());
            }
            Err(e) => eprintln!("   ⚠ enrichment load failed: {e}"),
        }
    }

    if kg_only {
        let total = start.elapsed();
        println!("\n╔══ Pipeline Complete (KG only) ═══════════════════════════╗");
        println!("║ Search: {:>6.1}s  Fetch: {:>6.1}s  KG: {:>6.1}s  Total: {:>6.1}s",
            phase1_dur.as_secs_f64(), phase2_dur.as_secs_f64(),
            phase3_dur.as_secs_f64(), total.as_secs_f64());
        println!("╚════════════════════════════════════════════════════════════╝");
        manifest.log_event("pipeline_complete_kg_only", format!("total_secs={:.1}", total.as_secs_f64()));
        match manifest.save() {
            Ok(path) => println!("📁 audit manifest: {}", path.display()),
            Err(e) => println!("⚠️  failed to save project manifest: {e}"),
        }
        return;
    }

    // ── Phase 4: Embedding & Link Prediction (resumable) ──────────
    let phase_start = Instant::now();
    println!("\n━━━ Phase 4: Embedding & Link Prediction ━━━");

    let candidates_path = project_dir.join("candidates.json");
    let mut all_candidates: Vec<miniagent_kg::link_prediction::HypothesisCandidate> =
        if manifest.is_stage_done("link_prediction") {
            std::fs::read(&candidates_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    if all_candidates.is_empty() {
        let mut kge = KgeModel::new(128);
        kge.train(&kg, 200, 0.005);
        println!("   TransE 128-dim trained on {} relations", kg.relation_count());

        let scorer = LinkPredictionScorer::new().with_kge(kge);
        let mut cands = Vec::new();
        let rel_types = [RelationType::Regulates, RelationType::Inhibits, RelationType::Activates, RelationType::AssociatedWith];

        for entity in kg.all_entities() {
            for rt in &rel_types {
                let candidates = scorer.predict_tails(&entity.id, rt, &kg, 2);
                cands.extend(candidates);
            }
        }

        cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Anchor the candidate set to the queried disease. Without this, hub
        // entities from a single off-topic paper (e.g. "mortality risk") soak
        // up the top scores and every hypothesis drifts away from the disease
        // the user asked about.
        if let Some(anchor) = find_disease_anchor(&kg, query) {
            let name = kg.get_entity(&anchor).map(|e| e.name.clone()).unwrap_or_default();
            let anchored: Vec<_> = cands
                .iter()
                .filter(|c| c.head == anchor || c.tail == anchor)
                .cloned()
                .collect();
            if anchored.len() >= 3 {
                println!("   🎯 disease anchor: '{name}' — {}/{} candidates anchored", anchored.len(), cands.len());
                cands = anchored;
            } else {
                println!("   (only {} candidate(s) touch disease anchor '{name}' — keeping unfiltered set)", anchored.len());
            }
        }

        cands.truncate(15);
        all_candidates = cands;
        if let Ok(json) = serde_json::to_vec_pretty(&all_candidates) {
            let _ = std::fs::write(&candidates_path, json);
        }
    } else {
        println!("   ↻ resumed: {} candidates from {}", all_candidates.len(), candidates_path.display());
    }

    println!("   Link prediction candidates:");
    for (i, c) in all_candidates.iter().enumerate().take(10) {
        let head_name = kg.get_entity(&c.head).map(|e| e.name.as_str()).unwrap_or("?");
        let tail_name = kg.get_entity(&c.tail).map(|e| e.name.as_str()).unwrap_or("?");
        let rel_name = format!("{:?}", c.relation).to_lowercase();
        println!("   {}. {head_name} --[{rel_name}]--> {tail_name} (score: {:.3})", i + 1, c.score);
    }

    let phase4_dur = phase_start.elapsed();
    manifest.set_kg_stats(serde_json::json!({
        "entities": kg.entity_count(),
        "relations": kg.relation_count(),
    }));
    manifest.record_stage(
        "link_prediction",
        miniagent_research::StageStatus::Completed,
        phase4_dur,
        vec![],
        None,
    );

    // ── Phase 5: Hypothesis Generation (resumable, parallel) ──────
    // The KG is shared read-only across parallel generation jobs.
    let kg = std::sync::Arc::new(kg);

    let hyps_full_path = project_dir.join("hypotheses_full.json");
    let mut hypotheses: Vec<miniagent_hypothesis::Hypothesis> =
        if manifest.is_stage_done("hypothesis_generation") {
            std::fs::read(&hyps_full_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    let phase5_dur = if hypotheses.is_empty() && !all_candidates.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 5: Hypothesis Generation ━━━");

        let top_candidates: Vec<_> = all_candidates.iter().take(5).cloned().collect();
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
        let mut jobs = Vec::with_capacity(top_candidates.len());
        for (i, candidate) in top_candidates.into_iter().enumerate() {
            let kg = kg.clone();
            let sem = sem.clone();
            let cfg = config.clone();
            let cancel = cancel.child_token();
            let task_candidate = candidate.clone();
            jobs.push((
                i,
                candidate,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    // Each job gets its own provider handle (cheap); the
                    // generator API takes ownership.
                    let generator = HypothesisGenerator::new()
                        .with_provider(make_providers(&cfg).1);
                    generator.generate(&task_candidate, &kg, cancel).await
                }),
            ));
        }
        let mut results: Vec<Option<miniagent_hypothesis::Hypothesis>> =
            vec![None; jobs.len()];
        for (i, candidate, job) in jobs {
            let head_name = kg.get_entity(&candidate.head).map(|e| e.name.as_str()).unwrap_or("?");
            let tail_name = kg.get_entity(&candidate.tail).map(|e| e.name.as_str()).unwrap_or("?");
            print!("   {}. {head_name} → {tail_name} ... ", i + 1);
            std::io::Write::flush(&mut std::io::stdout()).ok();
            match job.await {
                Ok(Ok(h)) => {
                    println!("✅ ({:.2})", h.confidence);
                    results[i] = Some(h);
                }
                Ok(Err(e)) => println!("❌ {e}"),
                Err(e) => println!("❌ task failed: {e}"),
            }
        }
        hypotheses = results.into_iter().flatten().collect();
        if let Ok(json) = serde_json::to_vec_pretty(&hypotheses) {
            let _ = std::fs::write(&hyps_full_path, json);
        }
        phase_start.elapsed()
    } else if !hypotheses.is_empty() {
        println!("\n━━━ Phase 5: ↻ resumed — {} hypotheses ━━━", hypotheses.len());
        std::time::Duration::default()
    } else {
        eprintln!("\n━━━ Phase 5: skipped (no candidates) ━━━");
        std::time::Duration::default()
    };
    manifest.record_stage(
        "hypothesis_generation",
        miniagent_research::StageStatus::Completed,
        phase5_dur,
        vec![hyps_full_path.clone()],
        Some(serde_json::json!({ "count": hypotheses.len() })),
    );
    let _ = manifest.save();

    // ── Phase 6: Ranking ──────────────────────────────────────────
    println!("\n━━━ Phase 6: Hypothesis Ranking ━━━");

    let mut ranked = HypothesisRanker::rank(&hypotheses);
    if ranked.is_empty() {
        println!("   No hypotheses generated. Try a different query or increase max_papers.");
    } else {
        for (i, rh) in ranked.iter().enumerate() {
            let h = &rh.hypothesis;
            let head_name = kg.get_entity(&h.source_candidate.head)
                .map(|e| e.name.as_str()).unwrap_or("?");
            let tail_name = kg.get_entity(&h.source_candidate.tail)
                .map(|e| e.name.as_str()).unwrap_or("?");

            println!("\n🏆 Rank #{} ({:.3}) — {head_name} ⟶ {tail_name}",
                i + 1, rh.composite_score);
            println!("   Hypothesis: {}", h.statement);
            if let Some(mech) = &h.mechanism {
                println!("   Mechanism: {}", mech);
            }
            println!("   Novelty: {:?} | Confidence: {:.2}", h.novelty, h.confidence);
            if let Some(exp) = &h.experimental_design {
                println!("   Experiment: {}", exp.approach);
                println!("   Methods: {}", exp.methods.join(", "));
                println!("   Feasibility: {:.2}", exp.feasibility);
            }
            if !h.counter_evidence.is_empty() {
                println!("   ⚠️  Counter: {}", h.counter_evidence.first().unwrap());
            }
        }
    }

    // Persist the ranked hypotheses for the audit trail.
    {
        let hyp_path = project_dir.join("hypotheses.json");
        let hyp_refs: Vec<miniagent_research::HypothesisRef> = ranked
            .iter()
            .map(|rh| {
                miniagent_research::HypothesisRef::new(
                    rh.hypothesis.id,
                    rh.hypothesis.statement.clone(),
                    Some(hyp_path.clone()),
                )
            })
            .collect();
        if let Ok(json) = serde_json::to_string_pretty(&ranked.iter().map(|rh| {
            let head = kg.get_entity(&rh.hypothesis.source_candidate.head).map(|e| e.name.clone()).unwrap_or_default();
            let tail = kg.get_entity(&rh.hypothesis.source_candidate.tail).map(|e| e.name.clone()).unwrap_or_default();
            serde_json::json!({
                "id": rh.hypothesis.id,
                "rank_score": rh.composite_score,
                "statement": rh.hypothesis.statement,
                "mechanism": rh.hypothesis.mechanism,
                "head": head,
                "tail": tail,
                "confidence": rh.hypothesis.confidence,
                "novelty": format!("{:?}", rh.hypothesis.novelty),
            })
        }).collect::<Vec<_>>()) {
            let _ = std::fs::write(&hyp_path, json);
        }
        manifest.record_hypotheses(hyp_refs);
    }
    manifest.record_stage(
        "ranking",
        miniagent_research::StageStatus::Completed,
        std::time::Duration::default(),
        vec![project_dir.join("hypotheses.json")],
        Some(serde_json::json!({ "count": ranked.len() })),
    );

    // ── Phase 6b: Hypothesis Debate · Compare · Refine ─────────────
    // Stress-test each hypothesis on evidence vs. contradiction, cross-compare
    // them, and refine the weak ones (goal 2). Drives validation planning.
    let mut debate_ok = false;
    let phase6b_dur = if debate && !ranked.is_empty() && !manifest.is_stage_done("debate") {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 6b: Hypothesis Debate · Compare · Refine ━━━");

        // Retrieve external literature evidence per hypothesis via web search
        // (goal 2: the debate must argue from retrieved literature, not just
        // parametric memory). Evidence is persisted for the audit trail.
        let ranked_hyps: Vec<miniagent_hypothesis::Hypothesis> =
            ranked.iter().map(|rh| rh.hypothesis.clone()).collect();
        let evidence = retrieve_debate_evidence(
            &ranked_hyps,
            std::cmp::min(4, ranked_hyps.len()),
            cancel.child_token(),
        )
        .await;
        if !evidence.is_empty() {
            let evidence_path = project_dir.join("debate_evidence.json");
            let dump: Vec<serde_json::Value> = evidence
                .iter()
                .map(|(id, query, body)| {
                    serde_json::json!({"hypothesis_id": id.to_string(), "query": query, "results": body})
                })
                .collect();
            if let Ok(json) = serde_json::to_vec_pretty(&dump) {
                let _ = std::fs::write(&evidence_path, json);
                println!("   🔎 web evidence for {} hypotheses → {}", evidence.len(), evidence_path.display());
                manifest.log_event("debate_evidence_retrieved", format!("count={}", evidence.len()));
            }
        }
        let evidence_map: std::collections::HashMap<uuid::Uuid, String> = evidence
            .into_iter()
            .map(|(id, query, body)| (id, format!("Search query: {query}\n{body}")))
            .collect();

        let debater = miniagent_hypothesis::HypothesisDebater::new(make_providers(config).1);
        match debater
            .debate_and_refine_with_evidence(&ranked_hyps, &kg, &evidence_map, cancel.child_token())
            .await
        {
            Ok(outcome) => {
                debate_ok = true;
                for v in &outcome.per_hypothesis {
                    println!(
                        "   {} → {:?} (confidence {:.2})",
                        short_id(&v.hypothesis_id.to_string()),
                        v.verdict,
                        v.confidence_after
                    );
                    if let Some(c) = v.contradicting_points.first() {
                        println!("      ⚠️  {}", c);
                    }
                    if let Some(s) = v.supporting_points.first() {
                        println!("      ✅ {}", s);
                    }
                }
                if let Some(id) = outcome.comparison.strongest_id {
                    println!("   🥇 strongest hypothesis: {}", short_id(&id.to_string()));
                }
                for cp in &outcome.comparison.contradictions_between {
                    println!(
                        "   ⚡ {} ⇄ {}: {}",
                        short_id(&cp.a.to_string()),
                        short_id(&cp.b.to_string()),
                        cp.reason
                    );
                }
                for ms in &outcome.comparison.merge_suggestions {
                    println!("   💡 merge: {}", ms);
                }

                // Persist the debate report into the project dir for auditing.
                let debate_path = project_dir.join("debate_report.json");
                match miniagent_hypothesis::persist_debate_report(&outcome, &kg, &debate_path) {
                    Ok(()) => {
                        println!("      → {}", debate_path.display());
                        manifest.record_debate(&debate_path);
                    }
                    Err(e) => println!("   ⚠️  debate report write failed: {e}"),
                }

                // Re-rank the refined set and shadow `ranked` so downstream
                // phases (validation, analysis) operate on the refined hypotheses.
                if !outcome.refined.is_empty() {
                    ranked = HypothesisRanker::rank(&outcome.refined);
                    let refined_path = project_dir.join("hypotheses_refined.json");
                    let _ = std::fs::write(
                        &refined_path,
                        serde_json::to_string_pretty(&ranked.iter().map(|rh| rh.hypothesis.id.to_string()).collect::<Vec<_>>()).unwrap_or_default(),
                    );
                    // Full-fidelity copy for resume (debate already ran).
                    let refined_full = project_dir.join("hypotheses_refined_full.json");
                    if let Ok(json) = serde_json::to_vec_pretty(&outcome.refined) {
                        let _ = std::fs::write(&refined_full, json);
                    }
                    let hyp_refs: Vec<miniagent_research::HypothesisRef> = ranked
                        .iter()
                        .map(|rh| {
                            miniagent_research::HypothesisRef::new(
                                rh.hypothesis.id,
                                rh.hypothesis.statement.clone(),
                                Some(refined_path.clone()),
                            )
                            .with_refined(true)
                        })
                        .collect();
                    manifest.record_hypotheses(hyp_refs);
                    println!("   → {} refined hypothesis/hypotheses", outcome.refined.len());
                }
            }
            Err(e) => {
                println!("❌ debate failed: {e} (continuing with the ranked set)");
                manifest.log_event("debate_failed", e.to_string());
            }
        }
        let _ = manifest.save();
        phase_start.elapsed()
    } else if debate && manifest.is_stage_done("debate") {
        // Resume: reload the refined hypothesis set persisted by a previous run.
        let refined_full = project_dir.join("hypotheses_refined_full.json");
        if let Some(hs) = std::fs::read(&refined_full)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<miniagent_hypothesis::Hypothesis>>(&b).ok())
            .filter(|v| !v.is_empty())
        {
            ranked = HypothesisRanker::rank(&hs);
            println!("\n━━━ Phase 6b: ↻ resumed — {} refined hypotheses ━━━", ranked.len());
        }
        std::time::Duration::default()
    } else {
        std::time::Duration::default()
    };
    // A failed debate must NOT be recorded as Completed — otherwise resume
    // skips re-running it and the refined-hypothesis set is lost forever.
    manifest.record_stage(
        "debate",
        if debate && ranked.is_empty() {
            miniagent_research::StageStatus::Skipped
        } else if debate_ok {
            miniagent_research::StageStatus::Completed
        } else if debate {
            miniagent_research::StageStatus::Failed
        } else {
            miniagent_research::StageStatus::Skipped
        },
        phase6b_dur,
        vec![],
        None,
    );

    // ── Phase 7: Validation Planning (resumable, parallel) ────────
    // Generate structured validation plans (data-analysis tasks + wet-lab
    // protocols) for the top-N hypotheses. (Goal 3) Plans are grounded with
    // real GEO dataset accessions and persisted inside the project dir.
    let mut validation_plans: Vec<miniagent_hypothesis::ValidationPlan> = Vec::new();
    let plans_dir = project_dir.join("plans");
    let phase7_dur = if validate && !ranked.is_empty() {
        if manifest.is_stage_done("validation") && !manifest.validation_plans.is_empty() {
            for path in &manifest.validation_plans {
                if let Some(plan) = std::fs::read(path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                {
                    validation_plans.push(plan);
                }
            }
            println!("\n━━━ Phase 7: ↻ resumed — {} validation plan(s) ━━━", validation_plans.len());
            std::time::Duration::default()
        } else {
            let phase_start = Instant::now();
            println!("\n━━━ Phase 7: Validation Planning (top {top_n}) ━━━");
            let _ = std::fs::create_dir_all(&plans_dir);

            let top: Vec<_> = ranked.iter().take(top_n).map(|rh| rh.hypothesis.clone()).collect();
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
            let mut jobs = Vec::with_capacity(top.len());
            for (i, h) in top.into_iter().enumerate() {
                let kg = kg.clone();
                let sem = sem.clone();
                let cfg = config.clone();
                let cancel = cancel.child_token();
                let task_h = h.clone();
                jobs.push((
                    i,
                    h,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    // Validation plans are long, schema-heavy JSON; reasoning
                    // models (pro) burn the budget on CoT and emit truncated
                    // or empty JSON, so use the flash chat model with one retry.
                    let mut last_err = None;
                    for attempt in 0..2 {
                        let generator = HypothesisGenerator::new()
                            .with_provider(make_providers(&cfg).0);
                        match generator.generate_validation_plan(&task_h, &kg, cancel.clone()).await {
                            Ok(plan) => return Ok(plan),
                            Err(e) => {
                                eprintln!("[plan attempt {} failed: {e}]", attempt + 1);
                                last_err = Some(e);
                            }
                        }
                    }
                    Err(last_err.expect("at least one attempt"))
                }),
                ));
            }

            let mut plans: Vec<(usize, miniagent_hypothesis::Hypothesis, miniagent_hypothesis::ValidationPlan)> =
                Vec::new();
            for (i, h, job) in jobs {
                let head_name = kg.get_entity(&h.source_candidate.head).map(|e| e.name.as_str()).unwrap_or("?");
                let tail_name = kg.get_entity(&h.source_candidate.tail).map(|e| e.name.as_str()).unwrap_or("?");
                print!("   #{}. {head_name} → {tail_name} validation plan ... ", i + 1);
                std::io::Write::flush(&mut std::io::stdout()).ok();
                match job.await {
                    Ok(Ok(plan)) => {
                        let n_da = plan.data_analysis_tasks.len();
                        let n_wl = plan.wet_lab_protocols.len();
                        println!("✅ {n_da} data-analysis task(s), {n_wl} wet-lab protocol(s)");
                        plans.push((i, h, plan));
                    }
                    Ok(Err(e)) => println!("❌ {e}"),
                    Err(e) => println!("❌ task failed: {e}"),
                }
            }

            // Ground the plans with real datasets: for GEO tasks whose
            // accession the LLM left empty, search NCBI GEO and backfill a
            // concrete accession (goal 3: executable validation plans).
            for (_, h, plan) in plans.iter_mut() {
                let grounded = ground_plan_datasets(plan, &h.statement, cancel.child_token()).await;
                for g in &grounded {
                    println!("      🧬 grounded: {g}");
                    manifest.log_event("dataset_grounded", g.clone());
                }
            }

            // Persist the plans inside the auditable project dir. Drop any
            // stale plan paths from previous runs first — the files are
            // rewritten with the same indices, so keeping the old entries
            // would execute each plan twice on resume.
            manifest.validation_plans.clear();
            for (i, _, plan) in plans {
                let plan_path = plans_dir.join(format!("validation_plan_{i}.json"));
                if let Ok(json) = serde_json::to_string_pretty(&plan) {
                    let _ = std::fs::write(&plan_path, json);
                    println!("      → {}", plan_path.display());
                    manifest.add_validation_plan(&plan_path);
                }
                validation_plans.push(plan);
            }
            let _ = manifest.save();
            phase_start.elapsed()
        }
    } else {
        std::time::Duration::default()
    };
    manifest.record_stage(
        "validation",
        if validate && !ranked.is_empty() {
            miniagent_research::StageStatus::Completed
        } else {
            miniagent_research::StageStatus::Skipped
        },
        phase7_dur,
        manifest.validation_plans.clone(),
        Some(serde_json::json!({ "plans": validation_plans.len() })),
    );

    // ── Phase 8: Data Analysis Execution (resumable) ──────────────
    // Execute each data-analysis task end-to-end with full provenance. (Goal 4)
    // Artifacts (script/notebook/provenance) land inside the project dir.
    let phase8_dur = if analyze && !validation_plans.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 8: Data Analysis Execution ━━━");

        // Script generation is long-form code; reasoning models (pro) can
        // return empty content, so use the flash chat model here too.
        let runner = miniagent_analysis::AnalysisRunner::new(make_providers(config).0);
        // Absolute project dir: the runner executes with different CWDs
        // (jupyter inherits the process CWD, scripts run with
        // current_dir(working_dir)), so only absolute paths are unambiguous.
        let project_abs = std::fs::canonicalize(&project_dir).unwrap_or_else(|_| project_dir.clone());
        let mut opts = miniagent_analysis::RunOpts::default();
        if let Some(d) = data {
            let p = std::path::PathBuf::from(d);
            opts.local_data = Some(if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) });
            println!("   local data: {}", opts.local_data.as_ref().unwrap().display());
        } else {
            println!("   (no --data: tasks without local data run as dry-runs)");
        }

        // Resume: skip tasks that already succeeded in a previous run.
        let done: std::collections::HashSet<(uuid::Uuid, String)> = manifest
            .analyses
            .iter()
            .filter(|a| a.success)
            .filter_map(|a| a.hypothesis_id.map(|h| (h, a.task_id.clone())))
            .collect();

        for (plan_idx, plan) in validation_plans.iter().enumerate() {
            let work_dir = project_abs.join("analysis").join(format!("plan_{plan_idx}"));
            for task in &plan.data_analysis_tasks {
                if done.contains(&(plan.hypothesis_id, task.id.clone())) {
                    println!("   ↻ {} [{}] already succeeded — skipping", task.id, short_id(&plan.hypothesis_id.to_string()));
                    continue;
                }
                print!("   ▶ {} [{}] ... ", task.id, task.statistical_method);
                std::io::Write::flush(&mut std::io::stdout()).ok();
                match runner
                    .run(task, &work_dir, Some(plan.hypothesis_id), &opts, cancel.child_token())
                    .await
                {
                    Ok(res) => {
                        if res.dry_run {
                            println!("📝 dry-run (script + notebook generated)");
                        } else if res.success {
                            println!("✅ {} output file(s) [{:?}]", res.output_files.len(), res.execution_backend);
                        } else {
                            println!("⚠️  {}", res.error.unwrap_or_default());
                        }
                        println!("      notebook: {} (executed: {})", res.notebook_path.display(), res.notebook_executed);
                        if let Some(p) = res.provenance_path.as_ref() {
                            println!("      provenance: {}", p.display());
                        }
                        // Unified audit manifest + structured trace log.
                        manifest.record_analysis(miniagent_research::AnalysisRef {
                            task_id: res.task_id.clone(),
                            hypothesis_id: Some(plan.hypothesis_id),
                            notebook_path: Some(res.notebook_path.clone()),
                            provenance_path: res.provenance_path.clone(),
                            success: res.success,
                            execution_backend: format!("{:?}", res.execution_backend).to_lowercase(),
                        });
                        tracing::info!(
                            target: "tool_call",
                            task_id = %res.task_id,
                            success = res.success,
                            dry_run = res.dry_run,
                            backend = ?res.execution_backend,
                            notebook_executed = res.notebook_executed,
                            script_hash = %res.provenance.script_hash,
                            conda_used = res.provenance.conda_used,
                            exit_code = ?res.provenance.exit_code,
                            provenance_path = %res.provenance_path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                            "analysis task executed",
                        );
                    }
                    Err(e) => println!("❌ {e}"),
                }
            }
        }
        let _ = manifest.save();
        phase_start.elapsed()
    } else {
        std::time::Duration::default()
    };
    manifest.record_stage(
        "analysis",
        if analyze && !validation_plans.is_empty() {
            miniagent_research::StageStatus::Completed
        } else {
            miniagent_research::StageStatus::Skipped
        },
        phase8_dur,
        vec![],
        Some(serde_json::json!({ "analyses": manifest.analyses.len() })),
    );

    let total = start.elapsed();
    println!("\n╔══ Pipeline Complete ═════════════════════════════════════╗");
    println!("║ Phase 1 (Search PubMed):  {:>8.1}s", phase1_dur.as_secs_f64());
    println!("║ Phase 2 (Fetch Abstracts):{:>8.1}s", phase2_dur.as_secs_f64());
    println!("║ Phase 3 (KG Extraction):  {:>8.1}s", phase3_dur.as_secs_f64());
    println!("║ Phase 4 (Link Prediction):{:>8.1}s", phase4_dur.as_secs_f64());
    println!("║ Phase 5 (Hypothesis Gen): {:>8.1}s", phase5_dur.as_secs_f64());
    println!("║ Phase 6b (Debate):        {:>8.1}s", phase6b_dur.as_secs_f64());
    if validate {
        println!("║ Phase 7 (Validation Plan):{:>8.1}s", phase7_dur.as_secs_f64());
    }
    if analyze {
        println!("║ Phase 8 (Data Analysis):  {:>8.1}s", phase8_dur.as_secs_f64());
    }
    println!("║ Total:                    {:>8.1}s", total.as_secs_f64());
    println!("║ KG: {} entities, {} relations", kg.entity_count(), kg.relation_count());
    println!("║ Hypotheses: {}", hypotheses.len());
    if validate || analyze {
        println!("║ Validation plans: {}", validation_plans.len());
    }
    println!("╚══════════════════════════════════════════════════════════╝");

    // Persist the unified, auditable project manifest.
    manifest.log_event("pipeline_complete", format!("total_secs={:.1}", total.as_secs_f64()));
    match manifest.save() {
        Ok(path) => println!("\n📁 audit manifest: {}", path.display()),
        Err(e) => println!("\n⚠️  failed to save project manifest: {e}"),
    }
}

// ── Research pipeline helpers ─────────────────────────────────

/// Extract entities/relations from one paper abstract via the shared flash
/// provider. Runs inside a parallel task (goal 1: performance).
async fn extract_paper_entities(
    flash: std::sync::Arc<dyn LlmProvider>,
    pmid: &str,
    text: &str,
    cancel: CancellationToken,
) -> Result<miniagent_kg::extraction::ExtractionResult, miniagent_core::error::AgentError> {
    use miniagent_kg::extraction::parse_extraction_result;

    let prompt = format!(
        r#"Extract key entities and their relationships from this scientific paper abstract.

**Paper ID:** PMID:{pmid}
**Content:** {text}

Output a JSON object with:
1. "entities": list of objects with "name" (canonical name), "type" (one of: Gene, Protein, Pathway, Disease, Phenotype, Drug, Method, Concept), "aliases" (alternative names)
2. "relations": list of objects with "from" (entity name), "to" (entity name), "type" (one of: activates, inhibits, regulates, binds_to, interacts_with, associated_with, correlated_with, uses_method, measured_by, is_a, part_of, supports, contradicts, extends), "evidence" (supporting quote)

Focus on biologically/scientifically meaningful entities. Output ONLY valid JSON."#
    );

    let request = miniagent_provider::traits::CompletionRequest {
        system: "You extract structured scientific entities and relationships. Output ONLY valid JSON.".into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.1), max_tokens: Some(2000), ..Default::default()
        },
    };

    let resp = flash.complete(&request, cancel).await?;
    let response_text = resp.content.iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("");
    let json_str = response_text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```");
    let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap_or_default();
    Ok(parse_extraction_result(uuid::Uuid::new_v4(), &parsed))
}

/// Serialize a KG (ids preserved) for resume + audit.
fn save_kg(kg: &miniagent_kg::KnowledgeGraph, path: &std::path::Path) -> std::io::Result<()> {
    let dump = serde_json::json!({
        "entities": kg.all_entities().collect::<Vec<_>>(),
        "relations": kg.all_relations(),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&dump)?)
}

/// Rebuild a KG from a `save_kg` dump. Entity/relation ids are preserved, so
/// cached link-prediction candidates stay valid.
fn load_kg(path: &std::path::Path) -> Option<miniagent_kg::KnowledgeGraph> {
    #[derive(serde::Deserialize)]
    struct Dump {
        entities: Vec<miniagent_kg::schema::Entity>,
        relations: Vec<miniagent_kg::schema::Relation>,
    }
    let dump: Dump = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut kg = miniagent_kg::KnowledgeGraph::new();
    for e in dump.entities {
        kg.add_entity(e);
    }
    for r in dump.relations {
        kg.add_relation(r);
    }
    Some(kg)
}

/// Backfill real GEO dataset accessions for data-analysis tasks whose source
/// is GEO but whose accession the LLM left empty (goal 3: executable plans).
/// Returns human-readable `task → accession` lines for the ones grounded.
async fn ground_plan_datasets(
    plan: &mut miniagent_hypothesis::ValidationPlan,
    hypothesis_statement: &str,
    cancel: CancellationToken,
) -> Vec<String> {
    use miniagent_hypothesis::validation::DatasetSource;
    use miniagent_tool::tools::GeoSearchTool;
    use miniagent_tool::traits::Tool;

    let mut grounded = Vec::new();
    for task in &mut plan.data_analysis_tasks {
        if task.dataset_accession.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            continue; // already concrete
        }
        if !matches!(task.dataset_source, DatasetSource::Geo) {
            continue; // only GEO can be grounded via the GEO search API
        }
        let query = geo_query_from_parts(&task.objective, hypothesis_statement);
        let tool = GeoSearchTool::new();
        let ctx = miniagent_tool::traits::ToolContext::new(
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
            "geo_grounding".to_string(),
        );
        let Ok(out) = tool
            .execute(
                serde_json::json!({ "query": query, "max_results": 3 }),
                &ctx,
                cancel.child_token(),
            )
            .await
        else {
            continue;
        };
        if let Some(acc) = first_geo_accession(&out.content) {
            task.dataset_accession = Some(acc.clone());
            grounded.push(format!("{} → {} ({})", task.id, acc, query));
        }
    }
    grounded
}

/// Build a compact English GEO query from a task objective (primary signal)
/// plus the hypothesis statement (disease/gene context).
fn geo_query_from_parts(objective: &str, hypothesis: &str) -> String {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "of", "in", "on", "for", "to", "and", "or", "with", "by",
        "from", "as", "is", "are", "be", "this", "that", "using", "used", "use",
        "between", "across", "whether", "test", "test;", "analysis", "analyze",
    ];
    let mut words: Vec<String> = Vec::new();
    for src in [objective, hypothesis] {
        for w in src.split(|c: char| !c.is_ascii_alphanumeric()) {
            let w = w.trim().to_lowercase();
            if w.len() < 3 || STOPWORDS.contains(&w.as_str()) || words.iter().any(|e| *e == w) {
                continue;
            }
            words.push(w);
            if words.len() >= 10 {
                break;
            }
        }
        if words.len() >= 10 {
            break;
        }
    }
    if words.is_empty() {
        "Homo sapiens expression profiling".to_string()
    } else {
        words.join(" ")
    }
}

/// Pull the first `GSE…` accession out of a `geo_search` result listing
/// (lines are formatted as `N. **GSE12345** — Title`).
fn first_geo_accession(content: &str) -> Option<String> {
    let idx = content.find("**GSE")? + "**".len();
    let acc: String = content[idx..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if acc.starts_with("GSE") && acc.len() > "GSE".len() {
        Some(acc)
    } else {
        None
    }
}

/// Score each paper's relevance to the research query (0-10) with the cheap
/// flash model and keep papers scoring >= 5. Returns (kept, rejected-with-reason).
/// Fails open (keeps the paper) when the LLM call errors.
async fn filter_irrelevant_papers(
    flash: std::sync::Arc<dyn LlmProvider>,
    query: &str,
    papers: &[(String, String)],
    cancel: CancellationToken,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let concurrency = 6usize;
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut jobs = Vec::with_capacity(papers.len());
    for (pmid, text) in papers {
        let flash = flash.clone();
        let sem = sem.clone();
        let cancel = cancel.child_token();
        let pmid = pmid.clone();
        let text = text.clone();
        let query = query.to_string();
        jobs.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            // Relevance is decidable from the head of the abstract.
            let snippet: String = text.chars().take(1200).collect();
            let prompt = format!(
                "Research query: {query}\n\nPaper abstract (PMID {pmid}):\n{snippet}\n\n\
                 Is this paper on-topic for the research query? Output ONLY JSON: \
                 {{\"score\": <integer 0-10>, \"reason\": \"<one short sentence>\"}}"
            );
            let request = miniagent_provider::traits::CompletionRequest {
                system: "You judge literature relevance. Output ONLY valid JSON.".into(),
                messages: vec![miniagent_core::message::Message::user(&prompt)],
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.0),
                    max_tokens: Some(120),
                    ..Default::default()
                },
            };
            let mut score = 10i64; // fail-open
            let mut reason = String::new();
            if let Ok(resp) = flash.complete(&request, cancel).await {
                let t: String = resp.content.iter()
                    .filter_map(|b| match b {
                        miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let cleaned = t.trim()
                    .trim_start_matches("```json").trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(cleaned) {
                    score = v.get("score").and_then(|s| s.as_i64()).unwrap_or(10);
                    reason = v.get("reason").and_then(|s| s.as_str()).unwrap_or("").to_string();
                }
            }
            (pmid, text, score, reason)
        }));
    }
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    for job in jobs {
        if let Ok((pmid, text, score, reason)) = job.await {
            if score >= 5 {
                kept.push((pmid, text));
            } else {
                rejected.push((pmid, reason));
            }
        }
    }
    (kept, rejected)
}

/// Find the KG entity that best represents the queried disease: among
/// Disease-type entities, the one whose name/alias tokens overlap the query
/// tokens the most. Used to anchor link-prediction candidates.
fn find_disease_anchor(
    kg: &miniagent_kg::KnowledgeGraph,
    query: &str,
) -> Option<miniagent_kg::schema::EntityId> {
    let q_tokens: std::collections::HashSet<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .map(str::to_string)
        .collect();
    if q_tokens.is_empty() {
        return None;
    }
    let mut best: Option<(miniagent_kg::schema::EntityId, usize)> = None;
    for e in kg.all_entities() {
        if e.entity_type != miniagent_kg::schema::EntityType::Disease {
            continue;
        }
        let mut tokens: Vec<String> = e
            .name
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 3)
            .map(str::to_string)
            .collect();
        for alias in &e.aliases {
            tokens.extend(
                alias
                    .to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|t| t.len() > 3)
                    .map(str::to_string),
            );
        }
        let overlap = tokens.iter().filter(|t| q_tokens.contains(*t)).count();
        if overlap > 0 && best.as_ref().is_none_or(|(_, b)| overlap > *b) {
            best = Some((e.id, overlap));
        }
    }
    best.map(|(id, _)| id)
}

/// Retrieve web-search evidence for the top hypotheses ahead of the debate.
/// Returns (hypothesis_id, query, truncated results markdown) triples; the
/// search backend chain (Serper → Tavily → … → AnySearch → DDG) is used
/// automatically. Failures degrade to an empty set (debate proceeds without
/// external evidence).
async fn retrieve_debate_evidence(
    hypotheses: &[miniagent_hypothesis::Hypothesis],
    max_hypotheses: usize,
    cancel: CancellationToken,
) -> Vec<(uuid::Uuid, String, String)> {
    use miniagent_tool::tools::WebSearchTool;
    use miniagent_tool::traits::{Tool, ToolContext};

    let tool = WebSearchTool::new();
    let ctx = ToolContext::new(
        std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        "debate_evidence".to_string(),
    );
    let mut out = Vec::new();
    for h in hypotheses.iter().take(max_hypotheses) {
        let query = geo_query_from_parts(&h.statement, "");
        let Ok(res) = tool
            .execute(
                serde_json::json!({"query": query, "num": 5}),
                &ctx,
                cancel.child_token(),
            )
            .await
        else {
            continue;
        };
        let body: String = res.content.chars().take(4000).collect();
        if !body.trim().is_empty() {
            out.push((h.id, query, body));
        }
    }
    out
}

/// Shorten a hex/uuid-ish string for compact CLI display (keeps the head).
fn short_id(s: &str) -> String {
    let len = s.len();
    if len <= 8 {
        s.to_string()
    } else {
        s.chars().take(8).collect()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}



fn has_non_english(s: &str) -> bool {
    s.chars().any(|c| c as u32 > 0x007F)
}

// ── Plan command ──────────────────────────────────────────────

async fn plan_command(query: &str, config: &Arc<AppConfig>) {
    use miniagent_agent::Agent;
    use miniagent_core::orchestration::{StageInput, StageDriver as _};
    use miniagent_planning::runners::PlanRunner;
    use miniagent_tool::tools;
    use miniagent_tool::approval::AutoApprove;
    use miniagent_tool::executor::ToolExecutor;
    use std::sync::Arc;

    let key = match config.require_active_key() {
        Ok(k) => k.clone(),
        Err(e) => { eprintln!("{e}"); return; }
    };

    // Build agent with tools（根据 PROVIDER 配置选择 provider）
    let (flash, pro) = make_providers(config);
    let tool_registry = tools::defaults();
    let executor = ToolExecutor::new(tool_registry, Box::new(AutoApprove));
    let agent = Arc::new(Agent::new(flash, pro).with_tools(executor).with_config(config.clone()));

    println!("🧠 Planning: decomposing + executing task via PlanRunner...\n");
    let cancel = tokio_util::sync::CancellationToken::new();

    // Planner uses a dedicated flash provider (independent of the Agent's).
    let planner_flash: Box<dyn miniagent_provider::traits::LlmProvider> = if config.is_stepfun() {
        Box::new(StepFunFlash::new(&key))
    } else {
        Box::new(DeepSeekFlash::new(&key))
    };
    let runner = PlanRunner::new(planner_flash, agent);

    let input = StageInput::new("plan", serde_json::json!(query), cancel.clone());
    match runner.run(input).await {
        Ok(outcome) => {
            println!("{}", outcome.summary);
            // Pretty-print the resulting plan (status_display reads from
            // serde_json::to_value's reconstructed plan when the Plan type
            // is JSON-compatible). We just print data.summary + side_effects.
            if let Some(plan) = outcome.data.as_object() {
                if let Some(goal) = plan.get("goal").and_then(|v| v.as_str()) {
                    println!("\nGoal: {goal}");
                }
                if let Some(steps) = plan.get("steps").and_then(|v| v.as_array()) {
                    for (i, s) in steps.iter().enumerate() {
                        let desc = s.get("description").and_then(|v| v.as_str()).unwrap_or("?");
                        let status = s.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                        let icon = match status {
                            "Completed" => "✅",
                            "Running" => "🔄",
                            "Failed" => "❌",
                            "Skipped" => "⏭️",
                            _ => "⏳",
                        };
                        println!("  {icon} [{}/{}] {desc}", i + 1, steps.len());
                    }
                }
            }
            println!("\n✅ Plan execution complete (PlanRunner: {}).", runner.name());
        }
        Err(e) => eprintln!("❌ Plan execution failed: {e}"),
    }
}

// ── Orchestrate command ───────────────────────────────────────

async fn orchestrate_command(query: &str, pattern: &str, config: &Arc<AppConfig>) {
    use miniagent_agent::Agent;
    use miniagent_tool::tools;
    use miniagent_tool::approval::AutoApprove;
    use miniagent_tool::executor::ToolExecutor;
    use miniagent_workflow::stage::{StageContext, StageHandler as _};
    use miniagent_workflow::stages::OrchestratorStage;
    use tokio_util::sync::CancellationToken;
    use std::sync::Arc;
    use std::collections::HashMap;

    let key = match config.require_active_key() {
        Ok(k) => k.clone(),
        Err(e) => { eprintln!("{e}"); return; }
    };

    // Build agent with tools（根据 PROVIDER 配置选择 provider）
    let (flash, pro) = make_providers(config);
    let tool_registry = tools::defaults();
    let executor = ToolExecutor::new(tool_registry, Box::new(AutoApprove));
    let agent = Arc::new(Agent::new(flash, pro).with_tools(executor).with_config(config.clone()));

    // Decompose task into sub-tasks using Planner/LLM
    eprintln!("🏗️  Orchestrator: analyzing task with pattern '{pattern}'...");

    let sub_tasks = match pattern {
        "chain" | "parallel" => {
            // Use LLM to decompose
            use miniagent_core::message::Message;
            use miniagent_core::config::InferenceConfig;
            use miniagent_provider::traits::{CompletionRequest, LlmProvider};

            let flash_provider: Box<dyn LlmProvider> = if config.is_stepfun() {
                Box::new(StepFunFlash::new(&key))
            } else {
                Box::new(DeepSeekFlash::new(&key))
            };
            let decompose_prompt = format!(
                r#"Decompose this task into 3-5 independent sub-tasks that can be worked on in parallel.
Each sub-task should be self-contained and researchable with web search tools.
Output a JSON array of strings. Example:
["Research topic A and its mechanisms", "Investigate topic B and key findings", "Analyze relationship between A and B"]

Task: {query}

Output ONLY the JSON array, no markdown."#
            );

            let request = CompletionRequest {
                system: "You are a task decomposer. Break complex tasks into independent parallel sub-tasks.".into(),
                messages: vec![Message::user(&decompose_prompt)],
                tools: vec![],
                config: InferenceConfig { temperature: Some(0.1), max_tokens: Some(1000), ..Default::default() },
            };
            let cancel = CancellationToken::new();
            let result = flash_provider.complete(&request, cancel).await;
            match result {
                Ok(resp) => {
                    let text: String = resp.content.iter()
                        .filter_map(|b| match b { miniagent_core::event::ContentBlock::Text { text } => Some(text.clone()), _ => None })
                        .collect::<Vec<_>>().join("");
                    let cleaned = text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```");
                    serde_json::from_str::<Vec<String>>(cleaned).unwrap_or_else(|_| vec![query.to_string()])
                }
                Err(e) => {
                    eprintln!("   ⚠️ Task decomposition failed: {e}");
                    vec![query.to_string()]
                }
            }
        }
        _ => vec![query.to_string()],
    };

    eprintln!("   Decomposed into {} sub-tasks:", sub_tasks.len());
    for (i, t) in sub_tasks.iter().enumerate() {
        eprintln!("     {}. {:.80}", i + 1, t);
    }

    // Run the OrchestratorStage
    let orchestrator = OrchestratorStage::new(agent.clone());
    let ctx = StageContext::new(
        miniagent_core::types::StageId::new(),
        serde_json::json!({
            "prompt": query,
            "system": "You are an AI agent with direct access to system tools. Use tools for actions — NEVER simulate or describe tool output.",
            "sub_tasks": sub_tasks,
            "complexity": "complex",
            "provider": "flash",
        }),
        HashMap::new(),
        tokio_util::sync::CancellationToken::new(),
    );

    match orchestrator.execute(&ctx).await {
        Ok(output) => {
            let response = output.data["response"].as_str().unwrap_or("(no output)");
            println!("{response}");

            let tokens_in = output.data["tokens_in"].as_u64().unwrap_or(0);
            let tokens_out = output.data["tokens_out"].as_u64().unwrap_or(0);
            let worker_count = output.data["worker_count"].as_u64().unwrap_or(0);
            let successful = output.data["successful_workers"].as_u64().unwrap_or(0);
            let duration = output.data["duration_secs"].as_f64().unwrap_or(0.0);
            eprintln!("\n📊 Orchestrator: {worker_count} workers ({successful} successful), {:.1}s, {tokens_in} in / {tokens_out} out", duration);
        }
        Err(e) => {
            eprintln!("❌ Orchestrator failed: {e}");
        }
    }
}

// ── Loop command ───────────────────────────────────────────────

async fn loop_command(query: &str, cli_max_loops: usize, config: &Arc<AppConfig>) {
    use tokio_util::sync::CancellationToken;

    if config.deepseek_api_key.is_none() {
        eprintln!("Error: DEEPSEEK_API_KEY required.");
        eprintln!("Set DEEPSEEK_API_KEY in .env or as environment variable.");
        return;
    }

    // Read max_loops from config, fall back to CLI argument
    let max_loops = if config.loop_max_loops > 0 { config.loop_max_loops } else { cli_max_loops };

    println!("🔄 Loop Pipeline: Explore → Plan → Dispatch → Evaluate → (Repair → Explore → ...)");
    println!("   Query: {query}");
    println!("   Max loops: {max_loops}\n");

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    match miniagent_loop_pipeline::LoopPipeline::run(query, config.clone(), max_loops, cancel).await {
        Ok(state) => {
            let output = state.final_output.unwrap_or_default();
            println!("{output}");
        }
        Err(e) => {
            eprintln!("\x1b[31mError: {}\x1b[0m", e);
        }
    }
}

// ── Debate command ────────────────────────────────────────────

async fn debate_command(query: &str, rounds: usize, config: &Arc<AppConfig>) {
    use miniagent_core::orchestration::{StageInput, StageDriver as _};
    use miniagent_planning::runners::{DebateRunner, DebateRound};
    use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    let key = match config.require_active_key() {
        Ok(k) => k.clone(),
        Err(e) => { eprintln!("{e}"); return; }
    };

    println!("🎤 Scientific Debate (DebateRunner): up to {} revise round(s) | Proposer vs Opponent → Judge\n", rounds);
    println!("   Topic: {query}\n");

    let work_dir = PathBuf::from("./miniagent_debate");
    let cancel = CancellationToken::new();

    // Models: Proposer uses Pro (deep reasoning), Opponent uses Flash (fast critique), Judge uses Pro (careful deliberation).
    // 根据 PROVIDER 配置选择 provider（尊重 PROVIDER=stepfun）。
    let (proposer_provider, opponent_provider, judge_provider): (
        Box<dyn LlmProvider>, Box<dyn LlmProvider>, Box<dyn LlmProvider>
    ) = if config.is_stepfun() {
        // StepFun 单模型，三个角色都用 StepFunFlash
        (
            Box::new(StepFunFlash::new(&key)),
            Box::new(StepFunFlash::new(&key)),
            Box::new(StepFunFlash::new(&key)),
        )
    } else {
        (
            Box::new(DeepSeekPro::new(&key)),
            Box::new(DeepSeekFlash::new(&key)),
            Box::new(DeepSeekPro::new(&key)),
        )
    };
    let runner = DebateRunner::new(proposer_provider, opponent_provider, judge_provider, work_dir)
        .with_max_revise_rounds(rounds);

    let input = StageInput::new("debate", serde_json::json!(query), cancel.clone());
    match runner.run(input).await {
        Ok(outcome) => {
            println!("{}", outcome.summary);
            // Pretty-print each debate round.
            if let Ok(rounds) = serde_json::from_value::<Vec<DebateRound>>(outcome.data.clone()) {
                for r in &rounds {
                    println!("\n━━━ Round {} (verdict: {}) ━━━", r.round, r.verdict);
                    println!("📝 Proposer:\n{}\n", r.proposer.content);
                    println!("⚔️  Opponent:\n{}\n", r.opponent.content);
                    println!("⚖️  Judge:\n{}\n", r.judge.content);
                }
            }
            println!("✅ Debate complete (DebateRunner: {}).", runner.name());
        }
        Err(e) => eprintln!("❌ Debate failed: {e}"),
    }
}

// ── Team command ──────────────────────────────────────────────

async fn team_command(query: &str, config: &Arc<AppConfig>) {
    use miniagent_core::orchestration::{StageInput, StageDriver as _};
    use miniagent_planning::runners::StateGraphRunner;
    use miniagent_planning::state_graph::StateGraph;
    use miniagent_planning::ModelTier;
    use tokio_util::sync::CancellationToken;

    let _key = match config.require_active_key() {
        Ok(k) => k.clone(),
        Err(e) => { eprintln!("{e}"); return; }
    };

    println!("👥 Scientific Team Pipeline (StateGraphRunner)");
    println!("   Task: {query}\n");

    let (flash, pro) = make_providers(config);

    // Build StateGraph: Researcher → Critic → Synthesizer → Reviewer → HITL
    let graph = StateGraph::new("researcher")
        .add_agent("researcher", "You research the topic thoroughly using available tools.", ModelTier::Flash)
        .add_agent("critic", "You critically evaluate the research findings for weaknesses.", ModelTier::Flash)
        .add_agent("synthesizer", "You synthesize findings into a coherent report with hypotheses.", ModelTier::Pro)
        .add_agent("reviewer", "You perform final quality review against scientific standards.", ModelTier::Pro)
        .add_human("approval", "Review the final output. Approve or request changes.")
        .add_edge("researcher", "critic")
        .add_edge("critic", "synthesizer")
        .add_edge("synthesizer", "reviewer")
        .add_edge("reviewer", "approval")
        .with_checkpoint("synthesizer")
        .with_checkpoint("reviewer");

    match graph.compile() {
        Ok(compiled) => {
            println!("{}", compiled.visualize());
            println!("⚡ Executing team pipeline via StateGraphRunner...\n");

            let runner = StateGraphRunner::new(compiled, flash, pro);
            let input = StageInput::new("team", serde_json::json!(query), CancellationToken::new());
            match runner.run(input).await {
                Ok(outcome) => {
                    println!("\n📋 Team pipeline complete: {}", outcome.summary);
                    // step_outputs surface as ArtifactWritten side effects.
                    println!("   step outputs: {}", outcome.side_effects.len() / 2);
                }
                Err(e) => eprintln!("❌ Failed: {e}"),
            }
        }
        Err(e) => eprintln!("❌ Graph error: {e}"),
    }
}


#[cfg(test)]
mod research_tests {
    use super::*;

    #[test]
    fn geo_accession_parsed_from_listing() {
        let listing = "## GEO DataSet Search: 'q'\nTotal: 5 | Showing: 2\n\n1. **GSE12345** — Alzheimer expression\n   https://www.ncbi.nlm.nih.gov/geo/query/acc.cgi?acc=GSE12345\n\n2. **GSE999** — other\n";
        assert_eq!(first_geo_accession(listing).as_deref(), Some("GSE12345"));
        assert_eq!(first_geo_accession("no accessions here"), None);
        assert_eq!(first_geo_accession("**GSE"), None); // too short / malformed
    }

    #[test]
    fn geo_query_strips_stopwords_and_dedupes() {
        let q = geo_query_from_parts(
            "Test whether APOE expression differs between cohorts",
            "APOE drives Alzheimer pathology",
        );
        assert!(q.contains("apoe"));
        assert!(q.contains("alzheimer"));
        assert!(!q.contains(" whether "));
        assert!(!q.contains(" between "));
        // no duplicate words
        let words: Vec<&str> = q.split(' ').collect();
        let uniq: std::collections::HashSet<&str> = words.iter().copied().collect();
        assert_eq!(words.len(), uniq.len());
    }

    #[test]
    fn kg_roundtrip_preserves_ids_and_edges() {
        use miniagent_kg::schema::{Entity, EntityId, EntityType, Relation, RelationId, RelationType};

        let mut kg = miniagent_kg::KnowledgeGraph::new();
        let head = Entity { id: EntityId::new(), name: "APOE".into(), entity_type: EntityType::Gene, aliases: vec!["ApoE".into()], metadata: serde_json::json!({}) };
        let tail = Entity { id: EntityId::new(), name: "Alzheimer disease".into(), entity_type: EntityType::Disease, aliases: vec![], metadata: serde_json::json!({}) };
        let head_id = head.id;
        let tail_id = tail.id;
        kg.add_entity(head);
        kg.add_entity(tail);
        kg.add_relation(Relation {
            id: RelationId::new(),
            from_id: head_id,
            to_id: tail_id,
            relation_type: RelationType::AssociatedWith,
            confidence: 0.9,
            evidence: "test".into(),
            source_paper_id: None,
        });

        let path = std::env::temp_dir().join(format!("mn_kg_test_{}.json", uuid::Uuid::new_v4()));
        save_kg(&kg, &path).expect("save kg");
        let loaded = load_kg(&path).expect("load kg");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.entity_count(), 2);
        assert_eq!(loaded.relation_count(), 1);
        // ids preserved → cached link-prediction candidates stay valid
        assert_eq!(loaded.get_entity(&head_id).map(|e| e.name.as_str()), Some("APOE"));
        assert!(loaded.contains_edge(&head_id, &RelationType::AssociatedWith, &tail_id));
    }
}
