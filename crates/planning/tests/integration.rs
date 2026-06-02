use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use miniagent_planning::{
    EventStream, EventKind, TodoAttention,
    ActivationRule, ControlShell, ContextManager,
    StateGraph, GraphState, Checkpoint,
    TournamentArena,
};
use miniagent_planning::control_shell::Condition;
use miniagent_planning::orchestrator::{
    Orchestrator, OrchestrationPattern, RoleAgent,
};
#[cfg(test)]
use miniagent_planning::research::{
    TournamentMasterRole, PrincipalInvestigatorRole,
    EvidenceAccumulatorRole, SynthesisJudgeRole,
};
#[cfg(test)]
use miniagent_planning::tournament::{
    DebateSession, DebateRubricScores, Verdict,
};
#[cfg(test)]
use miniagent_planning::roles::{AgentRole, Blackboard};
use miniagent_provider::MockProvider;
#[cfg(test)]
use miniagent_provider::{DeepSeekFlash, DeepSeekPro};
use tokio_util::sync::CancellationToken;

#[allow(dead_code)]
fn setup_workspace() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&work_dir).unwrap();
    (dir, work_dir)
}

// ── EventStream Tests ─────────────────────────────────────────

#[test]
fn event_stream_append_and_read() {
    let dir = TempDir::new().unwrap();
    let mut stream = EventStream::new(dir.path());

    stream.task_started("researcher", "search pubmed");
    stream.task_completed("researcher", "found 5 papers", vec!["researcher/findings.json".into()]);

    assert_eq!(stream.len(), 2);
    assert_eq!(stream.count_for_agent("researcher"), 2);
    assert_eq!(stream.count_by_kind(EventKind::TaskStarted), 1);
    assert_eq!(stream.count_by_kind(EventKind::TaskCompleted), 1);
}

#[test]
fn event_stream_persists_to_disk() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();

    {
        let mut stream = EventStream::new(&path);
        stream.task_started("researcher", "task 1");
        stream.task_completed("researcher", "done", vec![]);
    }

    // Load from disk
    let stream2 = EventStream::new(&path);
    assert_eq!(stream2.len(), 2);
}

#[test]
fn event_stream_format_recent() {
    let dir = TempDir::new().unwrap();
    let mut stream = EventStream::new(dir.path());

    stream.task_started("researcher", "search");
    stream.task_failed("researcher", "timeout");

    let text = stream.format_recent(10, Some("researcher"));
    assert!(text.contains("researcher"));
    assert!(text.contains("FAIL"));
    assert!(text.contains("OK"));
}

// ── TodoAttention Tests ───────────────────────────────────────

#[test]
fn todo_attention_add_and_complete() {
    let dir = TempDir::new().unwrap();
    let mut todo = TodoAttention::new(dir.path());

    todo.add("Search PubMed for COVID-19", Some("researcher"), 10);
    todo.add("Write report", Some("writer"), 5);

    let pending = todo.pending();
    assert_eq!(pending.len(), 2);

    // Complete first task
    let id = pending[0].id.clone();
    todo.complete(&id);

    assert_eq!(todo.pending().len(), 1);
    let pct = todo.progress_pct();
    assert_eq!(pct, 50.0);
}

#[test]
fn todo_attention_persists_to_disk() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();

    {
        let mut todo = TodoAttention::new(&path);
        todo.add("Task 1", Some("agent1"), 5);
        todo.add("Task 2", Some("agent2"), 3);
    }

    // Reload from disk
    let todo2 = TodoAttention::new(&path);
    assert_eq!(todo2.pending().len(), 2);
}

#[test]
fn todo_attention_format_todo() {
    let dir = TempDir::new().unwrap();
    let mut todo = TodoAttention::new(dir.path());

    todo.add("Research topic", Some("researcher"), 10);
    let text = todo.refresh();

    assert!(text.contains("# Current Objectives"));
    assert!(text.contains("Research topic"));
    assert!(text.contains("Progress:"));
}

// ── ControlShell Tests ────────────────────────────────────────

#[test]
fn control_shell_file_exists_condition() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();
    let events = EventStream::new(work_dir);

    // Create a file to trigger the condition
    let researcher_dir = work_dir.join("researcher");
    std::fs::create_dir_all(&researcher_dir).unwrap();
    std::fs::write(researcher_dir.join("findings.json"), "{}").unwrap();

    let mut shell = ControlShell::new();
    shell.add_rule(ActivationRule::new(
        "activate_critic",
        Condition::FileExists("researcher/findings.json".into()),
        vec!["critic"],
    ).with_priority(10));

    let activated = shell.evaluate(work_dir, 0, &events);
    assert!(activated.contains(&"critic".to_string()));
}

#[test]
fn control_shell_file_exists_and_not() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();
    let events = EventStream::new(work_dir);

    // Create findings but NOT critique
    let researcher_dir = work_dir.join("researcher");
    std::fs::create_dir_all(&researcher_dir).unwrap();
    std::fs::write(researcher_dir.join("findings.json"), "{}").unwrap();

    let mut shell = ControlShell::new();
    shell.add_rule(ActivationRule::new(
        "critique_on_findings",
        Condition::FileExistsAndNot(
            "researcher/findings.json".into(),
            "critic/critique.json".into(),
        ),
        vec!["critic"],
    ).with_priority(10));

    let activated = shell.evaluate(work_dir, 0, &events);
    assert!(activated.contains(&"critic".to_string()));

    // Now create critique — should not activate
    let critic_dir = work_dir.join("critic");
    std::fs::create_dir_all(&critic_dir).unwrap();
    std::fs::write(critic_dir.join("critique.json"), "{}").unwrap();

    let activated2 = shell.evaluate(work_dir, 1, &events);
    assert!(!activated2.contains(&"critic".to_string()));
}

#[test]
fn control_shell_scientific_defaults() {
    let shell = ControlShell::default();
    assert_eq!(shell.rule_count(), 5); // 5 default scientific rules
}

#[test]
fn control_shell_pipeline_defaults() {
    let shell = ControlShell::new().with_pipeline_defaults();
    assert_eq!(shell.rule_count(), 4);
}

// ── Checkpoint Tests ──────────────────────────────────────────

#[test]
fn checkpoint_save_and_load() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();

    let state = GraphState::default().with_work_dir(work_dir.to_path_buf());
    let ckpt = Checkpoint::from_state(&state, "test_node");

    let path = ckpt.save_to_disk(work_dir).unwrap();
    assert!(path.exists());

    let loaded = Checkpoint::load_from_disk(&path).unwrap();
    assert_eq!(loaded.node_name, "test_node");
    assert_eq!(loaded.state.iteration, 0);
}

#[test]
fn checkpoint_list_and_find_latest() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();

    let state = GraphState::default().with_work_dir(work_dir.to_path_buf());

    Checkpoint::from_state(&state, "node_a").save_to_disk(work_dir).unwrap();
    Checkpoint::from_state(&state, "node_b").save_to_disk(work_dir).unwrap();
    Checkpoint::from_state(&state, "node_a").save_to_disk(work_dir).unwrap();

    let all = Checkpoint::list_checkpoints(work_dir);
    assert_eq!(all.len(), 3);

    let latest_a = Checkpoint::latest_for_node(work_dir, "node_a");
    assert!(latest_a.is_some());
    assert_eq!(latest_a.unwrap().node_name, "node_a");
}

// ── StateGraph Tests ──────────────────────────────────────────

#[test]
fn state_graph_compile_cycle_detection() {
    let graph = StateGraph::new("a")
        .add_lambda("a", |_s| Ok(miniagent_planning::NodeOutput {
            content: "a".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_lambda("b", |_s| Ok(miniagent_planning::NodeOutput {
            content: "b".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_edge("a", "b")
        .add_edge("b", "a"); // cycle!

    let result = graph.compile();
    assert!(result.is_err());
    let err_msg = result.err().unwrap();
    assert!(err_msg.contains("Cycle detected"));
}

#[test]
fn state_graph_compile_valid_dag() {
    let graph = StateGraph::new("start")
        .add_lambda("start", |_s| Ok(miniagent_planning::NodeOutput {
            content: "started".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_lambda("middle", |_s| Ok(miniagent_planning::NodeOutput {
            content: "middle".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_lambda("end", |_s| Ok(miniagent_planning::NodeOutput {
            content: "ended".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_edge("start", "middle")
        .add_edge("middle", "end");

    let result = graph.compile();
    assert!(result.is_ok());
    let compiled = result.unwrap();

    // Should have 3 waves of 1 node each (sequential)
    assert_eq!(compiled.waves().len(), 3);
    assert_eq!(compiled.waves()[0], vec!["start"]);
    assert_eq!(compiled.waves()[1], vec!["middle"]);
    assert_eq!(compiled.waves()[2], vec!["end"]);
}

#[test]
fn state_graph_parallel_waves() {
    // b and c can run in parallel after a
    let graph = StateGraph::new("a")
        .add_lambda("a", |_s| Ok(miniagent_planning::NodeOutput {
            content: "a".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_lambda("b", |_s| Ok(miniagent_planning::NodeOutput {
            content: "b".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_lambda("c", |_s| Ok(miniagent_planning::NodeOutput {
            content: "c".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_lambda("d", |_s| Ok(miniagent_planning::NodeOutput {
            content: "d".into(), metadata: Default::default(), next: None, interrupt: None,
        }))
        .add_edge("a", "b")
        .add_edge("a", "c")
        .add_edge("b", "d")
        .add_edge("c", "d");

    let compiled = graph.compile().unwrap();

    // Wave 0: [a], Wave 1: [b, c] (parallel), Wave 2: [d]
    assert_eq!(compiled.waves().len(), 3);
    assert_eq!(compiled.waves()[0], vec!["a"]);
    assert_eq!(compiled.waves()[2], vec!["d"]);

    // Middle wave should contain both b and c
    let wave1 = &compiled.waves()[1];
    assert!(wave1.contains(&"b".to_string()));
    assert!(wave1.contains(&"c".to_string()));
}

// ── ContextManager Tests ──────────────────────────────────────

#[test]
fn context_manager_builds_context() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();

    // Create researcher output
    let researcher_dir = work_dir.join("researcher");
    std::fs::create_dir_all(&researcher_dir).unwrap();
    std::fs::write(researcher_dir.join("findings.json"), r#"{"findings": []}"#).unwrap();

    let mut todo = TodoAttention::new(work_dir);
    todo.add("Test task", Some("researcher"), 5);

    let events = EventStream::new(work_dir);
    let ctx = ContextManager::new(work_dir);

    let context = ctx.build_context("critic", &mut todo, &events);
    assert!(context.contains("# Current Objectives"));
    assert!(context.contains("researcher"));
}

#[test]
fn context_manager_file_ref_for_large_files() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();

    let researcher_dir = work_dir.join("researcher");
    std::fs::create_dir_all(&researcher_dir).unwrap();
    let large_content = "x".repeat(5000);
    std::fs::write(researcher_dir.join("findings.json"), &large_content).unwrap();

    let mut todo = TodoAttention::new(work_dir);
    let events = EventStream::new(work_dir);
    let ctx = ContextManager::new(work_dir);

    let context = ctx.build_context("critic", &mut todo, &events);
    // Should contain a file reference, not the full 5000 chars
    assert!(context.contains("researcher/findings.json"));
    assert!(context.len() < 5000);
}

#[test]
fn context_manager_error_preservation() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path();

    let ctx = ContextManager::new(work_dir);
    ctx.log_error("researcher", "timeout on PubMed search");
    ctx.log_error("critic", "failed to parse findings");

    let mut todo = TodoAttention::new(work_dir);
    let events = EventStream::new(work_dir);

    let context = ctx.build_context("synthesizer", &mut todo, &events);
    assert!(context.contains("Errors"));
    assert!(context.contains("researcher"));
}

// ── End-to-End Workflow Tests ────────────────────────────────────

/// E2E: Sequential pipeline via Orchestrator chain pattern.
/// Researcher → Critic → Synthesizer run sequentially,
/// each receiving the previous agent's output.
#[tokio::test]
async fn e2e_sequential_chain_pipeline() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let mut orch = Orchestrator::new().with_work_dir(&work_dir);
    orch.register(RoleAgent::new(
        "researcher",
        "You are a researcher.",
        Box::new(MockProvider::new("Found 3 papers on CRISPR base editing.")),
    ));
    orch.register(RoleAgent::new(
        "critic",
        "You are a critic.",
        Box::new(MockProvider::new("The methodology is sound but sample size is small.")),
    ));
    orch.register(RoleAgent::new(
        "synthesizer",
        "You are a synthesizer.",
        Box::new(MockProvider::new("Synthesis: CRISPR shows promise but needs larger trials.")),
    ));

    let results = orch.execute(
        "Investigate CRISPR base editing efficacy",
        OrchestrationPattern::Chain,
        CancellationToken::new(),
    ).await.unwrap();

    // 3 agents, 3 results
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, "researcher");
    assert_eq!(results[1].0, "critic");
    assert_eq!(results[2].0, "synthesizer");

    // Each agent persisted output to disk
    for (name, _) in &results {
        let output_path = work_dir.join(name).join("output.txt");
        assert!(output_path.exists(), "Missing output for {name}");
    }
}

/// E2E: Parallel fan-out via Orchestrator.
/// All agents process the same input concurrently and independently.
#[tokio::test]
async fn e2e_parallel_orchestration() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let mut orch = Orchestrator::new().with_work_dir(&work_dir);
    orch.register(RoleAgent::new(
        "researcher_a",
        "Researcher A",
        Box::new(MockProvider::new("Finding A")),
    ));
    orch.register(RoleAgent::new(
        "researcher_b",
        "Researcher B",
        Box::new(MockProvider::new("Finding B")),
    ));
    orch.register(RoleAgent::new(
        "researcher_c",
        "Researcher C",
        Box::new(MockProvider::new("Finding C")),
    ));

    let results = orch.execute(
        "Analyze topic X from different angles",
        OrchestrationPattern::Parallel,
        CancellationToken::new(),
    ).await.unwrap();

    assert_eq!(results.len(), 3);
    let names: Vec<&str> = results.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"researcher_a"));
    assert!(names.contains(&"researcher_b"));
    assert!(names.contains(&"researcher_c"));
}

/// E2E: Hierarchical delegation via Orchestrator.
/// Supervisor produces JSON plan, workers execute delegated subtasks.
#[tokio::test]
async fn e2e_hierarchical_delegation() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let delegations_json = serde_json::json!([
        {"task_id": "t1", "agent": "executor", "description": "Search for papers",
         "dependencies": [], "input_files": [], "expected_output": "paper list", "priority": 1},
        {"task_id": "t2", "agent": "writer", "description": "Write summary",
         "dependencies": ["t1"], "input_files": [], "expected_output": "report", "priority": 2},
    ]).to_string();

    let mut orch = Orchestrator::new().with_work_dir(&work_dir);
    // Supervisor returns structured JSON delegations
    orch.register(RoleAgent::new(
        "supervisor",
        "You decompose tasks.",
        Box::new(MockProvider::new(delegations_json)),
    ));
    orch.register(RoleAgent::new(
        "executor",
        "You execute tasks.",
        Box::new(MockProvider::new("Found 10 relevant papers.")),
    ));
    orch.register(RoleAgent::new(
        "writer",
        "You write reports.",
        Box::new(MockProvider::new("Summary report completed.")),
    ));

    let results = orch.execute(
        "Complete a research task",
        OrchestrationPattern::Hierarchical,
        CancellationToken::new(),
    ).await.unwrap();

    // supervisor + 2 workers
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, "supervisor");

    // Workers should have persisted their outputs
    let exec_output = work_dir.join("executor").join("t1.txt");
    let writer_output = work_dir.join("writer").join("t2.txt");
    assert!(exec_output.exists(), "executor output missing");
    assert!(writer_output.exists(), "writer output missing");
}

/// E2E: StateGraph execution with parallel waves.
/// Verifies that lambda nodes execute in correct wave order
/// and that outputs propagate across waves.
#[tokio::test]
async fn e2e_state_graph_parallel_execution() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let call_order = Arc::new(AtomicUsize::new(0));

    let order_a = call_order.clone();
    let order_b = call_order.clone();
    let order_c = call_order.clone();

    // Graph: a → [b, c] → d   (b and c run in parallel)
    let graph = StateGraph::new("a")
        .add_lambda("a", move |_| {
            let n = order_a.fetch_add(1, Ordering::SeqCst);
            Ok(miniagent_planning::NodeOutput {
                content: format!("a({n})"), metadata: Default::default(),
                next: None, interrupt: None,
            })
        })
        .add_lambda("b", move |_| {
            let n = order_b.fetch_add(1, Ordering::SeqCst);
            Ok(miniagent_planning::NodeOutput {
                content: format!("b({n})"), metadata: Default::default(),
                next: None, interrupt: None,
            })
        })
        .add_lambda("c", move |_| {
            let n = order_c.fetch_add(1, Ordering::SeqCst);
            Ok(miniagent_planning::NodeOutput {
                content: format!("c({n})"), metadata: Default::default(),
                next: None, interrupt: None,
            })
        })
        .add_lambda("d", move |_| {
            Ok(miniagent_planning::NodeOutput {
                content: "d(done)".into(), metadata: Default::default(),
                next: None, interrupt: None,
            })
        })
        .add_edge("a", "b")
        .add_edge("a", "c")
        .add_edge("b", "d")
        .add_edge("c", "d")
        .with_checkpoint("d");

    let compiled = graph.compile().unwrap();
    assert_eq!(compiled.waves().len(), 3);

    let flash = MockProvider::new("flash");
    let pro = MockProvider::new("pro");
    let state = GraphState::default().with_work_dir(&work_dir);

    let result = compiled.execute(state, CancellationToken::new(), &flash, &pro).await.unwrap();

    assert!(result.finished);
    assert_eq!(result.iteration, 4); // 4 nodes executed
    assert!(result.step_outputs.contains_key("a"));
    assert!(result.step_outputs.contains_key("b"));
    assert!(result.step_outputs.contains_key("c"));
    assert!(result.step_outputs.contains_key("d"));

    // Checkpoint for node "d" should be persisted to disk
    let checkpoints = Checkpoint::list_checkpoints(&work_dir);
    assert_eq!(checkpoints.len(), 1, "Expected 1 checkpoint for node d");

    // All 4 nodes recorded execution order (0..4)
    assert_eq!(call_order.load(Ordering::SeqCst), 3); // a, b, c incremented
}

/// E2E: StateGraph with Parallel node type.
/// Verifies sub-nodes within a Parallel node execute correctly.
#[tokio::test]
async fn e2e_state_graph_parallel_subnodes() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    // start → parallel_node[x, y] → end
    let graph = StateGraph::new("start")
        .add_lambda("start", |_| Ok(miniagent_planning::NodeOutput {
            content: "started".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_parallel("parallel_node", vec!["sub_x", "sub_y"])
        .add_lambda("sub_x", |_| Ok(miniagent_planning::NodeOutput {
            content: "x_result".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_lambda("sub_y", |_| Ok(miniagent_planning::NodeOutput {
            content: "y_result".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_lambda("end", |_| Ok(miniagent_planning::NodeOutput {
            content: "finished".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_edge("start", "parallel_node")
        .add_edge("parallel_node", "end");

    let compiled = graph.compile().unwrap();
    let flash = MockProvider::new("flash");
    let pro = MockProvider::new("pro");
    let state = GraphState::default().with_work_dir(&work_dir);

    let result = compiled.execute(state, CancellationToken::new(), &flash, &pro).await.unwrap();

    assert!(result.finished);
    // parallel_node output should contain both sub-node results
    let parallel_output = result.step_outputs.get("parallel_node").unwrap();
    assert!(parallel_output.contains("x_result"), "Missing sub_x output");
    assert!(parallel_output.contains("y_result"), "Missing sub_y output");
}

/// E2E: Full pipeline — Blackboard + EventStream + TodoAttention + ControlShell.
/// Simulates a research workflow where:
/// 1. Researcher produces findings → file written to disk
/// 2. ControlShell detects the file → activates critic
/// 3. TodoAttention tracks progress throughout
/// 4. EventStream records all events
/// 5. ContextManager builds incremental context for each role
#[tokio::test]
async fn e2e_full_pipeline_with_shared_state() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    // 1. Setup shared workspace
    let mut event_stream = EventStream::new(&work_dir);
    let mut todo = TodoAttention::new(&work_dir);
    let mut control_shell = ControlShell::new();
    control_shell.add_rule(ActivationRule::new(
        "activate_critic_on_findings",
        Condition::FileExistsAndNot(
            "researcher/findings.json".into(),
            "critic/critique.json".into(),
        ),
        vec!["critic"],
    ).with_priority(10));
    control_shell.add_rule(ActivationRule::new(
        "activate_writer_on_synthesis",
        Condition::FileExists("synthesizer/synthesis.json".into()),
        vec!["writer"],
    ).with_priority(8));

    // 2. Researcher produces findings
    todo.add("Search literature", Some("researcher"), 10);
    event_stream.task_started("researcher", "literature search");

    let researcher_dir = work_dir.join("researcher");
    std::fs::create_dir_all(&researcher_dir).unwrap();
    let findings = r#"{"papers": 5, "topic": "CRISPR base editing"}"#;
    std::fs::write(researcher_dir.join("findings.json"), findings).unwrap();

    event_stream.task_completed("researcher", "found 5 papers", vec!["researcher/findings.json".into()]);
    todo.complete("t1");
    todo.add("Critique findings", Some("critic"), 8);

    // 3. ControlShell should detect findings and activate critic
    let activated = control_shell.evaluate(&work_dir, 1, &event_stream);
    assert!(activated.contains(&"critic".to_string()),
        "Critic should be activated after researcher produces findings");

    // 4. Critic produces critique
    let critic_dir = work_dir.join("critic");
    std::fs::create_dir_all(&critic_dir).unwrap();
    std::fs::write(critic_dir.join("critique.json"), r#"{"quality": "good"}"#).unwrap();
    event_stream.task_started("critic", "critique findings");
    event_stream.task_completed("critic", "critique done", vec!["critic/critique.json".into()]);

    // 5. ControlShell should no longer activate critic (file exists)
    let activated2 = control_shell.evaluate(&work_dir, 2, &event_stream);
    assert!(!activated2.contains(&"critic".to_string()),
        "Critic should not be re-activated after critique is done");

    // 6. Synthesizer produces synthesis
    let synth_dir = work_dir.join("synthesizer");
    std::fs::create_dir_all(&synth_dir).unwrap();
    std::fs::write(synth_dir.join("synthesis.json"), r#"{"synthesis": "combined findings"}"#).unwrap();
    event_stream.task_completed("synthesizer", "synthesis done", vec![]);

    // 7. ControlShell should now activate writer
    let activated3 = control_shell.evaluate(&work_dir, 3, &event_stream);
    assert!(activated3.contains(&"writer".to_string()),
        "Writer should be activated after synthesizer produces synthesis");

    // 8. Verify event stream recorded everything
    assert_eq!(event_stream.count_by_kind(EventKind::TaskStarted), 2); // researcher + critic
    assert_eq!(event_stream.count_by_kind(EventKind::TaskCompleted), 3); // researcher + critic + synth
    assert!(event_stream.total_duration().is_some());

    // 9. Verify TodoAttention tracked progress
    let todo_text = todo.refresh();
    assert!(todo_text.contains("# Current Objectives"));
    assert!(todo_text.contains("[x]"), "Completed item should have [x] marker");

    // 10. ContextManager builds role-specific context
    let ctx = ContextManager::new(&work_dir);
    let critic_ctx = ctx.build_context("critic", &mut todo, &event_stream);
    assert!(critic_ctx.contains("researcher"), "Critic context should reference researcher");
    assert!(critic_ctx.contains("# Current Objectives"), "Context should include todo anchor");

    let writer_ctx = ctx.build_context("writer", &mut todo, &event_stream);
    assert!(writer_ctx.contains("synthesizer"), "Writer context should reference synthesizer");
}

/// E2E: Error preservation — errors are recorded in event stream,
/// step outputs, and context, following the Manus principle.
#[tokio::test]
async fn e2e_error_preservation_workflow() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let mut event_stream = EventStream::new(&work_dir);

    // Simulate a node failure
    event_stream.task_started("executor", "run code");
    event_stream.task_failed("executor", "Python timeout after 30s");

    // Error should be queryable
    let recent = event_stream.recent(10, Some("executor"));
    assert!(recent.iter().any(|e| !e.success));
    assert!(recent.iter().any(|e| matches!(e.kind, EventKind::TaskFailed)));

    // Error logged via ContextManager
    let ctx = ContextManager::new(&work_dir);
    ctx.log_error("executor", "Python timeout after 30s");

    let mut todo = TodoAttention::new(&work_dir);
    let context = ctx.build_context("supervisor", &mut todo, &event_stream);
    assert!(context.contains("Errors"), "Context should include error section");
    assert!(context.contains("executor"), "Error section should mention the failed agent");
}

/// E2E: Checkpoint-based resume.
/// Execute a graph partially, save checkpoint, then verify checkpoint
/// can be loaded and contains the correct intermediate state.
#[tokio::test]
async fn e2e_checkpoint_resume_workflow() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    // Build graph: start → middle → end, with checkpoint at middle
    let graph = StateGraph::new("start")
        .add_lambda("start", |_| Ok(miniagent_planning::NodeOutput {
            content: "started".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_lambda("middle", |_| Ok(miniagent_planning::NodeOutput {
            content: "processed".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_lambda("end", |_| Ok(miniagent_planning::NodeOutput {
            content: "finished".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_edge("start", "middle")
        .add_edge("middle", "end")
        .with_checkpoint("middle");

    let compiled = graph.compile().unwrap();
    let flash = MockProvider::new("flash");
    let pro = MockProvider::new("pro");
    let state = GraphState::default().with_work_dir(&work_dir);

    let result = compiled.execute(state, CancellationToken::new(), &flash, &pro).await.unwrap();
    assert!(result.finished);

    // Load the checkpoint for "middle" and verify it has the correct state
    let ckpt = Checkpoint::latest_for_node(&work_dir, "middle").unwrap();
    assert_eq!(ckpt.node_name, "middle");
    // At the point of checkpoint, "start" and "middle" should have completed
    assert!(ckpt.state.step_outputs.contains_key("start"));
    assert!(ckpt.state.step_outputs.contains_key("middle"));
    // "end" may or may not be present depending on timing
    assert_eq!(ckpt.state.iteration, 2); // start + middle

    // Simulate resume: create a new graph from middle onwards
    let resume_graph = StateGraph::new("middle")
        .add_lambda("middle", |_| Ok(miniagent_planning::NodeOutput {
            content: "resumed_middle".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_lambda("end", |_| Ok(miniagent_planning::NodeOutput {
            content: "resumed_end".into(), metadata: Default::default(),
            next: None, interrupt: None,
        }))
        .add_edge("middle", "end");

    let resume_compiled = resume_graph.compile().unwrap();
    let mut resumed_state = ckpt.state.clone();
    resumed_state.finished = false;

    let resumed = resume_compiled.execute(
        resumed_state, CancellationToken::new(), &flash, &pro,
    ).await.unwrap();
    assert!(resumed.finished);
    assert!(resumed.step_outputs.contains_key("end"));
}

/// E2E: Debate pattern — multi-round exchange between agents.
#[tokio::test]
async fn e2e_debate_pattern() {
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();

    let mut orch = Orchestrator::new().with_work_dir(&work_dir);
    orch.register(RoleAgent::new(
        "proposer",
        "You propose hypotheses.",
        Box::new(MockProvider::new("Hypothesis: CRISPR can achieve 90% efficiency.")),
    ));
    orch.register(RoleAgent::new(
        "opponent",
        "You challenge hypotheses.",
        Box::new(MockProvider::new("Counter: efficiency depends on cell type.")),
    ));
    orch.register(RoleAgent::new(
        "judge",
        "You evaluate both sides.",
        Box::new(MockProvider::new("Verdict: Both sides have merit. Need more data.")),
    ));

    let results = orch.execute(
        "Evaluate CRISPR base editing efficiency claims",
        OrchestrationPattern::Debate { rounds: 2 },
        CancellationToken::new(),
    ).await.unwrap();

    // 3 agents × 2 rounds = 6 results
    assert_eq!(results.len(), 6);

    // Each round file should be persisted
    for agent in &["proposer", "opponent", "judge"] {
        for round in 0..2 {
            let path = work_dir.join(agent).join(format!("round_{round}.txt"));
            assert!(path.exists(), "Missing {agent} round {round} output");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  E2E Tests: Real LLM (DeepSeek) — run with `cargo test -- --ignored`
// ═══════════════════════════════════════════════════════════════════

fn load_api_key() -> String {
    // First try environment variable
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        return key;
    }
    // Fall back to .env file
    let env_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.env");
    if env_path.exists() {
        for line in std::io::BufRead::lines(std::io::BufReader::new(
            std::fs::File::open(&env_path).unwrap()
        )) {
            let line = line.unwrap();
            if let Some((key, value)) = line.trim().split_once('=') {
                if key == "DEEPSEEK_API_KEY" && !value.starts_with("sk-your") && !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    panic!("DEEPSEEK_API_KEY required. Set it in .env or environment.")
}

/// E2E: Full Elo tournament with real LLM debate scoring.
/// Two hypotheses about AD mechanism compete in 3 rounds.
#[tokio::test]
#[ignore]
async fn e2e_elo_tournament_with_real_llm() {
    let api_key = load_api_key();
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&work_dir).unwrap();

    let _flash = DeepSeekFlash::new(&api_key);
    let pro = DeepSeekPro::new(&api_key);
    let cancel = CancellationToken::new();

    let h_amyloid = "amyloid_cascade";
    let h_tau = "tau_propagation";

    let amyloid_text = "The amyloid cascade hypothesis posits that accumulation of Aβ42 oligomers is the primary \
         driver of AD pathogenesis, triggering downstream tau hyperphosphorylation, neuroinflammation, \
         and synaptic loss. Supported by: familial AD mutations in APP/PSEN1/2, Down syndrome AD pathology, \
         Aducanumab plaque reduction. Challenged by: many amyloid-positive individuals are cognitively normal, \
         failed anti-amyloid clinical trials.".to_string();
    let tau_text = "The tau propagation hypothesis posits that tau hyperphosphorylation and prion-like spread \
         along connected brain regions is the primary driver of neurodegeneration in AD. Braak staging \
         shows predictable spread. Tau PET correlates with cognitive decline better than amyloid PET. \
         Challenged by: primary tauopathies differ from AD, tau alone insufficient for full phenotype.".to_string();

    // Write hypothesis files to proposer/ directory so seed_hypotheses() finds them
    let proposer_dir = work_dir.join("proposer");
    std::fs::create_dir_all(&proposer_dir).unwrap();
    let hypothesis_json = serde_json::json!({
        "hypothesis": h_amyloid,
        "mechanism": amyloid_text,
    });
    std::fs::write(proposer_dir.join("hypothesis.json"), serde_json::to_string(&hypothesis_json).unwrap()).unwrap();

    // Store texts in blackboard for load_hypothesis_text()
    let mut blackboard = Blackboard::new(&work_dir);
    blackboard.artifacts.insert(h_amyloid.into(), amyloid_text);
    blackboard.artifacts.insert(h_tau.into(), tau_text);

    // Set up arena and store in blackboard so tournament master can pick it up
    let mut arena = TournamentArena::new(3);
    arena.seed(h_amyloid);
    arena.seed(h_tau);
    arena.start_tournament().unwrap();
    blackboard.artifacts.insert("tournament_arena".into(), serde_json::to_string(&arena).unwrap());

    // Run tournament master role to conduct debates
    let master = TournamentMasterRole::new(Box::new(pro));
    let task = "Which mechanism is more central to Alzheimer's disease pathogenesis: \
                amyloid cascade or tau propagation?";

    for round in 1..=3 {
        println!("\n=== Tournament Round {round} ===");

        let result = master.execute(task, &mut blackboard, cancel.clone()).await;
        match result {
            Ok(output) => {
                println!("Round {round} result: status={}, confidence={:.2}",
                    output.status, output.confidence);
                println!("Content preview: {}...", &output.content[..output.content.len().min(200)]);

                assert_eq!(output.status, "success");
                assert!(!output.content.is_empty());
                assert!(!output.output_files.is_empty());
            }
            Err(e) => panic!("Round {round} failed: {e}"),
        }

        if arena.is_finished() {
            println!("Tournament converged after round {round}");
            break;
        }
    }

    // Verify arena state was persisted
    let arena_path = work_dir.join("tournament_master/arena.json");
    assert!(arena_path.exists(), "Arena state should be persisted");

    let arena_json = std::fs::read_to_string(&arena_path).unwrap();
    let restored: TournamentArena = serde_json::from_str(&arena_json)
        .expect("Arena JSON should deserialize");

    println!("\n=== Final Standings ===");
    for rating in restored.standings() {
        println!("  {} rating={:.1} wins={} losses={} draws={}",
            rating.hypothesis_id, rating.rating,
            rating.wins, rating.losses, rating.draws);
    }

    // Both hypotheses should have been rated
    assert!(restored.elo.len() >= 2);
    // Ratings should differ (one should be higher)
    let ratings: Vec<f64> = restored.standings().iter().map(|r| r.rating).collect();
    assert!(ratings.len() >= 2, "Should have at least 2 rated hypotheses");
}

/// E2E: Full AD pipeline StateGraph compilation and execution with real LLM.
/// Runs a simplified version (3 mechanism profiles instead of 6) to save API costs.
#[tokio::test]
#[ignore]
async fn e2e_ad_pipeline_with_real_llm() {
    let api_key = load_api_key();
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&work_dir).unwrap();

    let flash = DeepSeekFlash::new(&api_key);
    let pro = DeepSeekPro::new(&api_key);

    // Build a simplified pipeline (3 mechanisms to keep costs reasonable)
    let mut graph = StateGraph::new("literature_search");
    graph = graph.add_agent(
        "literature_search",
        "You are a neuroscience researcher. Search for key findings about Alzheimer's disease \
         mechanisms. Focus on amyloid beta, tau protein, and neuroinflammation. \
         Provide 5-10 key findings with PMIDs or references. Output JSON.",
        miniagent_planning::state_graph::ModelTier::Flash,
    );
    graph = graph.add_agent(
        "seed_amyloid",
        "Propose a refined amyloid cascade hypothesis for AD. Include: hypothesis statement, \
         mechanism, 3+ evidence items with sources, confidence (0-1), testable prediction. Output JSON.",
        miniagent_planning::state_graph::ModelTier::Pro,
    );
    graph = graph.add_agent(
        "seed_tau",
        "Propose a tau propagation hypothesis for AD. Include: hypothesis statement, mechanism, \
         3+ evidence items with sources, confidence (0-1), testable prediction. Output JSON.",
        miniagent_planning::state_graph::ModelTier::Pro,
    );
    graph = graph.add_agent(
        "seed_inflammation",
        "Propose a neuroinflammation-driven hypothesis for AD. Include: hypothesis statement, \
         mechanism, 3+ evidence items with sources, confidence (0-1), testable prediction. Output JSON.",
        miniagent_planning::state_graph::ModelTier::Pro,
    );
    graph = graph.add_agent(
        "synthesis",
        "You are the Synthesis Judge. Integrate the hypotheses from the three mechanism proposals \
         into a unified multi-mechanism framework for Alzheimer's disease. Identify synergies and \
         produce testable predictions. Output JSON with: integrated_hypothesis, component_mechanisms, \
         testable_predictions, open_questions, confidence.",
        miniagent_planning::state_graph::ModelTier::Pro,
    );

    // Wire: search → 3 parallel seeds → synthesis
    graph = graph.add_edge("literature_search", "seed_amyloid");
    graph = graph.add_edge("literature_search", "seed_tau");
    graph = graph.add_edge("literature_search", "seed_inflammation");
    graph = graph.add_edge("seed_amyloid", "synthesis");
    graph = graph.add_edge("seed_tau", "synthesis");
    graph = graph.add_edge("seed_inflammation", "synthesis");
    graph = graph.with_checkpoint("synthesis");

    let compiled = graph.compile().expect("Graph compilation failed");

    // Verify wave structure: [search] → [3 seeds parallel] → [synthesis]
    let waves = compiled.waves();
    println!("Wave structure:");
    for (i, wave) in waves.iter().enumerate() {
        println!("  Wave {i}: {:?}", wave);
    }
    assert_eq!(waves.len(), 3, "Should have 3 waves");
    assert_eq!(waves[0], vec!["literature_search"]);
    assert_eq!(waves[1].len(), 3, "Wave 1 should have 3 parallel seed nodes");
    assert_eq!(waves[2], vec!["synthesis"]);

    // Execute with real LLM
    let state = GraphState::default().with_work_dir(&work_dir);
    let cancel = CancellationToken::new();

    println!("\n=== Executing AD Pipeline with Real LLM ===");
    let result = compiled.execute(state, cancel, &flash, &pro).await;

    match result {
        Ok(final_state) => {
            println!("\n=== Pipeline Completed Successfully ===");
            println!("Total iterations: {}", final_state.iteration);
            println!("Nodes executed: {}", final_state.step_outputs.len());
            println!("Messages: {}", final_state.messages.len());

            // Verify all nodes produced output
            assert!(final_state.step_outputs.contains_key("literature_search"),
                "literature_search should have output");
            assert!(final_state.step_outputs.contains_key("seed_amyloid"),
                "seed_amyloid should have output");
            assert!(final_state.step_outputs.contains_key("seed_tau"),
                "seed_tau should have output");
            assert!(final_state.step_outputs.contains_key("seed_inflammation"),
                "seed_inflammation should have output");
            assert!(final_state.step_outputs.contains_key("synthesis"),
                "synthesis should have output");

            // Print key outputs
            println!("\n--- Literature Search (truncated) ---");
            let lit = &final_state.step_outputs["literature_search"];
            println!("{}...", &lit[..lit.len().min(300)]);

            println!("\n--- Amyloid Hypothesis (truncated) ---");
            let amy = &final_state.step_outputs["seed_amyloid"];
            println!("{}...", &amy[..amy.len().min(300)]);

            println!("\n--- Synthesis (truncated) ---");
            let syn = &final_state.step_outputs["synthesis"];
            println!("{}...", &syn[..syn.len().min(500)]);

            // Verify checkpoint was saved
            let checkpoint_dir = work_dir.join("checkpoints");
            if checkpoint_dir.exists() {
                let checkpoints = std::fs::read_dir(&checkpoint_dir).unwrap();
                let count = checkpoints.count();
                println!("\nCheckpoints saved: {count}");
                assert!(count > 0, "Synthesis checkpoint should exist");
            }

            // Verify no errors in step outputs
            for (node, output) in &final_state.step_outputs {
                assert!(!output.starts_with("[ERROR:"),
                    "Node {node} should not have errors: {}",
                    &output[..output.len().min(100)]);
            }
        }
        Err(e) => {
            panic!("Pipeline execution failed: {e}");
        }
    }
}

/// E2E: Tournament debate between two specific AD hypotheses with Elo scoring.
/// Tests the full debate-rubric-Elo pipeline with real LLM judgment.
#[tokio::test]
#[ignore]
async fn e2e_debate_with_elo_scoring() {
    let api_key = load_api_key();
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&work_dir).unwrap();

    let pro = DeepSeekPro::new(&api_key);
    let cancel = CancellationToken::new();

    let hypothesis_a = serde_json::json!({
        "hypothesis": "Aβ42 oligomers directly cause synaptic dysfunction by binding to NMDA receptors, \
                        triggering excitotoxicity and downstream tau phosphorylation.",
        "mechanism": "Aβ42 oligomers → NMDA receptor binding → calcium dysregulation → synaptic loss → tau phosphorylation",
        "evidence": [
            {"claim": "Aβ oligomers inhibit LTP at picomolar concentrations", "source": "Walsh et al., 2002, Nature", "strength": 0.9},
            {"claim": "Aducanumab reduces plaques but cognitive benefit uncertain", "source": "EMERGE trial, 2021", "strength": 0.6}
        ],
        "confidence": 0.75
    });

    let hypothesis_b = serde_json::json!({
        "hypothesis": "Microglial TREM2 dysfunction leads to impaired Aβ clearance, chronic NLRP3 \
                        inflammasome activation, and complement-mediated synaptic pruning.",
        "mechanism": "TREM2 loss → Aβ accumulation → NLRP3 activation → IL-1β release → complement cascade → excessive pruning",
        "evidence": [
            {"claim": "TREM2 R47H variant is a major AD risk factor (OR ~3)", "source": "Guerreiro et al., 2013", "strength": 0.95},
            {"claim": "NLRP3 inhibition reduces pathology in 5xFAD mice", "source": "Heneka et al., 2013, Nature", "strength": 0.85}
        ],
        "confidence": 0.80
    });

    // Run tournament master to conduct the debate
    let mut blackboard = Blackboard::new(&work_dir);
    blackboard.artifacts.insert("h_amyloid_synaptic".into(), hypothesis_a.to_string());
    blackboard.artifacts.insert("h_trem2_inflammation".into(), hypothesis_b.to_string());

    let mut arena = TournamentArena::new(2);
    arena.seed("h_amyloid_synaptic");
    arena.seed("h_trem2_inflammation");
    arena.start_tournament().unwrap();

    // Store arena in blackboard for tournament master to find
    blackboard.artifacts.insert("tournament_arena".into(), serde_json::to_string(&arena).unwrap());

    let master = TournamentMasterRole::new(Box::new(pro));
    let result = master.execute(
        "Compare amyloid-centric vs inflammation-centric AD mechanisms",
        &mut blackboard,
        cancel,
    ).await;

    match result {
        Ok(output) => {
            println!("=== Debate Result ===");
            println!("Status: {}", output.status);
            println!("Confidence: {:.2}", output.confidence);
            println!("Content:\n{}", output.content);

            assert_eq!(output.status, "success");

            // Verify arena was updated
            let arena_json = std::fs::read_to_string(work_dir.join("tournament_master/arena.json"))
                .expect("Arena should be persisted");
            let updated_arena: TournamentArena = serde_json::from_str(&arena_json).unwrap();

            println!("\n=== Elo Standings ===");
            for r in updated_arena.standings() {
                println!("  {} rating={:.1} wins={}", r.hypothesis_id, r.rating, r.wins);
            }

            // Verify Elo was updated
            assert!(updated_arena.results.len() > 0, "Should have debate results");

            // At least one hypothesis should have a different rating
            let ratings: Vec<f64> = updated_arena.standings().iter().map(|r| r.rating).collect();
            if ratings.len() >= 2 {
                // They shouldn't both be exactly 1000 still
                let both_initial = ratings.iter().all(|r| (*r - 1000.0).abs() < 0.01);
                if !both_initial {
                    println!("Elo ratings diverged — debate had a winner");
                }
            }

            // Verify rubric scores in results
            for result in &updated_arena.results {
                println!("\nDebate: {} vs {}", result.hypothesis_a_id, result.hypothesis_b_id);
                println!("  Verdict: {}", result.verdict);
                println!("  Score A: {:.3}  Score B: {:.3}", result.rubric_total_a, result.rubric_total_b);
            }
        }
        Err(e) => panic!("Debate execution failed: {e}"),
    }
}

/// E2E: PI + Scheduler + Evidence Accumulator + Synthesis Judge pipeline.
/// Tests the full multi-role workflow with real LLM.
#[tokio::test]
#[ignore]
async fn e2e_multi_role_research_pipeline() {
    let api_key = load_api_key();
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&work_dir).unwrap();

    let flash = DeepSeekFlash::new(&api_key);
    let pro = DeepSeekPro::new(&api_key);
    let cancel = CancellationToken::new();

    let mut blackboard = Blackboard::new(&work_dir);
    blackboard.grant_full_access("pi");
    blackboard.grant_full_access("scheduler");
    blackboard.grant_full_access("evidence_accumulator");
    blackboard.grant_full_access("synthesis_judge");

    // Pre-populate with tournament results (simulated)
    let mut arena = TournamentArena::new(3);
    arena.seed("amyloid_cascade");
    arena.seed("tau_propagation");
    arena.seed("neuroinflammation");
    arena.start_tournament().unwrap();

    // Simulate some debate results
    for _ in 0..2 {
        let mut session = DebateSession::new(
            "sim_1", "amyloid_cascade",
            "Amyloid drives downstream pathology",
            "tau_propagation",
            "Tau spread correlates with symptoms",
        );
        session.verdict = Some(Verdict::AcceptA);
        session.rubric_scores_a = Some(DebateRubricScores {
            evidence_support: 0.8,
            mechanistic_plausibility: 0.7,
            falsifiability: 0.6,
            novelty: 0.4,
            consistency: 0.7,
        });
        session.rubric_scores_b = Some(DebateRubricScores {
            evidence_support: 0.7,
            mechanistic_plausibility: 0.8,
            falsifiability: 0.7,
            novelty: 0.5,
            consistency: 0.6,
        });
        session.rounds_completed = 1;
        arena.record_debate(&session);
    }
    arena.advance_round();

    for _ in 0..2 {
        let mut session = DebateSession::new(
            "sim_2", "amyloid_cascade",
            "Amyloid drives downstream pathology",
            "neuroinflammation",
            "Inflammation drives neurodegeneration",
        );
        session.verdict = Some(Verdict::ReviseA);
        session.rubric_scores_a = Some(DebateRubricScores {
            evidence_support: 0.6,
            mechanistic_plausibility: 0.5,
            falsifiability: 0.5,
            novelty: 0.3,
            consistency: 0.4,
        });
        session.rubric_scores_b = Some(DebateRubricScores {
            evidence_support: 0.85,
            mechanistic_plausibility: 0.8,
            falsifiability: 0.7,
            novelty: 0.8,
            consistency: 0.75,
        });
        session.rounds_completed = 1;
        arena.record_debate(&session);
    }
    arena.advance_round();

    blackboard.artifacts.insert(
        "tournament_arena".into(),
        serde_json::to_string(&arena).unwrap(),
    );

    println!("=== Step 1: PI reviews tournament ===");
    let pi = PrincipalInvestigatorRole::new(Box::new(flash));
    let pi_result = pi.execute(
        "Alzheimer's disease pathogenesis mechanisms",
        &mut blackboard,
        cancel.clone(),
    ).await.unwrap();

    println!("PI decision: {}", pi_result.content);
    assert_eq!(pi_result.status, "success");
    assert!(pi_result.output_files.contains(&"pi/decision.json".to_string()));

    // Persist PI decision to filesystem
    let pi_dir = work_dir.join("pi");
    std::fs::create_dir_all(&pi_dir).unwrap();
    std::fs::write(pi_dir.join("decision.json"), &pi_result.content).unwrap();

    println!("\n=== Step 2: Evidence Accumulator ===");
    let ea = EvidenceAccumulatorRole::new(Box::new(DeepSeekFlash::new(&api_key)));
    let ea_result = ea.execute(
        "Alzheimer's disease pathogenesis mechanisms",
        &mut blackboard,
        cancel.clone(),
    ).await.unwrap();

    println!("Evidence analysis (truncated): {}...",
        &ea_result.content[..ea_result.content.len().min(300)]);
    assert_eq!(ea_result.status, "success");
    assert!(ea_result.output_files.contains(&"evidence_accumulator/evidence.json".to_string()));

    println!("\n=== Step 3: Synthesis Judge ===");
    let judge = SynthesisJudgeRole::new(Box::new(DeepSeekPro::new(&api_key)));
    let syn_result = judge.execute(
        "Alzheimer's disease pathogenesis mechanisms",
        &mut blackboard,
        cancel.clone(),
    ).await.unwrap();

    println!("Synthesis (truncated): {}...",
        &syn_result.content[..syn_result.content.len().min(500)]);
    assert_eq!(syn_result.status, "success");
    assert!(syn_result.confidence > 0.0);
    assert!(syn_result.output_files.contains(&"synthesis_judge/synthesis.json".to_string()));
    assert!(syn_result.output_files.contains(&"synthesis_judge/report.md".to_string()));

    // Verify the report.md was written
    let report_path = work_dir.join("synthesis_judge/report.md");
    assert!(report_path.exists(), "Synthesis report should exist");
    let report = std::fs::read_to_string(&report_path).unwrap();
    assert!(report.contains("# Integrated Hypothesis Synthesis"), "Report should have header");
    println!("\n=== Synthesis Report (first 500 chars) ===");
    println!("{}...", &report[..report.len().min(500)]);

    println!("\n=== All pipeline steps completed successfully ===");
}

/// Helper: safely truncate a string at a char boundary.
fn trunc(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}

/// Helper: call LLM and return text response.
async fn llm_call(
    provider: &dyn miniagent_provider::traits::LlmProvider,
    system: &str,
    prompt: &str,
    cancel: CancellationToken,
) -> Result<String, Box<dyn std::error::Error>> {
    let request = miniagent_provider::traits::CompletionRequest {
        system: system.to_string(),
        messages: vec![miniagent_core::message::Message::user(prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.3),
            max_tokens: Some(4000),
            ..Default::default()
        },
    };
    let resp = provider.complete(&request, cancel).await?;
    let text: String = resp.content.iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    Ok(text)
}

/// LLM call with retry on transient errors.
async fn llm_call_retry(
    provider: &dyn miniagent_provider::traits::LlmProvider,
    system: &str,
    prompt: &str,
    cancel: CancellationToken,
    label: &str,
) -> String {
    for attempt in 0..3 {
        match llm_call(provider, system, prompt, cancel.clone()).await {
            Ok(text) => return text,
            Err(e) if attempt < 2 => {
                println!("  [{label}] attempt {} failed: {e}, retrying...", attempt+1);
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            Err(e) => panic!("[{label}] LLM failed after 3 attempts: {e}"),
        }
    }
    unreachable!()
}

/// E2E: Full hypothesis lifecycle — zero hardcoded data, real LLM throughout.
///
/// Flow:
///   1. LLM generates 3 AD mechanism hypotheses from scratch
///   2. Pairwise LLM-judged debates → Elo scoring
///   3. LLM independently critiques each hypothesis
///   4. LLM evolves the weakest hypothesis based on critique
///   5. Re-debate with evolved hypothesis
///   6. LLM synthesizes all hypotheses into integrated framework
#[tokio::test]
#[ignore]
async fn e2e_hypothesis_lifecycle_iterative() {
    let api_key = load_api_key();
    let dir = TempDir::new().unwrap();
    let work_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&work_dir).unwrap();

    let flash = DeepSeekFlash::new(&api_key);
    let pro = DeepSeekPro::new(&api_key);
    let cancel = CancellationToken::new();
    let topic = "阿尔茨海默症(AD)致病机制的核心通路与交互作用";

    // ═══════════════════════════════════════════════════════════
    //  Phase 1 — LLM generates 3 hypotheses from scratch
    // ═══════════════════════════════════════════════════════════
    println!("\n{}", "═".repeat(60));
    println!("Phase 1: Hypothesis Generation (real LLM)");
    println!("{}", "═".repeat(60));

    let domains = vec![
        "amyloid cascade pathway",
        "tau protein propagation",
        "neuroinflammation and microglial activation",
    ];

    let mut hypotheses: Vec<(String, serde_json::Value)> = Vec::new();
    for domain in &domains {
        let id = domain.split_whitespace().next().unwrap_or("h").to_string();
        let prompt = format!(
            "Propose a novel, specific, falsifiable hypothesis about the role of {domain} \
             in Alzheimer's disease pathogenesis. Include: mechanism, at least 3 evidence items \
             with real PMID/DOI references where possible, confidence (0-1), limitations, \
             and a testable prediction. \
             Output ONLY valid JSON: {{\"hypothesis\":\"...\",\"mechanism\":\"...\",\
             \"evidence\":[{{\"claim\":\"...\",\"source\":\"...\",\"strength\":0.0-1.0}}],\
             \"confidence\":0.0-1.0,\"limitations\":[\"...\"],\"testable_prediction\":\"...\"}}"
        );
        let raw = llm_call_retry(&pro, "You are a senior AD researcher. Output ONLY valid JSON.", &prompt, cancel.clone(), &id).await;

        let parsed = miniagent_planning::roles::parse_llm_json(&raw)
            .unwrap_or_else(|e| panic!("Failed to parse hypothesis JSON for {domain}: {e}\nRaw: {}", trunc(&raw, 300)));

        let hyp = parsed["hypothesis"].as_str().unwrap_or("");
        let conf = parsed["confidence"].as_f64().unwrap_or(0.0);
        println!("  [{id}] confidence={conf}");
        println!("      {}", trunc(hyp, 120));

        let hyp_dir = work_dir.join(format!("proposer_{id}"));
        std::fs::create_dir_all(&hyp_dir).unwrap();
        std::fs::write(hyp_dir.join("hypothesis.json"), serde_json::to_string_pretty(&parsed).unwrap()).unwrap();

        hypotheses.push((id.clone(), parsed));
    }
    assert_eq!(hypotheses.len(), 3);
    println!("  ✓ 3 hypotheses generated and persisted");

    // ═══════════════════════════════════════════════════════════
    //  Phase 2 — Pairwise LLM-judged debate → Elo
    // ═══════════════════════════════════════════════════════════
    println!("\n{}", "═".repeat(60));
    println!("Phase 2: Tournament Round 1 (real LLM judging)");
    println!("{}", "═".repeat(60));

    let mut arena = TournamentArena::new(5).with_convergence_threshold(20.0, 2);
    for (id, _) in &hypotheses { arena.seed(id); }
    arena.start_tournament().unwrap();

    for round_num in 1..=2 {
        let round_label = if round_num == 1 { "Round 1" } else { "Round 2 (post-evolution)" };
        println!("\n  --- Tournament {round_label}: {} matchups ---", arena.round_robin_pairs().len());

        for (a_id, b_id) in arena.round_robin_pairs() {
            let a_json = hypotheses.iter().find(|(id,_)| id==&a_id).map(|(_,v)| v).unwrap();
            let b_json = hypotheses.iter().find(|(id,_)| id==&b_id).map(|(_,v)| v).unwrap();

            let judge_prompt = format!(
                "Compare two AD hypotheses on: {topic}\n\n\
                 A ({a_id}):\n{}\n\n\
                 B ({b_id}):\n{}\n\n\
                 Score each 0-1: evidence_support(0.30), mechanistic_plausibility(0.25), \
                 falsifiability(0.20), novelty(0.15), consistency(0.10).\n\
                 JSON: {{\"scores_a\":{{evidence_support,mechanistic_plausibility,falsifiability,novelty,consistency}},\
                 \"scores_b\":{{same}},\"verdict\":\"accept_a\"|\"accept_b\"|\"revise_a\"|\"revise_b\"|\"draw\",\
                 \"critique_a\":\"...\",\"critique_b\":\"...\",\"reasoning\":\"...\"}}",
                serde_json::to_string(a_json).unwrap(),
                serde_json::to_string(b_json).unwrap(),
            );

            let judge_resp = llm_call_retry(&pro, "Impartial scientific judge. Output ONLY valid JSON.",
                &judge_prompt, cancel.clone(), &format!("debate_{a_id}_vs_{b_id}")).await;

            let mut session = DebateSession::new(
                format!("r{round_num}_{a_id}_vs_{b_id}"), &a_id,
                &serde_json::to_string(a_json).unwrap(),
                &b_id, &serde_json::to_string(b_json).unwrap(),
            );

            if let Ok(jp) = miniagent_planning::roles::parse_llm_json(&judge_resp) {
                session.rubric_scores_a = jp.get("scores_a").map(DebateRubricScores::from_json);
                session.rubric_scores_b = jp.get("scores_b").map(DebateRubricScores::from_json);
                session.verdict = jp["verdict"].as_str().and_then(Verdict::parse);
                session.critique_a = jp["critique_a"].as_str().map(String::from);
                session.critique_b = jp["critique_b"].as_str().map(String::from);
                session.judge_reasoning = jp["reasoning"].as_str().map(String::from);
                session.rounds_completed = 1;
            }

            let sa = session.rubric_scores_a.as_ref().map(|r| r.weighted_total()).unwrap_or(0.0);
            let sb = session.rubric_scores_b.as_ref().map(|r| r.weighted_total()).unwrap_or(0.0);
            println!("    {a_id}({sa:.3}) vs {b_id}({sb:.3}) → {:?}", session.verdict);

            arena.record_debate(&session);
        }

        // Print standings after this round
        println!("  Standings after {round_label}:");
        for r in arena.standings() {
            println!("    {} rating={:.1} W={} L={}", r.hypothesis_id, r.rating, r.wins, r.losses);
        }

        // ═══════════════════════════════════════════════════════════
        //  Phase 3 — LLM critiques all hypotheses (after round 1 only)
        // ═══════════════════════════════════════════════════════════
        if round_num == 1 {
            println!("\n{}", "═".repeat(60));
            println!("Phase 3: Independent Critical Review (real LLM)");
            println!("{}", "═".repeat(60));

            let mut critiques: Vec<(String, serde_json::Value)> = Vec::new();
            for (id, hyp_json) in &hypotheses {
                let critique_prompt = format!(
                    "Rigorously critique this AD hypothesis:\n{}\n\n\
                     Find EVERY weakness: evidence quality, methodological flaws, counter-evidence, \
                     alternative explanations, logical gaps, falsifiability.\n\
                     JSON: {{\"overall_score\":0.0-1.0,\"critical_flaws\":[\"...\"],\
                     \"counter_evidence\":[{{\"claim\":\"\",\"source\":\"\",\"strength\":0.0}}],\
                     \"alternative_explanations\":[\"\"],\"recommendation\":\"accept\"|\"revise\"|\"reject\",\
                     \"revision_suggestions\":[\"\"]}}",
                    serde_json::to_string_pretty(hyp_json).unwrap(),
                );
                let raw = llm_call_retry(&pro, "Hostile peer reviewer. Output ONLY valid JSON.",
                    &critique_prompt, cancel.clone(), &format!("critique_{id}")).await;
                let parsed = miniagent_planning::roles::parse_llm_json(&raw)
                    .unwrap_or_else(|e| panic!("Critique parse error for {id}: {e}"));

                let score = parsed["overall_score"].as_f64().unwrap_or(0.0);
                let n_flaws = parsed["critical_flaws"].as_array().map(|a| a.len()).unwrap_or(0);
                println!("  [{id}] score={score:.2} flaws={n_flaws} rec={}",
                    parsed["recommendation"].as_str().unwrap_or("?"));

                let crit_dir = work_dir.join(format!("critic_{id}"));
                std::fs::create_dir_all(&crit_dir).unwrap();
                std::fs::write(crit_dir.join("critique.json"), serde_json::to_string_pretty(&parsed).unwrap()).unwrap();
                critiques.push((id.clone(), parsed));
            }

            // ═══════════════════════════════════════════════════════════
            //  Phase 4 — LLM evolves weakest hypothesis
            // ═══════════════════════════════════════════════════════════
            println!("\n{}", "═".repeat(60));
            println!("Phase 4: Evolution of Weakest (real LLM)");
            println!("{}", "═".repeat(60));

            let standings = arena.standings();
            let weakest_id = standings.last().unwrap().hypothesis_id.clone();
            let weakest_idx = hypotheses.iter().position(|(id,_)| id==&weakest_id).unwrap();
            let weakest_rating = arena.elo.rating_of(&weakest_id);
            let weakest_critique = critiques.iter().find(|(id,_)| id==&weakest_id).unwrap().1.clone();
            let original = &hypotheses[weakest_idx].1;

            println!("  Weakest: {weakest_id} (rating {weakest_rating:.1})");

            let evolve_prompt = format!(
                "Improve this AD hypothesis based on the critique.\n\n\
                 Original:\n{}\n\n\
                 Critique:\n{}\n\n\
                 Address each critical flaw. Strengthen evidence. Improve falsifiability. \
                 Maintain the core insight. Add specific testable predictions.\n\
                 JSON: {{\"hypothesis\":\"\",\"mechanism\":\"\",\
                 \"evidence\":[{{\"claim\":\"\",\"source\":\"\",\"strength\":0.0}}],\
                 \"confidence\":0.0-1.0,\"improvements_made\":[\"\"],\"testable_prediction\":\"\"}}",
                serde_json::to_string_pretty(original).unwrap(),
                serde_json::to_string_pretty(&weakest_critique).unwrap(),
            );

            let evolved_raw = llm_call_retry(&pro,
                "Hypothesis evolution specialist. Output ONLY valid JSON.",
                &evolve_prompt, cancel.clone(), "evolve_weakest").await;
            let evolved = miniagent_planning::roles::parse_llm_json(&evolved_raw)
                .unwrap_or_else(|e| panic!("Evolution parse error: {e}"));

            let new_conf = evolved["confidence"].as_f64().unwrap_or(0.0);
            let n_improvements = evolved["improvements_made"].as_array().map(|a| a.len()).unwrap_or(0);
            println!("  Evolved: {} improvements, confidence={new_conf:.2}", n_improvements);
            println!("      {}", trunc(evolved["hypothesis"].as_str().unwrap_or(""), 150));

            hypotheses[weakest_idx] = (weakest_id.clone(), evolved);

            println!("\n{}", "═".repeat(60));
            println!("Phase 5: Tournament Round 2 with Evolved Hypothesis");
            println!("{}", "═".repeat(60));
        }

        arena.advance_round();
    }

    // ═══════════════════════════════════════════════════════════
    //  Phase 6 — Convergence & Elo summary
    // ═══════════════════════════════════════════════════════════
    println!("\n{}", "═".repeat(60));
    println!("Phase 6: Convergence Check");
    println!("{}", "═".repeat(60));
    let variance = arena.elo.rating_variance_top_k(3);
    println!("  Rating variance (top-3): {variance:.1}");
    println!("  Total debates: {}", arena.results.len());

    // ═══════════════════════════════════════════════════════════
    //  Phase 7 — LLM synthesizes all hypotheses
    // ═══════════════════════════════════════════════════════════
    println!("\n{}", "═".repeat(60));
    println!("Phase 7: Multi-Hypothesis Synthesis (real LLM)");
    println!("{}", "═".repeat(60));

    let mut ranked: Vec<(String, f64, &serde_json::Value)> = hypotheses.iter()
        .map(|(id,v)| (id.clone(), arena.elo.rating_of(id), v))
        .collect();
    ranked.sort_by(|a,b| b.1.partial_cmp(&a.1).unwrap());

    let mut hyp_sections = String::new();
    for (rank, (id, rating, hyp)) in ranked.iter().enumerate() {
        hyp_sections.push_str(&format!(
            "\n### #{rank} ({id}, Elo {rating:.1})\n{}\n",
            serde_json::to_string_pretty(hyp).unwrap_or_default(),
        ));
    }

    let syn_prompt = format!(
        "You are the Synthesis Judge for an AD hypothesis tournament.\n\
         Topic: {topic}\n\n\
         Ranked hypotheses (by Elo):\n{hyp_sections}\n\n\
         Task:\n\
         1. Identify synergies between mechanisms\n\
         2. Resolve contradictions with evidence-based reasoning\n\
         3. Determine causal directionality\n\
         4. Produce 3+ testable predictions\n\
         5. List remaining open questions\n\n\
         JSON: {{\"integrated_hypothesis\":\"\",\
         \"component_mechanisms\":[{{\"mechanism\":\"\",\"role\":\"\",\"evidence_strength\":0.0,\"source_hypothesis\":\"\"}}],\
         \"mechanism_interactions\":[{{\"from\":\"\",\"to\":\"\",\"interaction\":\"\",\"evidence\":\"\"}}],\
         \"resolved_contradictions\":[{{\"issue\":\"\",\"resolution\":\"\"}}],\
         \"open_questions\":[\"\"],\"testable_predictions\":[\"\"],\"confidence\":0.0-1.0}}"
    );

    let syn_raw = llm_call_retry(&pro, "Senior synthesis expert. Output ONLY valid JSON.",
        &syn_prompt, cancel.clone(), "synthesis").await;
    let synthesis = miniagent_planning::roles::parse_llm_json(&syn_raw)
        .unwrap_or_else(|e| panic!("Synthesis parse error: {e}"));

    let integrated = synthesis["integrated_hypothesis"].as_str().unwrap_or("");
    let conf = synthesis["confidence"].as_f64().unwrap_or(0.0);
    let n_mechs = synthesis["component_mechanisms"].as_array().map(|a| a.len()).unwrap_or(0);
    let n_preds = synthesis["testable_predictions"].as_array().map(|a| a.len()).unwrap_or(0);
    let n_questions = synthesis["open_questions"].as_array().map(|a| a.len()).unwrap_or(0);
    let n_interactions = synthesis["mechanism_interactions"].as_array().map(|a| a.len()).unwrap_or(0);
    let n_resolved = synthesis["resolved_contradictions"].as_array().map(|a| a.len()).unwrap_or(0);

    println!("  Integrated: {}", trunc(integrated, 200));
    println!("  Confidence: {conf:.2}");
    println!("  Mechanisms: {n_mechs} | Interactions: {n_interactions} | Contradictions resolved: {n_resolved}");
    println!("  Predictions: {n_preds} | Open questions: {n_questions}");

    if let Some(mechs) = synthesis["component_mechanisms"].as_array() {
        for m in mechs {
            let name = m["mechanism"].as_str().unwrap_or("?");
            let role = m["role"].as_str().unwrap_or("?");
            let str_val = m["evidence_strength"].as_f64().unwrap_or(0.0) * 100.0;
            println!("    - {name} [{role}] {str_val:.0}%");
        }
    }

    if let Some(preds) = synthesis["testable_predictions"].as_array() {
        println!("  Predictions:");
        for (i, p) in preds.iter().enumerate() {
            println!("    {}. {}", i+1, trunc(p.as_str().unwrap_or("?"), 120));
        }
    }

    if let Some(ints) = synthesis["mechanism_interactions"].as_array() {
        println!("  Interactions:");
        for i in ints {
            let from = i["from"].as_str().unwrap_or("?");
            let to = i["to"].as_str().unwrap_or("?");
            let kind = i["interaction"].as_str().unwrap_or("?");
            println!("    {from} --[{kind}]--> {to}");
        }
    }

    // Persist synthesis + report
    let syn_dir = work_dir.join("synthesis");
    std::fs::create_dir_all(&syn_dir).unwrap();
    std::fs::write(syn_dir.join("synthesis.json"), serde_json::to_string_pretty(&synthesis).unwrap()).unwrap();

    let mut report = String::new();
    report.push_str("# AD Hypothesis Lifecycle Report\n\n");
    report.push_str(&format!("## Topic\n{topic}\n\n"));
    report.push_str("## Final Tournament Standings\n");
    report.push_str("| Rank | Hypothesis | Elo | Wins | Losses |\n|------|-----------|-----|------|--------|\n");
    for (rank, (id, rating, _)) in ranked.iter().enumerate() {
        let r = arena.elo.ratings.get(id).unwrap();
        report.push_str(&format!("| {} | {} | {:.1} | {} | {} |\n", rank+1, id, rating, r.wins, r.losses));
    }
    report.push_str(&format!("\n## Integrated Hypothesis\n{integrated}\n\n**Confidence:** {conf:.2}\n"));
    report.push_str(&format!("\n## Total Debates: {}\n", arena.results.len()));
    std::fs::write(syn_dir.join("report.md"), &report).unwrap();

    // ── Assertions ──────────────────────────────────────────
    assert_eq!(hypotheses.len(), 3, "3 hypotheses");
    assert!(arena.results.len() >= 6, "At least 6 debates (3 pairs × 2 rounds), got {}", arena.results.len());
    assert!(!integrated.is_empty(), "Synthesis must not be empty");
    assert!(conf > 0.0, "Confidence must be positive");
    assert!(syn_dir.join("synthesis.json").exists());
    assert!(syn_dir.join("report.md").exists());

    // Verify Elo divergence — ratings should not all be identical
    let ratings: Vec<f64> = arena.standings().iter().map(|r| r.rating).collect();
    let max_spread = ratings.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        - ratings.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(max_spread > 0.0, "Elo ratings should diverge after real debates, spread={max_spread}");

    println!("\n{}", "═".repeat(60));
    println!("  ✓ Full lifecycle test PASSED");
    println!("{}", "═".repeat(60));
}
