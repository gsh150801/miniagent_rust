use miniagent_agent::Agent;
use miniagent_memory::manager::MemoryManager;
use miniagent_provider::deepseek::{DeepSeekFlash, DeepSeekPro};
use miniagent_server::{serve, ServerConfig};
use miniagent_tool::approval::AutoApprove;
use miniagent_tool::executor::ToolExecutor;
use miniagent_tool::tools;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    miniagent_telemetry::init("info");

    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .expect("DEEPSEEK_API_KEY required in .env");

    let max_iterations = std::env::var("MAX_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(35);

    let max_tokens = std::env::var("MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(393_216);

    let flash = Box::new(DeepSeekFlash::new(&api_key));
    let pro = Box::new(DeepSeekPro::new(&api_key));
    let agent = Agent::new(flash, pro)
        .with_tools(ToolExecutor::new(tools::defaults(), Box::new(AutoApprove)))
        .with_memory(MemoryManager::new_in_memory().expect("in-memory SQLite"));

    let config = ServerConfig {
        host: "0.0.0.0".into(),
        port: 3000,
        agent,
        memory: None,
        checkpoint_store: None,
        api_key,
        max_iterations,
        max_tokens,
    };

    if let Err(e) = serve(config).await {
        eprintln!("Server error: {e}");
        std::process::exit(1);
    }
}
