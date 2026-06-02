use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use miniagent_core::event::Usage;

// ── Global Metrics Counters ───────────────────────────────────

static AGENT_RUNS: AtomicU64 = AtomicU64::new(0);
static AGENT_FAILURES: AtomicU64 = AtomicU64::new(0);
static TOOL_CALLS: AtomicU64 = AtomicU64::new(0);
static TOOL_FAILURES: AtomicU64 = AtomicU64::new(0);
static PROVIDER_CALLS: AtomicU64 = AtomicU64::new(0);
static TOTAL_INPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static TOTAL_OUTPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static LATENCY_SUM_MS: AtomicU64 = AtomicU64::new(0);

// Per-tool metrics
static WEB_SEARCH_CALLS: AtomicU64 = AtomicU64::new(0);
static PUBMED_CALLS: AtomicU64 = AtomicU64::new(0);
static FETCH_CALLS: AtomicU64 = AtomicU64::new(0);
static BASH_CALLS: AtomicU64 = AtomicU64::new(0);

pub fn record_agent_run(elapsed: Duration, usage: &Usage) {
    AGENT_RUNS.fetch_add(1, Ordering::Relaxed);
    LATENCY_SUM_MS.fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
    TOTAL_INPUT_TOKENS.fetch_add(usage.input_tokens as u64, Ordering::Relaxed);
    TOTAL_OUTPUT_TOKENS.fetch_add(usage.output_tokens as u64, Ordering::Relaxed);
}

pub fn record_agent_failure() {
    AGENT_FAILURES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_tool_execution(tool: &str, _elapsed: Duration, success: bool) {
    TOOL_CALLS.fetch_add(1, Ordering::Relaxed);
    if !success {
        TOOL_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
    match tool {
        "web_search" => { WEB_SEARCH_CALLS.fetch_add(1, Ordering::Relaxed); }
        "pubmed_search" => { PUBMED_CALLS.fetch_add(1, Ordering::Relaxed); }
        "web_fetch" => { FETCH_CALLS.fetch_add(1, Ordering::Relaxed); }
        "bash" => { BASH_CALLS.fetch_add(1, Ordering::Relaxed); }
        _ => {}
    };
}

pub fn record_provider_call(_provider: &str, _elapsed: Duration, usage: &Usage) {
    PROVIDER_CALLS.fetch_add(1, Ordering::Relaxed);
    TOTAL_INPUT_TOKENS.fetch_add(usage.input_tokens as u64, Ordering::Relaxed);
    TOTAL_OUTPUT_TOKENS.fetch_add(usage.output_tokens as u64, Ordering::Relaxed);
}

pub struct MetricsSnapshot {
    pub agent_runs: u64,
    pub agent_failures: u64,
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub provider_calls: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub avg_latency_ms: f64,
    pub web_search_calls: u64,
    pub pubmed_calls: u64,
    pub fetch_calls: u64,
    pub bash_calls: u64,
}

pub fn snapshot() -> MetricsSnapshot {
    let runs = AGENT_RUNS.load(Ordering::Relaxed);
    MetricsSnapshot {
        agent_runs: runs,
        agent_failures: AGENT_FAILURES.load(Ordering::Relaxed),
        tool_calls: TOOL_CALLS.load(Ordering::Relaxed),
        tool_failures: TOOL_FAILURES.load(Ordering::Relaxed),
        provider_calls: PROVIDER_CALLS.load(Ordering::Relaxed),
        total_input_tokens: TOTAL_INPUT_TOKENS.load(Ordering::Relaxed),
        total_output_tokens: TOTAL_OUTPUT_TOKENS.load(Ordering::Relaxed),
        avg_latency_ms: if runs > 0 { LATENCY_SUM_MS.load(Ordering::Relaxed) as f64 / runs as f64 } else { 0.0 },
        web_search_calls: WEB_SEARCH_CALLS.load(Ordering::Relaxed),
        pubmed_calls: PUBMED_CALLS.load(Ordering::Relaxed),
        fetch_calls: FETCH_CALLS.load(Ordering::Relaxed),
        bash_calls: BASH_CALLS.load(Ordering::Relaxed),
    }
}
