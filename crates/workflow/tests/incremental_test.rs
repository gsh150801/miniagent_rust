use miniagent_workflow::engine::{Workflow, WorkflowState, StageResult, ReplanRequest};
use miniagent_workflow::stage::{Stage, StageContext, StageHandler, StageOutput, StageMetadata};
use miniagent_core::types::StageId;

// ── Helper stages for testing ──────────────────────────────────────

/// A stage that always succeeds, producing a deterministic output.
#[derive(Debug, Clone)]
pub struct PassStage {
    pub name: &'static str,
    pub response: &'static str,
}

impl PassStage {
    pub fn new(name: &'static str, response: &'static str) -> Self {
        Self { name, response }
    }
}

#[async_trait::async_trait]
impl StageHandler for PassStage {
    fn name(&self) -> &str { self.name }
    fn description(&self) -> &str { "Test pass-through stage" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, miniagent_workflow::stage::StageError> {
        Ok(StageOutput {
            data: serde_json::json!({ "response": self.response }),
            metadata: StageMetadata {
                duration_ms: 1,
                items_processed: 1,
                success: true,
                error: None,
            },
        })
    }
}

/// A stage that always fails with a retryable error.
#[derive(Debug, Clone)]
pub struct FailStage {
    pub name: &'static str,
    pub error_msg: &'static str,
    pub retryable: bool,
}

impl FailStage {
    pub fn new(name: &'static str, error_msg: &'static str, retryable: bool) -> Self {
        Self { name, error_msg, retryable }
    }
}

#[async_trait::async_trait]
impl StageHandler for FailStage {
    fn name(&self) -> &str { self.name }
    fn description(&self) -> &str { "Test failing stage" }

    async fn execute(&self, _ctx: &StageContext) -> Result<StageOutput, miniagent_workflow::stage::StageError> {
        if self.retryable {
            Err(miniagent_workflow::stage::StageError::Retryable(self.error_msg.into()))
        } else {
            Err(miniagent_workflow::stage::StageError::Failed(self.error_msg.into()))
        }
    }
}

// ── WorkflowState tests ────────────────────────────────────────────

#[test]
fn test_workflow_state_mark_completed() {
    let mut state = WorkflowState::new();
    let sid = StageId::new();
    let output = StageOutput {
        data: serde_json::json!({ "response": "hello" }),
        metadata: StageMetadata { duration_ms: 1, items_processed: 1, success: true, error: None },
    };

    state.mark_completed(sid, output.clone(), vec!["out.txt".into()]);

    assert!(state.completed_stages.contains_key(&sid));
    assert_eq!(state.completed_stages[&sid].data["response"], "hello");
    assert_eq!(state.artifacts.get(&sid), Some(&vec!["out.txt".into()]));
    assert!(!state.failed_stages.contains_key(&sid));
}

#[test]
fn test_workflow_state_mark_failed() {
    let mut state = WorkflowState::new();
    let sid = StageId::new();

    state.mark_failed(sid, "network timeout".into(), true);
    assert_eq!(state.failed_stages.get(&sid), Some(&"network timeout".into()));
    assert_eq!(state.retry_count(sid), 1);

    state.mark_failed(sid, "still failing".into(), true);
    assert_eq!(state.retry_count(sid), 2);

    // Non-retryable error should not increment retry count
    state.mark_failed(sid, "permanent error".into(), false);
    assert_eq!(state.retry_count(sid), 0); // reset because non-retryable
}

#[test]
fn test_workflow_state_artifacts_exist() {
    let mut state = WorkflowState::new();
    let sid = StageId::new();

    // No artifacts → always true (safe to skip)
    assert!(state.artifacts_exist(sid, "/tmp"));

    // With artifacts that exist
    let tmp = std::env::temp_dir();
    let test_file = tmp.join("wf_test_artifact.txt");
    std::fs::write(&test_file, "test").unwrap();
    state.artifacts.insert(sid, vec![test_file.display().to_string()]);
    assert!(state.artifacts_exist(sid, "/tmp"));

    // Clean up
    let _ = std::fs::remove_file(&test_file);
}

#[test]
fn test_workflow_state_remove_stage() {
    let mut state = WorkflowState::new();
    let sid = StageId::new();
    let output = StageOutput {
        data: serde_json::json!({ "response": "bye" }),
        metadata: StageMetadata { duration_ms: 1, items_processed: 1, success: true, error: None },
    };

    state.mark_completed(sid, output, vec![]);
    state.mark_failed(sid, "error".into(), true);
    state.retry_counts.insert(sid, 3);
    state.artifacts.insert(sid, vec!["f.txt".into()]);

    state.remove_stage(sid);

    assert!(!state.completed_stages.contains_key(&sid));
    assert!(!state.failed_stages.contains_key(&sid));
    assert!(!state.retry_counts.contains_key(&sid));
    assert!(!state.artifacts.contains_key(&sid));
}

// ── ReplanRequest tests ────────────────────────────────────────────

#[test]
fn test_replan_request_builder() {
    let req = ReplanRequest::new()
        .add_stage("step2", "llm", vec!["step1"])
        .remove_stage("step1")
        .add_edge("step2", "step3")
        .remove_edge("step1", "step3")
        .with_prompt("new prompt");

    assert_eq!(req.add_stages.len(), 1);
    assert_eq!(req.add_stages[0], ("step2".into(), "llm".into(), vec!["step1".into()]));
    assert_eq!(req.remove_stages, vec!["step1"]);
    assert_eq!(req.add_edges, vec![("step2".into(), "step3".into())]);
    assert_eq!(req.remove_edges, vec![("step1".into(), "step3".into())]);
    assert_eq!(req.new_prompt, Some("new prompt".into()));
}

// ── StageResult tests ──────────────────────────────────────────────

#[test]
fn test_stage_result_variants() {
    let completed = StageResult::Completed {
        output: StageOutput {
            data: serde_json::json!({}),
            metadata: StageMetadata { duration_ms: 1, items_processed: 1, success: true, error: None },
        },
        artifact_paths: vec!["a.txt".into()],
    };
    assert!(completed.is_success());
    assert!(!completed.is_failed());
    assert!(!completed.is_skipped());

    let failed = StageResult::Failed { error: "boom".into(), attempt: 2, retryable: false };
    assert!(!failed.is_success());
    assert!(failed.is_failed());
    assert!(!failed.is_skipped());

    let skipped = StageResult::Skipped { reason: "artifacts exist".into() };
    assert!(!skipped.is_success());
    assert!(!skipped.is_failed());
    assert!(skipped.is_skipped());
}

// ── Incremental workflow execution tests ───────────────────────────

/// Build a minimal 2-stage workflow: agent → critic
fn build_two_stage_wf() -> Workflow {
    let mut wf = Workflow::new("test_wf");
    let s1 = Stage::new("agent", PassStage::new("agent", "research result"));
    let s2 = Stage::new("critic", PassStage::new("critic", "critique result"));
    wf = wf.add_stage(s1).add_stage(s2);
    wf
}

#[tokio::test]
async fn test_incremental_run_all_success() {
    let mut wf = build_two_stage_wf();
    let state = WorkflowState::new();

    let result = wf.run_incremental(
        None,
        tokio_util::sync::CancellationToken::new(),
        None,
        state,
        3,
        |_state, _outputs, _task_dir| ReplanRequest::new(),
    ).await;

    assert!(result.is_ok(), "Expected success: {:?}", result.err());
    let wf_result = result.unwrap();
    assert_eq!(wf_result.total_stages, 2);
    assert_eq!(wf_result.stage_outputs.len(), 2);
}

#[tokio::test]
async fn test_incremental_run_with_failure_and_replan() {
    let mut wf = Workflow::new("test_fail");
    let s1 = Stage::new("step1", PassStage::new("step1", "result1"));
    let s2 = Stage::new("step2", FailStage::new("step2", "transient error", true));
    let s3 = Stage::new("step3", PassStage::new("step3", "result3"));
    wf = wf.add_stage(s1).add_stage(s2).add_stage(s3);
    // step1 → step2, step2 → step3
    let id1 = wf.stages[0].id;
    let id2 = wf.stages[1].id;
    let id3 = wf.stages[2].id;
    wf = wf.add_edge(id1, id2).add_edge(id2, id3);

    let state = WorkflowState::new();

    // On failure, replan removes step2 and wires step1 → step3 directly
    let result = wf.run_incremental(
        None,
        tokio_util::sync::CancellationToken::new(),
        None,
        state,
        2,
        |wf_state, _outputs, _task_dir| {
            assert!(wf_state.failed_stages.contains_key(&id2), "step2 should be in failed_stages");
            ReplanRequest::new()
                .remove_stage("step2")
                .add_edge("step1", "step3")
        },
    ).await;

    assert!(result.is_ok(), "Expected success after replan: {:?}", result.err());
    let wf_result = result.unwrap();
    // step1 + step3 = 2 stages completed (step2 was removed)
    assert_eq!(wf_result.stage_outputs.len(), 2, "Should have step1, step3");
}

#[tokio::test]
async fn test_incremental_run_max_loops_stops() {
    let mut wf = Workflow::new("test_max_loops");
    let s1 = Stage::new("always_fail", FailStage::new("always_fail", "permanent", false));
    wf = wf.add_stage(s1);

    let state = WorkflowState::new();

    let result = wf.run_incremental(
        None,
        tokio_util::sync::CancellationToken::new(),
        None,
        state,
        1, // max_loops = 1
        |_state, _outputs, _task_dir| ReplanRequest::new(),
    ).await;

    // Should return Ok even with failures (max loops reached)
    assert!(result.is_ok(), "Should return Ok when max loops reached: {:?}", result.err());
    let wf_result = result.unwrap();
    assert_eq!(wf_result.total_stages, 1);
    assert_eq!(wf_result.stage_outputs.len(), 0, "No stages should have succeeded");
}

#[tokio::test]
async fn test_incremental_run_artifact_reuse() {
    use std::fs;

    let tmp = std::env::temp_dir().join("wf_artifact_test");
    let _ = fs::create_dir_all(&tmp);
    let artifact_path = tmp.join("agent_output.json");
    fs::write(&artifact_path, r#"{"response": "cached"}"#).unwrap();

    let mut wf = Workflow::new("test_artifact");
    let s1 = Stage::new("agent", PassStage::new("agent", "fresh result"));
    let s2 = Stage::new("critic", PassStage::new("critic", "fresh critique"));
    wf = wf.add_stage(s1).add_stage(s2);

    let id1 = wf.stages[0].id;

    // Pretend agent already completed and its artifact exists on disk
    let mut state = WorkflowState::new();
    let agent_output = StageOutput {
        data: serde_json::json!({ "response": "cached", "artifacts": [artifact_path.display().to_string()] }),
        metadata: StageMetadata { duration_ms: 1, items_processed: 1, success: true, error: None },
    };
    state.mark_completed(id1, agent_output, vec![artifact_path.display().to_string()]);

    let result = wf.run_incremental(
        None,
        tokio_util::sync::CancellationToken::new(),
        None,
        state,
        2,
        |_state, _outputs, _task_dir| ReplanRequest::new(),
    ).await;

    assert!(result.is_ok(), "Expected success: {:?}", result.err());
    let wf_result = result.unwrap();
    // agent should be skipped (artifact exists), critic should run
    assert_eq!(wf_result.stage_outputs.len(), 2, "Both stages should be in outputs (agent reused, critic fresh)");
    // The agent output should be the cached one
    let agent_data = &wf_result.stage_outputs[&id1].data;
    assert_eq!(agent_data["response"], "cached", "Agent should reuse cached artifact");

    let _ = fs::remove_dir_all(&tmp);
}

// ── WorkflowResult tests ───────────────────────────────────────────

#[test]
fn test_workflow_result_debug() {
    use miniagent_workflow::engine::WorkflowResult;
    use miniagent_core::types::RunId;

    let result = WorkflowResult {
        run_id: RunId::new(),
        stage_outputs: std::collections::HashMap::new(),
        total_stages: 0,
    };
    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("WorkflowResult"));
}
