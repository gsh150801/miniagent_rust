use async_trait::async_trait;
use crate::traits::ToolClass;

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Allow,
    Deny(String),
}

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn approve(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        class: ToolClass,
    ) -> ApprovalDecision;
}

pub struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn approve(&self, _: &str, _: &serde_json::Value, _: ToolClass) -> ApprovalDecision {
        ApprovalDecision::Allow
    }
}

pub struct ReadOnlyAutoApprove;

#[async_trait]
impl ApprovalHandler for ReadOnlyAutoApprove {
    async fn approve(
        &self,
        _: &str,
        _: &serde_json::Value,
        class: ToolClass,
    ) -> ApprovalDecision {
        match class {
            ToolClass::ReadOnly => ApprovalDecision::Allow,
            ToolClass::Mutating => ApprovalDecision::Deny("Mutating tools require user approval".into()),
        }
    }
}
