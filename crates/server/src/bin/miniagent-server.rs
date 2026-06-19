use std::sync::Arc;
use miniagent_agent::Agent;
use miniagent_memory::manager::MemoryManager;
use miniagent_core::settings::AppConfig;
use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};
use miniagent_server::{serve, ServerConfig};
use miniagent_tool::approval::AutoApprove;
use miniagent_tool::executor::ToolExecutor;
use miniagent_tool::tools;

#[tokio::main]
async fn main() {
    let config = Arc::new(AppConfig::load());

    miniagent_telemetry::init("info");

    let key = match config.require_deepseek_key() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let flash = Box::new(DeepSeekFlash::new(key));
    let pro = Box::new(DeepSeekPro::new(key));
    let agent = Agent::new(flash, pro)
        .with_tools(ToolExecutor::new(tools::defaults(), Box::new(AutoApprove)))
        .with_memory(MemoryManager::new_in_memory().expect("in-memory SQLite"))
        .with_config(config.clone());

    let server_config = ServerConfig {
        config: config.clone(),
        agent,
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
