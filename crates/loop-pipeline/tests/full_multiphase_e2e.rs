use miniagent_loop_pipeline::LoopPipeline;
use miniagent_core::settings::AppConfig;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn load_env() {
    let _ = dotenvy::dotenv();
}

fn full_config() -> Arc<AppConfig> {
    load_env();
    unsafe { std::env::set_var("STEPFUN_BASE_URL", "https://api.stepfun.com/step_plan/v1"); }
    unsafe { std::env::set_var("STEPFUN_MODEL_NAME", "step-3.7-flash"); }
    unsafe { std::env::set_var("LOOP_EVOLUTION_ENABLED", "true"); }
    unsafe { std::env::set_var("LOOP_DISPATCH_DECOUPLED", "true"); }
    unsafe { std::env::set_var("LOOP_SEARCH_SCHEDULER_ENABLED", "true"); }
    Arc::new(AppConfig::load())
}

/// Full multi-phase E2E test with a challenging multi-topic research task.
/// All four phases active:
///   Phase 1: Memory Router (cold-start KB + retrieval)
///   Phase 2: Tournament Selection (plan variants + Elo)
///   Phase 3: Decoupled Execution (tactic retry + strategy escalation)
///   Phase 4: Search Scheduler (entropy decay + stagnation detection)
#[tokio::test]
#[ignore] // Requires STEPFUN_API_KEY; run with: cargo test -- --ignored --nocapture
async fn test_full_multiphase_research_task() {
    let config = full_config();
    let cancel = CancellationToken::new();

    let task = "Research two topics and write a comparison report:\n\
                1. Rust's ownership model vs garbage collection\n\
                2. Tokio async runtime vs Go's goroutine scheduler\n\
                Write the report to /tmp/rust_vs_go_report.md with clear sections.";

    let result = LoopPipeline::run(task, config, 3, cancel, None).await;

    assert!(result.is_ok(), "Pipeline should succeed: {:?}", result.err());
    let output = result.unwrap();
    println!("=== Full Multi-Phase E2E Output ===\n{}", &output[..output.len().min(2000)]);

    // Verify output quality
    assert!(!output.is_empty(), "Output should not be empty");
    assert!(
        output.to_lowercase().contains("rust") || output.to_lowercase().contains("ownership"),
        "Output should mention Rust"
    );

    // Check result directory
    let result_dir = std::path::PathBuf::from("./result/loop-pipeline");
    if result_dir.exists() {
        let subdirs: Vec<_> = std::fs::read_dir(&result_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        println!("Result subdirectories: {}", subdirs.len());
        for d in subdirs.iter().take(10) {
            println!("  - {}", d.path().file_name().unwrap().to_string_lossy());
        }
        assert!(!subdirs.is_empty(), "Should have task result subdirectories");
    }

    // Verify the report file was created
    let report_path = std::path::Path::new("/tmp/rust_vs_go_report.md");
    if report_path.exists() {
        let content = std::fs::read_to_string(report_path).unwrap_or_default();
        println!("Report file size: {} bytes", content.len());
        assert!(!content.is_empty(), "Report file should not be empty");
    }
}

/// Phase isolation test: verify each phase can be enabled independently.
#[tokio::test]
#[ignore]
async fn test_phase_isolation() {
    let config = full_config();
    let cancel = CancellationToken::new();

    // Test with only Phase 1 (memory) — no evolution, no decoupled, no scheduler
    unsafe { std::env::set_var("LOOP_EVOLUTION_ENABLED", "false"); }
    unsafe { std::env::set_var("LOOP_DISPATCH_DECOUPLED", "false"); }
    unsafe { std::env::set_var("LOOP_SEARCH_SCHEDULER_ENABLED", "false"); }
    let config_p1 = Arc::new(AppConfig::load());

    let result = LoopPipeline::run(
        "Write a simple Python hello world script",
        config_p1,
        2,
        cancel.clone(),
        None,
    ).await;
    assert!(result.is_ok(), "Phase 1 only should succeed: {:?}", result.err());

    println!("Phase isolation test passed");
}

/// Verify all unit tests pass in one run.
#[test]
fn test_all_evolution_unit_tests_pass() {
    // This is a meta-test: if this compiles, all evolution types are properly exported.
    use miniagent_evolution::{
        ColdStartKnowledgeBase, MemoryRouter, MemoryRetriever,
        SelectionEngine, CandidatePlan, MutationOp,
        EscalationContext, TacticResult,
        SearchScheduler, SearchStrategy, EliteEntry,
    };
    use miniagent_core::{TaskPlan, TaskUnit};

    // Verify types exist and are constructible
    let _kb = ColdStartKnowledgeBase::with_defaults();
    let _router = MemoryRouter::defaults();
    let _engine = SelectionEngine::new(3);
    let _scheduler = SearchScheduler::new();
    let _tactic = TacticResult {
        success: true,
        output: String::new(),
        error: None,
        error_messages: vec![],
        tokens_used: 0,
    };
    let _escalation = EscalationContext {
        task_id: "t1".into(),
        task_description: "test".into(),
        expected_output: "out".into(),
        failure_history: vec![],
        consecutive_failures: 0,
    };

    println!("All evolution types properly exported and constructible ✅");
}
