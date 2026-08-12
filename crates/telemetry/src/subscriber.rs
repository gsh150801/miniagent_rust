use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Initialize the telemetry subscriber.
///
/// 日志策略（用户要求：只记录报错 + 工具调用等重要信息，不记录 warning）：
/// - `miniagent` 整体只记 **error**（忽略 warn/info/debug）
/// - `tool_call` target 记到 **info**（工具调用的成功/失败/参数/结果，这是重要信息）
/// - 框架噪声（tokio/hyper/reqwest）压到 **error**
///
/// `level` 参数作为 miniagent 的 error 之上的覆盖级别（如调试时传 "debug"）。
pub fn init(level: &str) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            // tool_call target 单独放行到 info（工具调用是重要信息，需记录）
            tracing_subscriber::EnvFilter::new(format!(
                "miniagent={level},tool_call=info,tokio=error,hyper=error,reqwest=error"
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
