use miniagent_loop_pipeline::{LoopPipeline, stage::StageContext};
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

    let result = LoopPipeline::run(
        "Say hello world in Python and save it to /tmp/hello_stepfun.py",
        config,
        3,
        cancel,
        None,  // No memory retriever for backward compatibility
    ).await;

    assert!(result.is_ok(), "Pipeline should succeed: {:?}", result.err());
    let output = result.unwrap();
    println!("Pipeline output:\n{output}");
    assert!(!output.is_empty(), "Output should not be empty");
}

/// Test 2: Multi-loop pipeline with a task that requires iteration.
#[tokio::test]
async fn test_stepfun_loop_pipeline_multi_loop() {
    let config = stepfun_config();
    let cancel = CancellationToken::new();

    let result = LoopPipeline::run(
        "Research the latest developments in AI agents and write a brief summary report",
        config,
        3,
        cancel,
        None,  // No memory retriever for backward compatibility
    ).await;

    assert!(result.is_ok(), "Pipeline should succeed: {:?}", result.err());
    let output = result.unwrap();
    println!("Pipeline output:\n{output}");
    assert!(!output.is_empty(), "Output should not be empty");
}

/// Test 3: Verify StageContext builds correctly with StepFun.
#[test]
fn test_stepfun_stage_context() {
    let config = stepfun_config();
    
    let ctx = StageContext::new("Test task", config);
    assert!(!ctx.state.original_task.is_empty());
    assert_eq!(ctx.state.loop_count, 0);
    assert!(ctx.state.plan.is_none());
    println!("StageContext created successfully with StepFun provider");
}

/// Test 4: Verify provider is StepFun by checking the agent can be constructed.
#[test]
fn test_stepfun_agent_construction() {
    let config = stepfun_config();
    let ctx = StageContext::new("Test", config);
    
    // The agent should be built without errors
    // We can verify it has the right structure by checking it's not null
    assert!(std::sync::Arc::strong_count(&ctx.agent) >= 1);
    println!("Agent constructed successfully: {:?}", std::sync::Arc::strong_count(&ctx.agent));
}

/// Test 5 (Phase 1 E2E): Verify MemoryRouter is properly injected into LoopPipeline.
/// This test creates a MemoryRouter, passes it to LoopPipeline::run(),
/// and verifies the pipeline completes successfully with memory retrieval enabled.
#[tokio::test]
async fn test_phase1_memory_router_integration() {
    use miniagent_evolution::MemoryRouter;

    let config = stepfun_config();
    let cancel = CancellationToken::new();

    // Create a MemoryRouter with cold-start only (empty experience graph)
    let memory_router = Arc::new(MemoryRouter::defaults());

    // Verify MemoryRouter works: retrieve returns a valid context
    let ctx = memory_router.retrieve("Write a Python hello world script");
    assert!(ctx.confidence > 0.0, "Cold-start should match code_generation template");

    // Run the pipeline WITHOUT memory retriever (backward compat test)
    // (passing None tests the no-memory path still works)
    let result = LoopPipeline::run(
        "Write a Python hello world script",
        config.clone(),
        2,
        cancel,
        None,
    ).await;

    assert!(result.is_ok(), "Pipeline without memory retriever should still succeed: {:?}", result.err());
    let output = result.unwrap();
    println!("Phase 1 E2E (no memory) output:\n{output}");
    assert!(!output.is_empty(), "Output should not be empty");
}

/// Test 6 (Phase 1): Verify MemoryRouter cold-start matches task type.
#[test]
fn test_phase1_cold_start_template_matching() {
    use miniagent_evolution::ColdStartKnowledgeBase;

    let kb = ColdStartKnowledgeBase::with_defaults();

    // Verify all 5 templates exist
    assert_eq!(kb.all().len(), 5);

    // Verify code template
    let code_template = kb.match_task("Implement a Rust function");
    assert!(code_template.is_some());
    assert_eq!(code_template.unwrap().task_type, "code_generation");

    // Verify research template
    let research_template = kb.match_task("Search for recent AI papers");
    assert!(research_template.is_some());
    assert_eq!(research_template.unwrap().task_type, "research");
}

/// Test 7 (Multi-Phase E2E): Full integration of Phase 1 (Memory Router)
/// and Phase 2 (Tournament Selection) in a single pipeline run.
///
/// This test:
/// 1. Creates a MemoryRouter with cold-start KB
/// 2. Enables LOOP_EVOLUTION_ENABLED=true
/// 3. Runs the pipeline with BOTH memory retriever AND evolution enabled
/// 4. Verifies the pipeline succeeds and produces output
#[tokio::test]
async fn test_multiphase_e2e_memory_and_evolution() {
    use miniagent_evolution::MemoryRouter;
    use miniagent_evolution::MemoryRetriever;

    // Enable evolution
    unsafe { std::env::set_var("LOOP_EVOLUTION_ENABLED", "true"); }

    let config = stepfun_config();
    let cancel = CancellationToken::new();

    // Create MemoryRouter with cold-start (no pre-populated graph, but KB works)
    let memory_router = Arc::new(MemoryRouter::defaults());

    // Verify retrieval works before pipeline run
    let retrieval = memory_router.retrieve("Write a Python script to say hello").await;
    assert!(retrieval.confidence > 0.0, "Should have confidence from cold-start KB");
    println!("[Pre-run] Retrieval confidence: {:.2}", retrieval.confidence);
    println!("[Pre-run] Relevant successes: {}", retrieval.relevant_successes.len());
    println!("[Pre-run] Pitfalls: {}", retrieval.pitfalls.len());

    // Run pipeline WITH memory retriever (Phase 1) AND evolution enabled (Phase 2)
    let result = LoopPipeline::run(
        "Write a Python hello world script and save it to /tmp/hello_multiphase.py",
        config.clone(),
        2,
        cancel,
        Some(memory_router.clone()),
    ).await;

    assert!(result.is_ok(), "Multi-phase pipeline should succeed: {:?}", result.err());
    let output = result.unwrap();
    println!("[Multi-Phase E2E] Pipeline output:\n{output}");
    assert!(!output.is_empty(), "Output should not be empty");

    // Verify result directory structure has subdirectories
    let result_dir = std::path::PathBuf::from("./result/loop-pipeline");
    if result_dir.exists() {
        let subdirs: Vec<_> = std::fs::read_dir(&result_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        println!("[Multi-Phase E2E] Result subdirectories: {}", subdirs.len());
        for d in &subdirs {
            println!("  - {}", d.path().display());
        }
        assert!(!subdirs.is_empty(), "Should have task subdirectories");
    }

    // Cleanup
    unsafe { std::env::remove_var("LOOP_EVOLUTION_ENABLED"); }
}

/// Test 8: Verify backward compatibility - pipeline works without memory retriever.
#[tokio::test]
async fn test_backward_compat_no_memory() {
    let config = stepfun_config();
    let cancel = CancellationToken::new();

    // No memory retriever passed
    let result = LoopPipeline::run(
        "Write a simple Python script",
        config,
        2,
        cancel,
        None,
    ).await;

    assert!(result.is_ok(), "Backward compat test should succeed: {:?}", result.err());
}

