use std::sync::Arc;
use miniagent_agent::Agent;
use miniagent_memory::manager::MemoryManager;
use miniagent_core::settings::AppConfig;
use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};
use miniagent_provider::stepfun::StepFunFlash;
use miniagent_server::{serve, ServerConfig};
use miniagent_tool::executor::ToolExecutor;
use miniagent_tool::tools;

#[tokio::main]
async fn main() {
    let config = Arc::new(AppConfig::load());

    miniagent_telemetry::init("error");

    // Build providers based on the PROVIDER env var ("deepseek" or "stepfun").
    let agent = if config.is_stepfun() {
        let key = match config.require_stepfun_key() {
            Ok(k) => k,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        eprintln!("Using StepFun provider (model: {})", config.stepfun_model_name.as_deref().unwrap_or("step-3.7-flash"));
        // StepFun has a single model tier — use it for both flash and pro.
        let flash = Box::new(StepFunFlash::new(key));
        let pro = Box::new(StepFunFlash::new(key));
        Agent::new(flash, pro)
    } else {
        let key = match config.require_deepseek_key() {
            Ok(k) => k,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        eprintln!("Using DeepSeek provider");
        let flash = Box::new(DeepSeekFlash::new(key));
        let pro = Box::new(DeepSeekPro::new(key));
        Agent::new(flash, pro)
    };

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

    // ServerConfig 需要 owned Agent——此时 Arc 唯一引用（AgentTool 内部用 Weak 不会阻止）
    // 但 AgentTool 持有 Arc clone，所以 try_unwrap 会失败。
    // 解决：clone 一份给 ServerConfig（Arc<Agent> 即可，ServerConfig 也接受 Arc）
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
