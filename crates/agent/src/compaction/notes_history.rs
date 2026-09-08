//! Codex 式 notes 状态记忆 + 历史归档策略。
//!
//! 上下文超限时：
//! 1. 状态记忆层（notes.json，模型经 write_note/notes.md 自发维护；
//!    缺失时机械提取 checkpoint 兜底）必然非空——每轮由 Agent::run
//!    注入 system prompt；
//! 2. 被裁剪历史**归档不销毁**（history_archive.jsonl，全局 item_id
//!    连续），可经 search_history 工具按需查回；
//! 3. 重建窗口 = 首条 + 重启提示 + 最近 N 条，**不调 LLM 摘要**。

use miniagent_core::message::Message;
use tokio_util::sync::CancellationToken;

use crate::Agent;
use crate::compaction::{CompactionOutcome, HistoryCompactor};
use crate::context::RunContext;

pub struct NotesHistoryCompactor;

impl HistoryCompactor for NotesHistoryCompactor {
    fn name(&self) -> &'static str {
        "notes_history"
    }

    async fn compact(
        &self,
        agent: &Agent,
        history: &mut Vec<Message>,
        context: &RunContext,
        _cancel: CancellationToken,
    ) -> CompactionOutcome {
        if context.working_dir.is_empty() {
            return CompactionOutcome::Declined;
        }
        let mut notes_block =
            miniagent_tool::tools::write_note::render_notes_block(&context.working_dir);
        // ── 机械 checkpoint 兜底（Codex auto_compact_fallback 思路）──
        // 模型没写过任何 note 时，从历史里机械提取状态（原始任务→goal、
        // 最后 assistant 陈述→progress、写过的文件→artifacts），保证
        // notes 层必然非空——否则重启后的新窗口除最近几条外对任务
        // 一无所知。
        if notes_block.is_empty() && history.len() > 2 {
            let checkpoint = Agent::mechanical_checkpoint(history);
            if checkpoint.as_object().is_some_and(|m| !m.is_empty()) {
                Agent::seed_notes_from_checkpoint(&context.working_dir, &checkpoint);
                notes_block = miniagent_tool::tools::write_note::render_notes_block(
                    &context.working_dir,
                );
                tracing::info!("mechanical checkpoint seeded into notes (model wrote none)");
            }
        }
        if notes_block.is_empty() {
            // notes 层完全不可用（连机械提取都拿不到状态）→ 放弃，
            // 由调用方回落 LLM 摘要。
            return CompactionOutcome::Declined;
        }

        let keep_recent = agent
            .keep_recent_msgs()
            .min(history.len().saturating_sub(1));
        let first = history.first().cloned();
        let keep = keep_recent;
        let cut = history.len().saturating_sub(keep);
        // 归档被裁剪的历史（原始 prompt 之后、保留窗口之前）——只移出
        // 窗口、不销毁（对齐 Codex history/read_item/search_contents）。
        let archive_ids =
            Agent::append_history_archive(&context.working_dir, &history[1..cut.max(1)]);
        let recent: Vec<Message> = history
            .iter()
            .rev()
            .take(keep)
            .cloned()
            .collect::<Vec<_>>();
        let mut rebuilt = Vec::with_capacity(recent.len() + 2);
        if let Some(msg) = first {
            rebuilt.push(msg);
        }
        rebuilt.push(Message::user(format!(
            "[上下文已重启：较早的 {} 条历史已归档（item_id {}–{}）。\
             任务状态见 system 中的 Task State Notes；\
             需要当时的技术细节时用 search_history 按关键词检索、\
             read_history_item 按 item_id 读取。\
             从中断处继续任务。]",
            archive_ids.len(),
            archive_ids.first().copied().unwrap_or(0),
            archive_ids.last().copied().unwrap_or(0),
        )));
        rebuilt.extend(recent);
        tracing::info!(
            archived = archive_ids.len(),
            "notes-driven context restart (history archived, no LLM summary call)"
        );
        *history = rebuilt;
        CompactionOutcome::Rebuilt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RunContext;
    use crate::test_support::CapturingProvider;
    use miniagent_core::settings::AppConfig;
    use std::sync::Arc;

    fn notes_mode_agent() -> Arc<Agent> {
        let provider = CapturingProvider::new();
        let mut cfg = AppConfig::default();
        cfg.agent_notes_mode = true;
        Arc::new(
            Agent::new(
                Box::new(provider.clone()) as Box<dyn miniagent_provider::traits::LlmProvider>,
                Box::new(provider.clone()) as Box<dyn miniagent_provider::traits::LlmProvider>,
            )
            .with_config(Arc::new(cfg)),
        )
    }

    fn oversized_history() -> Vec<Message> {
        let filler = format!("step detail {}", "x".repeat(6_000));
        let mut h = vec![Message::user("任务：验证 notes_history 压缩")];
        for i in 0..40 {
            h.push(Message::assistant_text(format!("step {i}: {filler}")));
        }
        h
    }

    /// notes_history 模式：无 notes 时机械兜底生成 notes.json，归档落盘，
    /// 重建窗口，且全程零 LLM 调用（区别于 llm_summary 模式）。
    #[tokio::test]
    async fn notes_mode_rebuilds_without_llm_and_archives() {
        let dir = std::env::temp_dir().join("miniagent_notes_compactor_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let agent = notes_mode_agent();
        let ctx = RunContext::new("system").with_working_dir(dir.to_string_lossy().to_string());
        let mut history = oversized_history();
        let before = history.len();

        let outcome = NotesHistoryCompactor
            .compact(&agent, &mut history, &ctx, CancellationToken::new())
            .await;

        assert_eq!(outcome, CompactionOutcome::Rebuilt);
        assert!(history.len() < before, "window must shrink");
        // notes.json 由机械兜底生成
        let notes = std::fs::read_to_string(dir.join("notes.json")).unwrap();
        assert!(notes.contains("goal"), "机械提取应包含 goal: {notes}");
        // 归档非空
        let archive = std::fs::read_to_string(dir.join("history_archive.jsonl")).unwrap();
        assert!(archive.lines().count() > 0, "archived items must exist");
        // 零 LLM 调用（provider 由测试闭包持有，这里通过重建同一 provider 验证）
        // CapturingProvider 状态在 agent 内部，无法直接读取——
        // 用间接证据：无 compaction_history.json（LLM 摘要必写它）。
        assert!(!dir.join("compaction_history.json").exists(),
            "notes 模式不得产生 LLM 摘要记录");
    }

    /// notes_history 无 working_dir → Declined（调用方回落 llm_summary）。
    #[tokio::test]
    async fn notes_mode_declines_without_working_dir() {
        let agent = notes_mode_agent();
        // RunContext::new 默认 working_dir 为进程当前目录——必须显式置空
        let ctx = RunContext::new("system").with_working_dir("");
        let mut history = oversized_history();
        let outcome = NotesHistoryCompactor
            .compact(&agent, &mut history, &ctx, CancellationToken::new())
            .await;
        assert_eq!(outcome, CompactionOutcome::Declined);
    }
}
