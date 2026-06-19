use miniagent_core::secrets::ApiKey;
use miniagent_provider::stepfun::StepFunFlash;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use miniagent_core::config::InferenceConfig;
use miniagent_core::message::Message;
use tokio_util::sync::CancellationToken;

/// Load .env so STEPFUN_API_KEY etc. are available.
fn load_env() {
    let _ = dotenvy::dotenv();
}

/// A simple smoke test: call StepFun API with a basic prompt.
/// 
/// Run with:
///   cargo test --package miniagent-provider --test stepfun_smoke -- --nocapture
#[tokio::test]
async fn test_stepfun_smoke() {
    load_env();

    let key = std::env::var("STEPFUN_API_KEY")
        .expect("STEPFUN_API_KEY must be set in .env or environment");

    let provider = StepFunFlash::new(&ApiKey::new(key));

    let request = CompletionRequest {
        system: "You are a helpful assistant.".into(),
        messages: vec![Message::user("Say 'Hello from StepFun!' and nothing else.")],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.0),
            max_tokens: Some(100),
            ..Default::default()
        },
    };

    let cancel = CancellationToken::new();
    let response = provider.complete(&request, cancel)
        .await
        .expect("StepFun API call should succeed");

    let text: String = response.content.iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();

    println!("StepFun response: {text}");
    assert!(!text.is_empty(), "Response should not be empty");
    assert!(text.contains("StepFun") || text.contains("Hello"), 
        "Response should mention StepFun or Hello, got: {text}");
}

/// Test with a tool-use capable request (function calling).
#[tokio::test]
async fn test_stepfun_tool_use() {
    load_env();

    let key = std::env::var("STEPFUN_API_KEY")
        .expect("STEPFUN_API_KEY must be set");

    let provider = StepFunFlash::new(&ApiKey::new(key));

    let request = CompletionRequest {
        system: "You are a calculator assistant.".into(),
        messages: vec![Message::user("What is 25 * 47?")],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.0),
            max_tokens: Some(200),
            ..Default::default()
        },
    };

    let cancel = CancellationToken::new();
    let response = provider.complete(&request, cancel)
        .await
        .expect("StepFun tool-use call should succeed");

    let text: String = response.content.iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();

    println!("StepFun calculator response: {text}");
    assert!(!text.is_empty());
    assert!(text.contains("1175"), "Should calculate 25*47=1175, got: {text}");
}

/// Test that StepFun client respects custom base URL from env.
#[tokio::test]
async fn test_stepfun_custom_base_url() {
    load_env();

    // Set a custom base URL via env
    unsafe {
        std::env::set_var("STEPFUN_BASE_URL", "https://api.stepfun.com/step_plan/v1");
        std::env::set_var("STEPFUN_MODEL_NAME", "step-3.7-flash");
    }

    let key = std::env::var("STEPFUN_API_KEY")
        .expect("STEPFUN_API_KEY must be set");

    let provider = StepFunFlash::new(&ApiKey::new(key));

    let request = CompletionRequest {
        system: "You are a helpful assistant.".into(),
        messages: vec![Message::user("Reply with just: OK")],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.0),
            max_tokens: Some(50),
            ..Default::default()
        },
    };

    let cancel = CancellationToken::new();
    let response = provider.complete(&request, cancel)
        .await
        .expect("StepFun custom base URL call should succeed");

    let text: String = response.content.iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();

    println!("StepFun custom URL response: {text}");
    assert!(!text.is_empty());
}
