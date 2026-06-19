use miniagent_evolution::{ColdStartKnowledgeBase, MemoryRouter, RetrievalContext};
use miniagent_self_improve::offline::experience_graph::ExperienceGraph;
use miniagent_self_improve::online::q_router::QLearningRouter;
use std::sync::Arc;

// ── ColdStartKnowledgeBase Tests ──────────────────────────────

#[test]
fn test_cold_start_kb_has_default_templates() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    assert_eq!(kb.all().len(), 5, "Should have 5 domain templates");
}

#[test]
fn test_cold_start_kb_match_code_task() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    let template = kb.match_task("Write a Python script to process data");
    assert!(template.is_some(), "Should match code_generation template");
    assert_eq!(template.unwrap().task_type, "code_generation");
}

#[test]
fn test_cold_start_kb_match_research_task() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    let template = kb.match_task("Research the latest developments in AI agents");
    assert!(template.is_some(), "Should match research template");
    assert_eq!(template.unwrap().task_type, "research");
}

#[test]
fn test_cold_start_kb_match_report_task() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    let template = kb.match_task("Write a comprehensive report on climate change");
    assert!(template.is_some(), "Should match report_writing template");
    assert_eq!(template.unwrap().task_type, "report_writing");
}

#[test]
fn test_cold_start_kb_match_data_analysis() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    let template = kb.match_task("Analyze the CSV dataset and generate charts");
    assert!(template.is_some(), "Should match data_analysis template");
    assert_eq!(template.unwrap().task_type, "data_analysis");
}

#[test]
fn test_cold_start_kb_no_match() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    let template = kb.match_task("qwerty asdfgh zxcvbn");
    assert!(template.is_none(), "Should not match gibberish");
}

#[test]
fn test_cold_start_kb_template_has_tools() {
    let kb = ColdStartKnowledgeBase::with_defaults();
    for template in kb.all() {
        assert!(!template.typical_tools.is_empty(), "Template '{}' should have typical_tools", template.task_type);
        assert!(!template.keywords.is_empty(), "Template '{}' should have keywords", template.task_type);
    }
}

// ── MemoryRouter Tests ────────────────────────────────────────

fn make_router() -> MemoryRouter {
    MemoryRouter::defaults()
}

#[test]
fn test_memory_router_retrieve_returns_context() {
    let router = make_router();
    let ctx = router.retrieve("Write a Python script to process data");
    
    assert!(ctx.confidence > 0.0, "Should have some confidence from cold-start");
}

#[test]
fn test_memory_router_retrieve_code_task() {
    let router = make_router();
    let ctx = router.retrieve("Implement a Rust function to parse JSON");
    
    assert!(ctx.confidence > 0.0, "Code task should match domain template");
}

#[test]
fn test_memory_router_retrieve_research_task() {
    let router = make_router();
    let ctx = router.retrieve("Research the latest AI developments and summarize findings");
    
    assert!(ctx.confidence > 0.0, "Research task should match domain template");
}

#[test]
fn test_memory_router_retrieve_no_match() {
    let router = make_router();
    let ctx = router.retrieve("qwerty asdfgh zxcvbn");
    
    assert!(ctx.relevant_successes.is_empty(), "No successes expected");
    assert!(ctx.pitfalls.is_empty(), "No pitfalls expected");
    assert_eq!(ctx.confidence, 0.0, "No match → confidence should be 0");
}

#[test]
fn test_memory_router_suggested_provider() {
    let router = make_router();
    let ctx = router.retrieve("Research complex AI agent architectures");
    
    // Confidence should be > 0 due to cold-start match
    assert!(ctx.confidence > 0.0, "Should have confidence from domain match");
}

#[test]
fn test_memory_router_text_signature_produces_valid_vector() {
    let router = make_router();
    let sig = router.text_signature("Write a Python script");
    assert_eq!(sig.len(), 11, "Signature should be 11-dimensional (5 base + 6 language/domain)");
    assert!(sig[2] > 0.0, "Should detect code keyword (script), got sig={:?}", sig);
    assert!(sig[4] > 0.0, "Should detect write keyword (case-insensitive), got sig={:?}", sig);
}

#[test]
fn test_memory_router_text_signature_research() {
    let router = make_router();
    let sig = router.text_signature("Research machine learning trends");
    assert!(sig[3] > 0.0, "Should detect research keyword");
    assert!(sig[2] == 0.0, "Should not detect code keyword");
}

#[test]
fn test_memory_router_text_signature_write() {
    let router = make_router();
    let sig = router.text_signature("summarize the findings in a report");
    assert!(sig[4] > 0.0, "Should detect write keyword via 'summarize', got sig={:?}", sig);
}

// ── RRF Fusion Tests ──────────────────────────────────────────

#[test]
fn test_rrf_fusion_via_retrieve_with_experiences() {
    let mut graph = ExperienceGraph::new();
    
    let sig_code = vec![0.5, 0.5, 1.0, 0.0, 0.0];
    graph.add_experience(
        miniagent_self_improve::offline::experience_graph::NodeType::SuccessPattern,
        "Successfully implemented Python CLI tool",
        &vec!["Use argparse for CLI".into()],
        &sig_code,
    );
    
    let sig_research = vec![0.3, 0.3, 0.0, 1.0, 0.0];
    graph.add_experience(
        miniagent_self_improve::offline::experience_graph::NodeType::SuccessPattern,
        "Successfully researched ML papers",
        &vec!["Use semantic scholar API".into()],
        &sig_research,
    );
    
    let router = MemoryRouter::new(
        Arc::new(std::sync::Mutex::new(graph)),
        Arc::new(std::sync::Mutex::new(QLearningRouter::new())),
        Arc::new(ColdStartKnowledgeBase::with_defaults()),
    );
    
    let ctx = router.retrieve("Write a Python CLI tool for data processing");
    assert!(!ctx.relevant_successes.is_empty());
    assert!(ctx.relevant_successes[0].description.contains("CLI") ||
            ctx.relevant_successes[0].description.contains("Python"),
            "Should prioritize code experience, got: {}",
            ctx.relevant_successes[0].description);
}

// ── RetrievalContext Serialization Tests ──────────────────────

#[test]
fn test_retrieval_context_serialization() {
    let ctx = RetrievalContext {
        relevant_successes: vec![],
        pitfalls: vec![],
        confidence: 0.7,
    };
    
    let json = serde_json::to_string(&ctx).expect("Should serialize");
    let deserialized: RetrievalContext = serde_json::from_str(&json).expect("Should deserialize");
    assert_eq!(deserialized.confidence, 0.7);
}

// ── Integration: MemoryRouter with ExperienceGraph ────────────

#[test]
fn test_memory_router_with_populated_graph() {
    let mut graph = ExperienceGraph::new();
    
    let sig = vec![0.5, 0.5, 1.0, 0.0, 0.0];
    graph.add_experience(
        miniagent_self_improve::offline::experience_graph::NodeType::SuccessPattern,
        "Successfully wrote Python web scraper using bash and write tools",
        &vec!["Use bash for pip install".into(), "Use write for file creation".into()],
        &sig,
    );
    
    let router = MemoryRouter::new(
        Arc::new(std::sync::Mutex::new(graph)),
        Arc::new(std::sync::Mutex::new(QLearningRouter::new())),
        Arc::new(ColdStartKnowledgeBase::with_defaults()),
    );
    
    let ctx = router.retrieve("Write a Python web scraper to fetch product data");
    
    assert!(!ctx.relevant_successes.is_empty(), 
        "Should retrieve the similar success experience");
    assert!(ctx.relevant_successes[0].description.contains("web scraper"),
        "Retrieved experience should be relevant, got: {}",
        ctx.relevant_successes[0].description);
}

#[test]
fn test_memory_router_with_failure_experience() {
    let mut graph = ExperienceGraph::new();
    
    let sig = vec![0.3, 0.3, 0.0, 1.0, 0.0];
    graph.add_experience(
        miniagent_self_improve::offline::experience_graph::NodeType::FailurePattern,
        "Failed to find recent papers: web_search returned outdated results",
        &vec!["Use pubmed_search for scientific topics".into(), "Verify publication dates".into()],
        &sig,
    );
    
    let router = MemoryRouter::new(
        Arc::new(std::sync::Mutex::new(graph)),
        Arc::new(std::sync::Mutex::new(QLearningRouter::new())),
        Arc::new(ColdStartKnowledgeBase::with_defaults()),
    );
    
    let ctx = router.retrieve("Research recent papers on transformer architectures");
    
    assert!(!ctx.pitfalls.is_empty(),
        "Should retrieve the similar failure pattern");
    assert!(ctx.pitfalls[0].description.contains("web_search"),
        "Retrieved pitfall should mention the failing tool, got: {}",
        ctx.pitfalls[0].description);
}
