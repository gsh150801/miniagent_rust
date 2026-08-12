//! `SingleRoleRunner` — generic adapter that exposes any single
//! [`AgentRole`] through the unified [`StageDriver`] trait.
//!
//! Useful for CLI commands that want to invoke one role in isolation
//! (e.g. `miniagent role --name researcher --task "..."`). The debate and
//! plan runners above handle multi-role orchestration; this is the
//! single-role building block.

use crate::roles::{AgentRole, Blackboard, RoleOutput};
use miniagent_core::orchestration::{
    OrchestrationError, SideEffect, StageDriver, StageInput, StageOutcome,
};
use std::path::PathBuf;
use std::sync::Arc;

/// Adapter: any `Arc<dyn AgentRole>` → `StageDriver`.
pub struct SingleRoleRunner {
    role: Arc<dyn AgentRole>,
    work_dir: PathBuf,
}

impl SingleRoleRunner {
    pub fn new(role: Arc<dyn AgentRole>, work_dir: impl Into<PathBuf>) -> Self {
        Self {
            role,
            work_dir: work_dir.into(),
        }
    }

    fn extract_task(input: &StageInput) -> Result<String, OrchestrationError> {
        if let Some(s) = input.input.as_str() {
            return Ok(s.to_string());
        }
        if let Some(s) = input.input.get("task").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }
        if let Some(s) = input.input.get("prompt").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }
        Err(OrchestrationError::Plan(
            "SingleRoleRunner input must be a string or {\"task\":...}/{\"prompt\":...}".into(),
        ))
    }

    fn role_output_to_outcome(
        role_name: &str,
        output: &RoleOutput,
    ) -> (String, Vec<SideEffect>) {
        let summary = if output.status == "success" {
            format!("{role_name} succeeded")
        } else {
            format!("{role_name} failed: {}", output.status)
        };
        let effects: Vec<SideEffect> = output
            .output_files
            .iter()
            .map(|f| SideEffect::ArtifactWritten {
                key: f.clone(),
                path: f.clone(),
            })
            .collect();
        (summary, effects)
    }
}

#[async_trait::async_trait]
impl StageDriver for SingleRoleRunner {
    fn name(&self) -> &str {
        // Use the role's own name as the driver name (so logs read e.g.
        // "planning::researcher" instead of "planning::SingleRoleRunner").
        self.role.name()
    }

    async fn run(&self, input: StageInput) -> Result<StageOutcome, OrchestrationError> {
        let task = Self::extract_task(&input)?;
        let cancel = input.cancel.clone();
        let mut blackboard = Blackboard::new(&self.work_dir);
        let role_name = self.role.name().to_string();

        let output: RoleOutput = self
            .role
            .execute(&task, &mut blackboard, cancel)
            .await
            .map_err(|e| match e {
                miniagent_core::error::AgentError::Cancelled => OrchestrationError::Cancelled,
                other => OrchestrationError::Stage(format!("{role_name} failed: {other}")),
            })?;

        let data = serde_json::to_value(&output).unwrap_or_default();
        let (summary, side_effects) = Self::role_output_to_outcome(&role_name, &output);

        Ok(StageOutcome {
            data,
            summary,
            side_effects,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roles::RoleOutput;
    use async_trait::async_trait;
    use tokio_util::sync::CancellationToken;

    /// Trivial role used only for adapter testing.
    struct EchoRole;
    #[async_trait]
    impl AgentRole for EchoRole {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes the task back"
        }
        async fn execute(
            &self,
            task: &str,
            _bb: &mut Blackboard,
            _cancel: CancellationToken,
        ) -> Result<RoleOutput, miniagent_core::error::AgentError> {
            Ok(RoleOutput {
                content: format!("echo: {task}"),
                evidence: vec![],
                confidence: 1.0,
                metadata: Default::default(),
                output_files: vec!["echo/output.txt".into()],
                status: "success".into(),
            })
        }
    }

    #[tokio::test]
    async fn single_role_runner_invokes_role() {
        let runner = SingleRoleRunner::new(Arc::new(EchoRole), std::env::temp_dir());
        let outcome = runner
            .run(StageInput::new(
                "echo",
                serde_json::json!("hello"),
                CancellationToken::new(),
            ))
            .await
            .unwrap();
        assert_eq!(outcome.summary, "echo succeeded");
        assert!(outcome.summary.contains("succeeded"));
        // SideEffect captured the role's output file.
        assert!(outcome
            .side_effects
            .iter()
            .any(|s| matches!(s, SideEffect::ArtifactWritten { path, .. } if path == "echo/output.txt")));
    }

    #[test]
    fn extract_task_accepts_keys() {
        let cancel = CancellationToken::new();
        let a = StageInput::new("r", serde_json::json!("hi"), cancel.clone());
        assert_eq!(SingleRoleRunner::extract_task(&a).unwrap(), "hi");
        let b = StageInput::new("r", serde_json::json!({"task": "t"}), cancel.clone());
        assert_eq!(SingleRoleRunner::extract_task(&b).unwrap(), "t");
        let c = StageInput::new("r", serde_json::json!({"prompt": "p"}), cancel);
        assert_eq!(SingleRoleRunner::extract_task(&c).unwrap(), "p");
    }
}