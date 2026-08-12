//! 仲裁员（Arbiter）：综合执行者产物和校验员报告，决定任务处置。

use miniagent_core::error::AgentError;
use miniagent_core::message::Message;
use miniagent_core::event::ContentBlock;
use miniagent_core::config::InferenceConfig;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::validator::ValidationReport;

/// 仲裁决策：综合 Executor 产物 + Validator 报告后对任务的处置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action")]
pub enum ArbiterDecision {
    /// 通过——任务完成，可交付
    #[serde(rename = "pass")]
    Pass,
    /// 修改——执行者需基于反馈重新执行
    #[serde(rename = "revise")]
    Revise {
        feedback: String,
    },
    /// 补充——执行者需在现有产物基础上补充内容
    #[serde(rename = "supplement")]
    Supplement {
        feedback: String,
    },
}

impl ArbiterDecision {
    /// 是否通过
    pub fn is_pass(&self) -> bool {
        matches!(self, ArbiterDecision::Pass)
    }

    /// 获取反馈文本（Revise/Supplement 的 feedback）
    pub fn feedback(&self) -> Option<&str> {
        match self {
            ArbiterDecision::Pass => None,
            ArbiterDecision::Revise { feedback } | ArbiterDecision::Supplement { feedback } => Some(feedback),
        }
    }
}

/// 运行仲裁员：单次 LLM 调用，输入执行者产物 + 校验员报告，输出决策。
///
/// 决策逻辑：
/// - Validator passed=true → 倾向 Pass
/// - Validator passed=false + severity=critical → 倾向 Revise
/// - Validator passed=false + severity=major → 倾向 Revise 或 Supplement
/// - Validator passed=false + severity=minor → 可能 Pass（小问题不阻塞）
pub async fn run_arbiter(
    provider: &dyn LlmProvider,
    task_description: &str,
    executor_output: &str,
    validation: &ValidationReport,
    cancel: CancellationToken,
) -> Result<ArbiterDecision, AgentError> {
    let system = "You are an arbiter. Based on the executor's output and the validator's report, decide the task's fate. \
Choose one action: \"pass\" (task is complete and deliverable), \"revise\" (executor must redo with changes), \
or \"supplement\" (executor must add missing content to existing output). \
Be decisive. Minor issues should not block delivery. \
Respond in JSON: {\"action\":\"pass\"} or {\"action\":\"revise\",\"feedback\":\"...\"} or {\"action\":\"supplement\",\"feedback\":\"...\"}";

    let validation_json = serde_json::to_string_pretty(validation).unwrap_or_default();

    let prompt = format!(
        "## Task\n{task_description}\n\n\
         ## Executor's Output\n{executor_output}\n\n\
         ## Validator's Report\n{validation_json}\n\n\
         Decide the action. If the output is mostly correct with only minor issues, pass it. \
         If there are major gaps, request revise or supplement with specific feedback."
    );

    let request = CompletionRequest {
        system: system.to_string(),
        messages: vec![Message::user(&prompt)],
        tools: vec![],
        config: InferenceConfig {
            temperature: Some(0.1),
            max_tokens: Some(800),
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

    let cleaned = text.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match serde_json::from_str::<ArbiterDecision>(cleaned) {
        Ok(d) => Ok(d),
        Err(e) => {
            tracing::error!(error = %e, raw = %text.chars().take(300).collect::<String>(), "arbiter JSON parse failed");
            Err(AgentError::invalid_state(format!("Arbiter parse failed: {e}")))
        }
    }
}

/// 更健壮的版本：解析失败时降级为 Pass（不阻塞流程）。
pub async fn run_arbiter_forgiving(
    provider: &dyn LlmProvider,
    task_description: &str,
    executor_output: &str,
    validation: &ValidationReport,
    cancel: CancellationToken,
) -> ArbiterDecision {
    match run_arbiter(provider, task_description, executor_output, validation, cancel).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "arbiter failed, defaulting to Pass");
            ArbiterDecision::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniagent_provider::MockProvider;

    #[test]
    fn test_arbiter_decision_serde() {
        let pass = ArbiterDecision::Pass;
        let json = serde_json::to_string(&pass).unwrap();
        assert_eq!(json, r#"{"action":"pass"}"#);

        let revise = ArbiterDecision::Revise { feedback: "fix X".into() };
        let json = serde_json::to_string(&revise).unwrap();
        let back: ArbiterDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(back, revise);
    }

    #[test]
    fn test_arbiter_decision_helpers() {
        assert!(ArbiterDecision::Pass.is_pass());
        assert!(!ArbiterDecision::Revise { feedback: "x".into() }.is_pass());
        assert_eq!(ArbiterDecision::Pass.feedback(), None);
        assert_eq!(ArbiterDecision::Supplement { feedback: "add Y".into() }.feedback(), Some("add Y"));
    }

    #[tokio::test]
    async fn test_run_arbiter_decides_pass() {
        let provider = MockProvider::new(r#"{"action":"pass"}"#);
        let validation = ValidationReport {
            passed: true, issues: vec![], severity: "minor".into(), suggestions: vec![],
        };
        let decision = run_arbiter_forgiving(
            &provider, "task", "good output", &validation,
            CancellationToken::new(),
        ).await;
        assert!(decision.is_pass());
    }

    #[tokio::test]
    async fn test_run_arbiter_decides_revise() {
        let provider = MockProvider::new(
            r#"{"action":"revise","feedback":"rewrite the conclusion section"}"#
        );
        let validation = ValidationReport {
            passed: false, issues: vec!["bad conclusion".into()],
            severity: "major".into(), suggestions: vec![],
        };
        let decision = run_arbiter_forgiving(
            &provider, "task", "output with bad conclusion", &validation,
            CancellationToken::new(),
        ).await;
        assert!(!decision.is_pass());
        assert_eq!(decision.feedback(), Some("rewrite the conclusion section"));
    }
}
