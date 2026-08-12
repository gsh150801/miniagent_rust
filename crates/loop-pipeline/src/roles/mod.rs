//! 三角色执行结构：Executor → Validator → Arbiter 协作循环。
//!
//! 每个子任务由三个角色协作完成：
//! - **Executor**（执行者）：执行任务、产出产物（含工具调用）
//! - **Validator**（校验员）：校验产出、指出不足（不修改产物）
//! - **Arbiter**（仲裁员）：综合产物+校验报告，决定 1️⃣补充 2️⃣修改 3️⃣通过
//!
//! 协作循环：Executor→Validator→Arbiter→(Revise/Supplement→重新Executor，或 Pass→完成)

pub mod arbiter;
pub mod validator;

pub use arbiter::{ArbiterDecision, run_arbiter, run_arbiter_forgiving};
pub use validator::{ValidationReport, run_validator};

use miniagent_core::error::AgentError;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

/// 三角色协作的完整结果。
#[derive(Debug, Clone)]
pub struct ThreeRoleResult {
    /// 执行者的最终产出
    pub executor_output: String,
    /// 校验员的报告
    pub validation: ValidationReport,
    /// 仲裁员的最终决策
    pub decision: ArbiterDecision,
    /// 经历的重试轮数（0=一次通过）
    pub rounds: usize,
}

/// 三角色协作执行一个子任务。
///
/// 流程：
/// 1. `executor_fn` 执行任务，产出 output
/// 2. `run_validator` 校验 output
/// 3. `run_arbiter` 决策
/// 4. 若 Revise/Supplement → 带反馈重新执行 executor_fn → 回到步骤 2
/// 5. 最多 `max_rounds` 轮（默认 2），超限强制 Pass
///
/// `executor_fn` 是一个异步闭包，接收可选的反馈文本（首轮为 None），
/// 返回执行者的 output 文本。这样允许调用方复用现有的 execute_single_task。
pub async fn execute_with_roles<F, Fut>(
    validator_provider: &dyn LlmProvider,
    arbiter_provider: &dyn LlmProvider,
    task_description: &str,
    expected_output: &str,
    max_rounds: usize,
    cancel: CancellationToken,
    mut executor_fn: F,
) -> Result<ThreeRoleResult, AgentError>
where
    F: FnMut(Option<&str>) -> Fut,
    Fut: std::future::Future<Output = Result<String, AgentError>>,
{
    let mut current_output = executor_fn(None).await?;
    let mut rounds = 0usize;

    loop {
        // Validator 校验
        let validation = run_validator(
            validator_provider,
            task_description,
            expected_output,
            &current_output,
            cancel.clone(),
        ).await?;

        // Arbiter 决策
        let decision = run_arbiter_forgiving(
            arbiter_provider,
            task_description,
            &current_output,
            &validation,
            cancel.clone(),
        ).await;

        // 判断是否完成
        if decision.is_pass() {
            return Ok(ThreeRoleResult {
                executor_output: current_output,
                validation,
                decision,
                rounds,
            });
        }

        // 未通过：检查是否还有重试额度
        rounds += 1;
        if rounds > max_rounds {
            tracing::warn!(
                rounds = rounds,
                max_rounds = max_rounds,
                "three-role loop exceeded max rounds — forcing pass"
            );
            return Ok(ThreeRoleResult {
                executor_output: current_output,
                validation,
                decision: ArbiterDecision::Pass, // 强制通过
                rounds,
            });
        }

        // 带反馈重新执行
        let feedback = decision.feedback().unwrap_or("Improve the output based on validation feedback.");
        tracing::info!(
            round = rounds,
            feedback = feedback,
            "arbiter requested revision — re-executing with feedback"
        );
        current_output = executor_fn(Some(feedback)).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniagent_provider::MockProvider;

    #[tokio::test]
    async fn test_execute_with_roles_pass_on_first_round() {
        // Validator 返回 passed=true → Arbiter 返回 Pass → 一次通过
        let validator = MockProvider::new(
            r#"{"passed":true,"issues":[],"severity":"minor","suggestions":[]}"#
        );
        let arbiter = MockProvider::new(r#"{"action":"pass"}"#);

        let result = execute_with_roles(
            &validator, &arbiter,
            "write report", "markdown report",
            2,
            CancellationToken::new(),
            |_feedback| async move { Ok("# Report\nDone".to_string()) },
        ).await.unwrap();

        assert!(result.decision.is_pass());
        assert_eq!(result.rounds, 0);
    }

    #[tokio::test]
    async fn test_execute_with_roles_loops_on_revise() {
        // Arbiter 第一次返回 Revise，第二次返回 Pass
        // 用两个不同的 MockProvider：validator 总是 passed=false
        let validator = MockProvider::new(
            r#"{"passed":false,"issues":["incomplete"],"severity":"major","suggestions":["add more"]}"#
        );
        let arbiter = MockProvider::new(r#"{"action":"pass"}"#);
        // 注：MockProvider 是静态的，无法第一轮返回 revise 第二轮返回 pass。
        // 这里测试的是"revise 后重新执行"——Arbiter 返回 pass 但 validator 说 false，
        // Arbiter 仍可能 pass（因为 severity 非 critical）。测试 executor_fn 被调用次数。

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = call_count.clone();

        let result = execute_with_roles(
            &validator, &arbiter,
            "task", "output",
            2,
            CancellationToken::new(),
            move |_feedback| {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok("output".to_string())
                }
            },
        ).await.unwrap();

        // Arbiter 返回 pass → executor 只调用一次
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(result.decision.is_pass());
    }

    #[tokio::test]
    async fn test_execute_with_roles_force_pass_after_max_rounds() {
        // Arbiter 始终返回 revise → 超过 max_rounds 后强制 pass
        let validator = MockProvider::new(
            r#"{"passed":false,"issues":["bad"],"severity":"critical","suggestions":[]}"#
        );
        let arbiter = MockProvider::new(r#"{"action":"revise","feedback":"redo"}"#);

        let result = execute_with_roles(
            &validator, &arbiter,
            "task", "output",
            1, // max_rounds=1
            CancellationToken::new(),
            |_feedback| async move { Ok("output".to_string()) },
        ).await.unwrap();

        // 超限后强制 pass
        assert!(result.decision.is_pass());
        assert!(result.rounds > 0);
    }
}
