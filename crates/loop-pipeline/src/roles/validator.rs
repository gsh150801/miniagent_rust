//! 校验员（Validator）：校验执行者的产出，指出不足，不修改产物。

use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::event::ContentBlock;
use miniagent_core::config::InferenceConfig;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// 校验报告：对执行者产出的质量评估。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// 是否通过校验（true=产出达标）
    pub passed: bool,
    /// 发现的不足之处
    pub issues: Vec<String>,
    /// 最严重的问题等级："minor" / "major" / "critical"
    pub severity: String,
    /// 改进建议
    pub suggestions: Vec<String>,
}

/// 运行校验员：单次 LLM 调用，输入执行者产物 + 任务描述，输出结构化校验报告。
///
/// Validator 不执行工具、不修改产物——它只读执行者的 output 文本，
/// 对照 task 描述和 expected_output 做质量评估。
pub async fn run_validator(
    provider: &dyn LlmProvider,
    task_description: &str,
    expected_output: &str,
    executor_output: &str,
    cancel: CancellationToken,
) -> Result<ValidationReport, AgentError> {
    let system = "You are a strict quality validator. Evaluate the executor's output against the task requirements. \
Be thorough and honest — if the output is incomplete, inaccurate, or misses requirements, report it. \
Respond in JSON: {\"passed\": bool, \"issues\": [string], \"severity\": \"minor|major|critical\", \"suggestions\": [string]}";

    let prompt = format!(
        "## Task\n{task_description}\n\n\
         ## Expected Output\n{expected_output}\n\n\
         ## Executor's Output\n{executor_output}\n\n\
         Evaluate whether the executor's output meets the task requirements. \
         Check for completeness, accuracy, and alignment with expected output."
    );

    let request = CompletionRequest {
        system: system.to_string(),
        messages: vec![Message::user(&prompt)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.1), // 低温度保证一致性
            max_tokens: Some(1500),
            ..Default::default()
        },
    };

    let resp = provider.complete(&request, cancel).await?;
    let text: String = resp.content.iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    // 解析 JSON（容错：LLM 可能加 markdown 包裹）
    let cleaned = text.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<ValidationReport>(cleaned)
        .map_err(|e| {
            tracing::error!(error = %e, raw = %text.chars().take(300).collect::<String>(), "validator JSON parse failed");
            AgentError::invalid_state(format!("Validator parse failed: {e}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniagent_provider::MockProvider;

    #[tokio::test]
    async fn test_validator_parses_passed_report() {
        let provider = MockProvider::new(r#"{"passed":true,"issues":[],"severity":"minor","suggestions":[]}"#);
        let report = run_validator(
            &provider, "write a report", "a markdown report",
            "# Report\nContent here",
            CancellationToken::new(),
        ).await.unwrap();
        assert!(report.passed);
        assert!(report.issues.is_empty());
    }

    #[tokio::test]
    async fn test_validator_detects_issues() {
        let provider = MockProvider::new(
            r#"{"passed":false,"issues":["missing conclusion"],"severity":"major","suggestions":["add conclusion section"]}"#
        );
        let report = run_validator(
            &provider, "write report with conclusion", "report + conclusion",
            "# Report\nno conclusion",
            CancellationToken::new(),
        ).await.unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.severity, "major");
    }
}
