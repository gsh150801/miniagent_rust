pub mod tracer;
pub mod metrics;
pub mod subscriber;

use miniagent_core::event::Usage;
use miniagent_core::types::{RunId, ToolCallId};
use std::time::{Duration, Instant};

/// Initialize the global telemetry subscriber.
/// Call once at startup.
pub fn init(level: &str) {
    subscriber::init(level);
}

// ── Spans ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AgentSpan {
    pub run_id: RunId,
    pub start: Instant,
    pub provider: String,
    pub complexity: String,
}

impl AgentSpan {
    pub fn start(run_id: RunId, provider: &str, complexity: &str) -> Self {
        tracing::info!(
            target: "miniagent.agent",
            run_id = %run_id.0,
            provider = %provider,
            complexity = %complexity,
            event = "agent.run.started"
        );
        Self {
            run_id,
            start: Instant::now(),
            provider: provider.to_string(),
            complexity: complexity.to_string(),
        }
    }

    pub fn finish(self, usage: &Usage, error: Option<&str>) -> AgentSpanResult {
        let elapsed = self.start.elapsed();
        match error {
            Some(e) => {
                tracing::error!(
                    target: "miniagent.agent",
                    run_id = %self.run_id.0,
                    provider = %self.provider,
                    duration_ms = elapsed.as_millis(),
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    error = %e,
                    event = "agent.run.failed"
                );
            }
            None => {
                tracing::info!(
                    target: "miniagent.agent",
                    run_id = %self.run_id.0,
                    provider = %self.provider,
                    duration_ms = elapsed.as_millis(),
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    total_tokens = usage.total_tokens(),
                    event = "agent.run.completed"
                );
            }
        }
        metrics::record_agent_run(elapsed, usage);
        AgentSpanResult { elapsed, usage: usage.clone() }
    }
}

pub struct AgentSpanResult {
    pub elapsed: Duration,
    pub usage: Usage,
}

// ── Tool Spans ────────────────────────────────────────────────

pub struct ToolSpan {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub start: Instant,
}

impl ToolSpan {
    pub fn start(call_id: ToolCallId, tool_name: &str) -> Self {
        tracing::debug!(
            target: "miniagent.tool",
            call_id = %call_id.0,
            tool = %tool_name,
            event = "tool.execution.started"
        );
        Self {
            call_id,
            tool_name: tool_name.to_string(),
            start: Instant::now(),
        }
    }

    pub fn finish(self, success: bool, error: Option<&str>) {
        let elapsed = self.start.elapsed();
        if success {
            tracing::debug!(
                target: "miniagent.tool",
                call_id = %self.call_id.0,
                tool = %self.tool_name,
                duration_ms = elapsed.as_millis(),
                event = "tool.execution.completed"
            );
        } else {
            tracing::warn!(
                target: "miniagent.tool",
                call_id = %self.call_id.0,
                tool = %self.tool_name,
                duration_ms = elapsed.as_millis(),
                error = %error.unwrap_or("unknown"),
                event = "tool.execution.failed"
            );
        }
        metrics::record_tool_execution(&self.tool_name, elapsed, success);
    }
}

// ── Provider Spans ────────────────────────────────────────────

pub struct ProviderSpan {
    pub provider: String,
    pub start: Instant,
}

impl ProviderSpan {
    pub fn start(provider: &str) -> Self {
        tracing::debug!(
            target: "miniagent.provider",
            provider = %provider,
            event = "provider.call.started"
        );
        Self {
            provider: provider.to_string(),
            start: Instant::now(),
        }
    }

    pub fn finish(self, usage: &Usage, success: bool) {
        let elapsed = self.start.elapsed();
        if success {
            tracing::debug!(
                target: "miniagent.provider",
                provider = %self.provider,
                duration_ms = elapsed.as_millis(),
                input_tokens = usage.input_tokens,
                output_tokens = usage.output_tokens,
                event = "provider.call.completed"
            );
        }
        metrics::record_provider_call(&self.provider, elapsed, usage);
    }
}

// ── Memory Spans ──────────────────────────────────────────────

pub struct MemorySpan {
    pub operation: String,
    pub start: Instant,
}

impl MemorySpan {
    pub fn start(operation: &str) -> Self {
        tracing::debug!(
            target: "miniagent.memory",
            operation = %operation,
            event = "memory.operation.started"
        );
        Self {
            operation: operation.to_string(),
            start: Instant::now(),
        }
    }

    pub fn finish(self, items: usize) {
        let elapsed = self.start.elapsed();
        tracing::debug!(
            target: "miniagent.memory",
            operation = %self.operation,
            duration_ms = elapsed.as_millis(),
            items = items,
            event = "memory.operation.completed"
        );
    }
}

// ── Workflow Spans ────────────────────────────────────────────

pub struct WorkflowSpan {
    pub name: String,
    pub total_stages: usize,
    pub start: Instant,
}

impl WorkflowSpan {
    pub fn start(name: &str, total_stages: usize) -> Self {
        tracing::info!(
            target: "miniagent.workflow",
            workflow = %name,
            total_stages = total_stages,
            event = "workflow.started"
        );
        Self {
            name: name.to_string(),
            total_stages,
            start: Instant::now(),
        }
    }

    pub fn stage_completed(&self, stage: &str, index: usize, duration_ms: u64) {
        tracing::info!(
            target: "miniagent.workflow",
            workflow = %self.name,
            stage = %stage,
            stage_index = index,
            stage_duration_ms = duration_ms,
            event = "workflow.stage_completed"
        );
    }

    pub fn finish(self, success: bool) {
        let elapsed = self.start.elapsed();
        if success {
            tracing::info!(
                target: "miniagent.workflow",
                workflow = %self.name,
                duration_ms = elapsed.as_millis(),
                event = "workflow.completed"
            );
        } else {
            tracing::error!(
                target: "miniagent.workflow",
                workflow = %self.name,
                duration_ms = elapsed.as_millis(),
                event = "workflow.failed"
            );
        }
    }
}
