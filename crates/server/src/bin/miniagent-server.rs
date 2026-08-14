use std::sync::Arc;
use miniagent_agent::Agent;
use miniagent_memory::manager::MemoryManager;
use miniagent_core::models::ModelRegistry;
use miniagent_core::settings::AppConfig;
use miniagent_server::{serve, ServerConfig};
use miniagent_tool::executor::ToolExecutor;
use miniagent_tool::tools;

#[tokio::main]
async fn main() {
    let config = Arc::new(AppConfig::load());

    miniagent_telemetry::init("error");

    // Build providers from the active model-profile (registry = env built-ins
    // + user-defined models from models.json). No hardcoded model names here.
    let registry = ModelRegistry::load(&config);
    let active = registry.active().clone();
    let build = |tier| miniagent_provider::factory::build_provider(&active, tier);
    use miniagent_provider::factory::ProviderTier;
    let (flash, pro) = match (build(ProviderTier::Flash), build(ProviderTier::Pro)) {
        (Ok(f), Ok(p)) => (f, p),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "Using provider '{}' (model: {}, pro: {})",
        active.display_name,
        active.model_name,
        active.pro_model()
    );
    let agent = Agent::new(flash, pro);

    // 加载权限规则（如果 settings.json 存在）
    let settings_path = std::path::Path::new("./settings.json");
    let rules = miniagent_tool::approval::PermissionRules::load(settings_path);
    let approval: Box<dyn miniagent_tool::approval::ApprovalHandler> = if rules.allow.is_empty() && rules.deny.is_empty() {
        Box::new(miniagent_tool::approval::AutoApprove)
    } else {
        tracing::info!(
            allow_count = rules.allow.len(),
            deny_count = rules.deny.len(),
            "loaded permission rules from settings.json"
        );
        Box::new(miniagent_tool::approval::RuleBasedApproval::new(
            rules,
            Box::new(miniagent_tool::approval::AutoApprove),
        ))
    };

    // 构建 Agent（含 memory + config，先用默认工具）
    let agent = agent
        .with_tools(ToolExecutor::new(tools::defaults(), approval))
        .with_memory(MemoryManager::new_in_memory().expect("in-memory SQLite"))
        .with_config(config.clone());

    // 包 Arc，构造 AgentTool（LLM 可自主派生子 agent）
    let agent = std::sync::Arc::new(agent);
    let (tool_registry, sub_rx) = miniagent_agent::agent_tool::build_tools_with_agent(agent.clone());

    // 运行时替换 ToolExecutor 为含 AgentTool 的版本
    agent.replace_tools(ToolExecutor::new(tool_registry, Box::new(miniagent_tool::approval::AutoApprove)));

    // 注入子 agent 完成事件 receiver
    agent.set_sub_agent_rx(sub_rx);

    let server_config = ServerConfig {
        config: config.clone(),
        agent: agent.clone(),
        memory: None,
        checkpoint_store: None,
    };

    // Probe search backend health in background — won't block server startup
    tokio::spawn(async {
        miniagent_tool::probe_all_backends().await;
    });

    if let Err(e) = serve(server_config).await {
        eprintln!("Server error: {e}");
        std::process::exit(1);
    }
}
