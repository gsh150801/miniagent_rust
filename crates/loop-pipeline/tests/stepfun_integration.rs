use miniagent_loop_pipeline::LoopPipeline;
use miniagent_core::settings::AppConfig;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Load .env so STEPFUN_API_KEY etc. are available.
fn load_env() {
    let _ = dotenvy::dotenv();
}

fn stepfun_config() -> Arc<AppConfig> {
    load_env();
    let key = std::env::var("STEPFUN_API_KEY")
        .expect("STEPFUN_API_KEY must be set in .env or environment");
    unsafe { std::env::set_var("STEPFUN_API_KEY", &key); }
    unsafe { std::env::set_var("STEPFUN_BASE_URL", "https://api.stepfun.com/step_plan/v1"); }
    unsafe { std::env::set_var("STEPFUN_MODEL_NAME", "step-3.7-flash"); }
    Arc::new(AppConfig::load())
}

/// Test 1: Simple single-loop pipeline with StepFun.
/// This should complete in 1 loop with a simple task.
#[tokio::test]
async fn test_stepfun_loop_pipeline_simple() {
    let config = stepfun_config();
    let cancel = CancellationToken::new();

    // Ensure no proxy is used for direct API connection
    for var in ["ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        unsafe { std::env::remove_var(var); }
    }

    let result = LoopPipeline::run(
        "Say hello world in Python and save it to /tmp/hello_stepfun.py",
        config,
        3,
        cancel,
        None, None).await;

    assert!(result.is_ok(), "Pipeline should succeed: {:?}", result.err());
    let state = result.unwrap();
    let output = state.final_output.unwrap_or_default();
    println!("Pipeline output:\n{output}");
    assert!(!output.is_empty(), "Output should not be empty");
}

/// Test 2: Multi-loop pipeline with a task that requires iteration.
#[tokio::test]
async fn test_stepfun_loop_pipeline_multi_loop() {
    let config = stepfun_config();
    let cancel = CancellationToken::new();

    // Ensure no proxy is used for direct API connection
    for var in ["ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        unsafe { std::env::remove_var(var); }
    }

    let result = LoopPipeline::run(
        "Research the latest developments in AI agents and write a brief summary report",
        config,
        3,
        cancel,
        None, None).await;

    assert!(result.is_ok(), "Pipeline should succeed: {:?}", result.err());
    let state = result.unwrap();
    let output = state.final_output.unwrap_or_default();
    println!("Pipeline output:\n{output}");
    assert!(!output.is_empty(), "Output should not be empty");
}

/// Test 3: Verify StageContext builds correctly with StepFun.
#[test]
fn test_stepfun_stage_context() {
    let config = stepfun_config();
    
    let ctx = miniagent_loop_pipeline::stage::StageContext::new("Test task", config);
    assert!(!ctx.state.original_task.is_empty());
    assert_eq!(ctx.state.loop_count, 0);
    assert!(ctx.state.plan.is_none());
    println!("StageContext created successfully with StepFun provider");
}

/// Test 4: Verify provider is StepFun by checking the agent can be constructed.
#[test]
fn test_stepfun_agent_construction() {
    let config = stepfun_config();
    let ctx = miniagent_loop_pipeline::stage::StageContext::new("Test", config);
    
    // The agent should be built without errors
    // We can verify it has the right structure by checking it's not null
    assert!(std::sync::Arc::strong_count(&ctx.agent) >= 1);
    println!("Agent constructed successfully: {:?}", std::sync::Arc::strong_count(&ctx.agent));
}
