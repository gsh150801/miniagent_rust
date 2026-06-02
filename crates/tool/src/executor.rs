use std::sync::Arc;
use std::time::Instant;

use miniagent_core::error::AgentError;
use miniagent_core::types::ToolCallId;
use tokio_util::sync::CancellationToken;

use crate::approval::{ApprovalDecision, ApprovalHandler};
use crate::registry::ToolRegistry;
use crate::traits::{ToolClass, ToolContext, ToolOutput};

pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
    approval: Box<dyn ApprovalHandler>,
}

impl ToolExecutor {
    pub fn new(registry: ToolRegistry, approval: Box<dyn ApprovalHandler>) -> Self {
        Self {
            registry: Arc::new(registry),
            approval,
        }
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub async fn execute(
        &self,
        name: &str,
        input: &serde_json::Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let tool = self
            .registry
            .get(name)
            .ok_or_else(|| AgentError::ToolNotFound(name.to_string()))?;

        let class = tool.class();
        match self.approval.approve(name, input, class).await {
            ApprovalDecision::Allow => {}
            ApprovalDecision::Deny(reason) => {
                return Err(AgentError::PolicyDenied(format!(
                    "tool '{name}': {reason}"
                )));
            }
        }

        let start = Instant::now();
        let result = tool.execute(input.clone(), ctx, cancel).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(mut output) => {
                output.metadata = Some(crate::traits::ToolMetadata {
                    duration_ms,
                    is_error: false,
                });
                Ok(output)
            }
            Err(e) => {
                Ok(ToolOutput {
                    content: format!("Error: {e}"),
                    metadata: Some(crate::traits::ToolMetadata {
                        duration_ms,
                        is_error: true,
                    }),
                })
            }
        }
    }

    /// Execute a batch of tool calls. ReadOnly tools run in parallel, Mutating sequentially.
    pub async fn execute_batch(
        &self,
        calls: &[ToolCallRequest],
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Vec<(ToolCallId, ToolOutput)> {
        let (reads, writes): (Vec<_>, Vec<_>) = calls
            .iter()
            .partition(|c| self.is_readonly(&c.name));

        let mut results = Vec::new();

        // Parallel ReadOnly
        if !reads.is_empty() {
            let read_futures: Vec<_> = reads
                .iter()
                .map(|c| {
                    let name = c.name.clone();
                    let input = c.input.clone();
                    let id = c.id;
                    let token = cancel.child_token();
                    async move {
                        let result = self
                            .execute(&name, &input, ctx, token)
                            .await
                            .unwrap_or_else(|e| ToolOutput {
                                content: format!("Error: {e}"),
                                metadata: None,
                            });
                        (id, result)
                    }
                })
                .collect();

            let read_results = futures_util::future::join_all(read_futures).await;
            results.extend(read_results);
        }

        // Sequential Mutating
        for call in writes {
            let token = cancel.child_token();
            let result = self
                .execute(&call.name, &call.input, ctx, token)
                .await
                .unwrap_or_else(|e| ToolOutput {
                    content: format!("Error: {e}"),
                    metadata: None,
                });
            results.push((call.id, result));
        }

        results
    }

    fn is_readonly(&self, name: &str) -> bool {
        self.registry
            .get(name)
            .is_some_and(|t| t.class() == ToolClass::ReadOnly)
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub id: ToolCallId,
    pub name: String,
    pub input: serde_json::Value,
}
