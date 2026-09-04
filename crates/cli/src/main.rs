use std::sync::Arc;
use clap::{Parser, Subcommand};
use miniagent_agent::Agent;
use miniagent_core::config::TaskComplexity;
use miniagent_core::message::Message;
use miniagent_core::secrets::ApiKey;
use miniagent_core::settings::AppConfig;
use miniagent_provider::deepseek::DeepSeekFlash;
use miniagent_provider::stepfun::StepFunFlash;
use miniagent_provider::minimax::MiniMaxFlash;
use miniagent_provider::router::ProviderChoice;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

/// 构造 (flash, pro) provider 对，由模型注册表（ModelRegistry）决定具体模型。
///
/// 注册表 = .env 内置档案（deepseek/stepfun/minimax）+ models.json 自定义档案。
/// 所有 CLI 命令应通过此函数获取 provider —— 代码中不出现硬编码模型名。
fn make_providers(config: &AppConfig) -> (Box<dyn LlmProvider>, Box<dyn LlmProvider>) {
    match miniagent_provider::factory::active_provider_pair(config) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
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

        /// Max papers to retrieve and analyze (optional; derived from the
        /// request semantics when omitted, PubMed max 500)
        #[arg(short = 'n', long)]
        max_papers: Option<usize>,

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

        /// Seed link prediction with the persistent cross-project KG store
        /// (kg_store.json, merged automatically after every run). Broadens
        /// candidates with knowledge accumulated from previous queries.
        #[arg(long)]
        use_store: bool,

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

        /// Maximum papers to collect (optional; derived from the request
        /// semantics when omitted, PubMed max 500)
        #[arg(short = 'n', long)]
        max_papers: Option<usize>,

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
            use_store,
            project_dir,
            min_year,
        } => {
            // --debate defaults to ON when --validate is set; --no-debate forces off.
            let debate = debate.unwrap_or(validate);
            let opts = miniagent_research::ResearchOptions {
                max_papers, kg_only, validate, analyze,
                data, top_n, enrich_file, enrich_delim,
                enrich_relation, debate, min_year, use_store, stop_after: None,
            };
            // Unified result layout: result/{id}_{brief} (server-compatible).
            let dir = match project_dir.as_deref() {
                Some(p) => std::path::PathBuf::from(p),
                None => miniagent_core::paths::result_root().join(cli_task_dir_name(&query)),
            };
            println!("📁 project dir: {}", dir.display());
            // Research × Loop: same loop-orchestrated path as the server
            // (per-phase explore→plan→dispatch→adjudicate→repair); the
            // clarify step self-skips without an interactive channel.
            let summary = miniagent_research::run_research_in_loop(query.clone(), dir.clone(), opts, config.clone(), None, None, None, false).await;
            println!("{summary}");
            // The research pipeline writes `<brief>.md` (user-facing report)
            // and `<brief>.md` is what the server's restart-restore scan
            // uses to detect completion. No additional write is needed here;
            // writing `summary` again would overwrite the rich user report.
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

    let registry = miniagent_core::models::ModelRegistry::load(&config);
    let profile = registry.active().clone();
    let provider_name = if provider == "pro" && profile.pro_model_name.is_some() {
        format!("{} ({})", profile.display_name, profile.pro_model())
    } else {
        profile.display_name.clone()
    };
    let provider_name = provider_name.as_str();

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
    let task_dir = miniagent_core::paths::result_root().join(&task_dir_name);
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
    match workflow.run(cancel).await {
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
    miniagent_core::paths::sanitize_task_brief(prompt)
}

/// Unified CLI task directory name: `{8-char-id}_{brief}`.
///
/// Matches the server's `create_new_task` scheme so every mode (server
/// workflow/loop/debate/research, CLI debate/research) lands its artifacts
/// under `result/{resultid}_{resultname}` and the server's restart scan can
/// pick CLI-produced runs up as tasks.
fn cli_task_dir_name(prompt: &str) -> String {
    let id: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
    format!("{}_{}", id, sanitize_task_brief(prompt))
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
    max_papers: Option<usize>,
    generate_hypotheses: bool,
    config: &Arc<AppConfig>,
) {
    // Delegate to the real research pipeline
    let kg_only = !generate_hypotheses;
    let opts = miniagent_research::ResearchOptions { max_papers, kg_only, ..Default::default() };
    let dir = miniagent_core::paths::result_root().join(cli_task_dir_name(query));
    let summary = miniagent_research::run_research(query.to_string(), dir.clone(), opts, config.clone(), None).await;
    println!("{summary}");
    if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        let brief = name.split_once('_').map(|(_, b)| b).unwrap_or(name);
        let _ = std::fs::write(dir.join(format!("{brief}.md")), &summary);
    }
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

async fn plan_command(query: &str, config: &Arc<AppConfig>) {
    use miniagent_agent::Agent;
    use miniagent_core::orchestration::{StageInput, StageDriver as _};
    let key = match config.require_active_key() {
        Ok(k) => k.clone(),
        Err(e) => { eprintln!("{e}"); return; }
    };
    use miniagent_planning::runners::PlanRunner;
    use miniagent_tool::tools;
    use miniagent_tool::approval::AutoApprove;
    use miniagent_tool::executor::ToolExecutor;
    use std::sync::Arc;

    if let Err(e) = config.require_active_key() {
        eprintln!("{e}");
        return;
    }

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

    let _key = match config.require_active_key() {
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
            use miniagent_core::message::Message;
            use miniagent_core::config::InferenceConfig;
            use miniagent_provider::traits::{CompletionRequest, LlmProvider};

            // 供应商经模型注册表解析（活跃档案），并附跨家族备援——
            // 硬编码 StepFun/DeepSeek 会在供应商故障（429/402）时直接失败。
            let active_profile = active_model_profile(config);
            let decompose_providers: Vec<Box<dyn LlmProvider>> = {
                let mut v = vec![miniagent_provider::factory::build_provider(
                    &active_profile, miniagent_provider::factory::ProviderTier::Flash,
                ).expect("active model profile usable")];
                v.extend(miniagent_provider::factory::codegen_fallback_providers(config));
                v
            };
            let flash_provider = decompose_providers.first().expect("at least one provider");

            // P-多智能体分配：两阶段——先枚举独立工作项（模型对"列举"的
            // 服从度远高于"完整分解"），≥2 项时机械扇出为多个 worker。
            let mut tasks: Vec<String> = Vec::new();
            if let Some(items) = miniagent_loop_pipeline::plan::enumerate_work_items(
                flash_provider.as_ref(),
                &query,
                tokio_util::sync::CancellationToken::new(),
            )
            .await
                && items.len() >= 2
            {
                eprintln!("   🧩 enumerated {} parallel work items:", items.len());
                for (i, (title, role)) in items.iter().enumerate() {
                    eprintln!("     {}. [{role}] {title:.90}", i + 1);
                }
                tasks = items.into_iter().map(|(t, _)| t).collect();
                tasks.push("汇总以上全部子任务结果，输出最终报告".to_string());
            }

            if tasks.is_empty() {
                // 回退：LLM 完整分解（枚举失败或单项时的既有路径）
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
                tasks = match flash_provider.complete(&request, cancel).await {
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
                };
            }
            tasks
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

    match miniagent_loop_pipeline::LoopPipeline::run(query, config.clone(), max_loops, cancel, None, None).await {
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
    use tokio_util::sync::CancellationToken;

    println!("🎤 Scientific Debate (DebateRunner): up to {} revise round(s) | Proposer vs Opponent → Judge\n", rounds);
    println!("   Topic: {query}\n");

    // Unified result layout: ./result/{id}_{brief}/ like every other mode
    // (previously ./miniagent_debate at the repo root, invisible to the
    // server's result-dir scan and download API).
    let work_dir = miniagent_core::paths::result_root().join(cli_task_dir_name(query));
    let _ = std::fs::create_dir_all(&work_dir);
    println!("   Work dir: {}\n", work_dir.display());
    let cancel = CancellationToken::new();

    // Role providers route through the model registry: ⚙️ selection >
    // DEBATE_*_MODEL env > active main model. No more hardcoded DeepSeek /
    // StepFun clients (which broke under PROVIDER=minimax).
    let (proposer_provider, opponent_provider, judge_provider) =
        match miniagent_provider::factory::resolve_debate_providers(config) {
            Ok(trio) => trio,
            Err(e) => { eprintln!("❌ debate providers: {e}"); return; }
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

/// Load the active model profile from the runtime registry (per-call, cheap).
fn active_model_profile(config: &Arc<AppConfig>) -> miniagent_core::models::ModelProfile {
    miniagent_core::models::ModelRegistry::load(config).active().clone()
}
