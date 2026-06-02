use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Initialize the telemetry subscriber.
/// - `level`: tracing level filter (trace, debug, info, warn, error)
/// - `json_format`: true for JSON output (machine-readable), false for human-readable
pub fn init(level: &str) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(format!(
                "miniagent={level},tokio=warn,hyper=warn,reqwest=warn"
            ))
        });

    // JSON format for observability (machine-readable structured logs)
    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_target(true)
                .with_current_span(false)
                .with_thread_ids(false)
                .with_thread_names(false)
        )
        .init();
}
