/// Re-exports and convenience wrappers for producing OpenTelemetry-compatible events.
pub use super::{AgentSpan, AgentSpanResult, ProviderSpan, ToolSpan, MemorySpan, WorkflowSpan};

use miniagent_core::types::RunId;

/// Context propagation: inject trace context into headers for distributed tracing
pub fn inject_context(run_id: &RunId) -> Vec<(&'static str, String)> {
    vec![
        ("x-trace-id", run_id.0.to_string()),
        ("x-span-id", uuid::Uuid::new_v4().to_string()),
    ]
}

