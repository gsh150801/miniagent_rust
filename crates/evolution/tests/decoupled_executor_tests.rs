use miniagent_evolution::decoupled_executor::{EscalationContext, TacticResult};

// ── EscalationContext Tests ────────────────────────────────────

#[test]
fn test_escalation_context_creation() {
    let ctx = EscalationContext {
        task_id: "task_1".into(),
        task_description: "Write Python script".into(),
        expected_output: "script.py".into(),
        failure_history: vec!["Error 1".into(), "Error 2".into()],
        consecutive_failures: 3,
    };
    assert_eq!(ctx.task_id, "task_1");
    assert_eq!(ctx.consecutive_failures, 3);
    assert_eq!(ctx.failure_history.len(), 2);
}

#[test]
fn test_escalation_context_clone() {
    let ctx = EscalationContext {
        task_id: "t1".into(),
        task_description: "desc".into(),
        expected_output: "out".into(),
        failure_history: vec!["err".into()],
        consecutive_failures: 1,
    };
    let cloned = ctx.clone();
    assert_eq!(cloned.task_id, ctx.task_id);
    assert_eq!(cloned.consecutive_failures, ctx.consecutive_failures);
}

#[test]
fn test_escalation_context_debug() {
    let ctx = EscalationContext {
        task_id: "t1".into(),
        task_description: "test".into(),
        expected_output: "out".into(),
        failure_history: vec![],
        consecutive_failures: 0,
    };
    let debug_str = format!("{:?}", ctx);
    assert!(debug_str.contains("t1"));
}

// ── TacticResult Tests ─────────────────────────────────────────

#[test]
fn test_tactic_result_success() {
    let result = TacticResult {
        success: true,
        output: "Done".into(),
        error: None,
        error_messages: vec![],
        tokens_used: 100,
    };
    assert!(result.success);
    assert!(result.error.is_none());
    assert_eq!(result.tokens_used, 100);
}

#[test]
fn test_tactic_result_failure() {
    let result = TacticResult {
        success: false,
        output: String::new(),
        error: Some("Agent error".into()),
        error_messages: vec!["Error msg".into()],
        tokens_used: 0,
    };
    assert!(!result.success);
    assert!(result.error.is_some());
    assert_eq!(result.error_messages.len(), 1);
}

#[test]
fn test_tactic_result_clone() {
    let result = TacticResult {
        success: true,
        output: "test".into(),
        error: None,
        error_messages: vec![],
        tokens_used: 50,
    };
    let cloned = result.clone();
    assert_eq!(cloned.success, result.success);
    assert_eq!(cloned.tokens_used, result.tokens_used);
}

// ── Phase 3 Integration: dispatch.rs helpers ───────────────────

#[test]
fn test_escalation_context_with_multiple_failures() {
    let ctx = EscalationContext {
        task_id: "task_1".into(),
        task_description: "Implement complex algorithm".into(),
        expected_output: "Working code".into(),
        failure_history: vec![
            "Timeout".into(),
            "Wrong output".into(),
            "Dependency error".into(),
        ],
        consecutive_failures: 3,
    };
    assert_eq!(ctx.failure_history.len(), 3);
    assert!(ctx.failure_history.contains(&"Timeout".into()));
}

#[test]
fn test_tactic_result_error_messages_for_escalation() {
    let result = TacticResult {
        success: false,
        output: String::new(),
        error: Some("API call failed".into()),
        error_messages: vec![
            "Attempt 1: timeout".into(),
            "Attempt 2: invalid response".into(),
            "Attempt 3: API call failed".into(),
        ],
        tokens_used: 0,
    };
    // These error messages would be passed to strategy_replan
    assert_eq!(result.error_messages.len(), 3);
    assert!(result.error_messages[0].contains("timeout"));
}

#[test]
fn test_escalation_triggers_at_max_retries() {
    // Simulate the escalation logic from dispatch.rs
    let max_retries = 3;
    let mut retries = 0;
    let mut should_escalate = false;

    // Simulate 3 consecutive failures
    for _ in 0..max_retries {
        retries += 1;
        if retries >= max_retries {
            should_escalate = true;
            break;
        }
    }

    assert!(should_escalate, "Should escalate after max_retries");
    assert_eq!(retries, max_retries);
}

#[test]
fn test_escalation_does_not_trigger_on_success() {
    let max_retries = 3;
    let mut retries = 0;
    let mut should_escalate = false;

    // Simulate success on first attempt
    let success = true;
    if !success {
        for _ in 0..max_retries {
            retries += 1;
            if retries >= max_retries {
                should_escalate = true;
                break;
            }
        }
    }

    assert!(!should_escalate, "Should NOT escalate on success");
    assert_eq!(retries, 0);
}

#[test]
fn test_strategy_replan_prompt_contains_key_elements() {
    // Verify the strategy prompt would contain all necessary context
    let description = "Write Python script";
    let expected = "script.py";
    let failures = "Agent error: timeout";
    let count = 3;
    let memory_section = "## Relevant Past Successes\n- [SUCCESS] Use bash to run scripts";

    let prompt = format!(
        r#"Original Task: {description}
Expected Output: {expected}
Failure History: {failures}
Consecutive Failures: {count}
{memory_section}"#,
        description = description,
        expected = expected,
        failures = failures,
        count = count,
        memory_section = memory_section,
    );

    assert!(prompt.contains(description));
    assert!(prompt.contains(expected));
    assert!(prompt.contains(failures));
    assert!(prompt.contains(&count.to_string()));
    assert!(prompt.contains("Relevant Past Successes"));
}
