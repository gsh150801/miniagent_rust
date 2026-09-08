//! 传统 LLM 摘要压缩策略。
//!
//! 上下文超限时把被裁剪历史交给 LLM 生成四段式结构化摘要
//!（约束/决策/产物/待办），重建窗口为首条 + 摘要消息 + 最近 N 条。
//! 摘要同时持久化到记忆数据库、磁盘上下文文件与任务目录
//! compaction_history.json（可审计/可回放）。

use miniagent_core::message::Message;
use tokio_util::sync::CancellationToken;

use crate::Agent;
use crate::compaction::{CompactionOutcome, HistoryCompactor};
use crate::context::RunContext;

pub struct LlmSummaryCompactor;

impl HistoryCompactor for LlmSummaryCompactor {
    fn name(&self) -> &'static str {
        "llm_summary"
    }

    async fn compact(
        &self,
        agent: &Agent,
        history: &mut Vec<Message>,
        context: &RunContext,
        cancel: CancellationToken,
    ) -> CompactionOutcome {
        let keep_recent = agent
            .keep_recent_msgs()
            .min(history.len().saturating_sub(1));
        let discard_count = history.len().saturating_sub(keep_recent + 1);

        // Collect text from messages being discarded (owned, no borrow conflict)
        let discarded_text: String = history
            .iter()
            .skip(1)
            .take(discard_count)
            .map(|m| {
                let role = format!("{:?}", m.role);
                format!("[{role}] {}", m.text_content())
            })
            .collect::<Vec<_>>()
            .join("\n---\n");

        // Generate summary via LLM
        let summary = agent.summarize_discarded(&discarded_text, context, &cancel).await;

        // Persist to memory database
        if let Some(ref mem) = agent.memory() {
            let rec = miniagent_memory::types::StructuredSummary {
                raw_summary: summary.clone(),
                ..Default::default()
            };
            let _ = mem.store_paper_summary(
                "Context History Summary",
                &rec,
                &["context_summary".to_string()],
                None,
            );
        }

        // Persist to disk file
        Agent::save_context_file(&summary);

        // P-记忆机制 Layer A：压缩记录持久化到任务工作目录（append 列表，
        // 可审计/可回放）。working_dir 缺失（如部分测试）时静默跳过。
        if !context.working_dir.is_empty() {
            Agent::append_compaction_record(&context.working_dir, discard_count, &summary);
        }

        // Rebuild: prompt + summary + last N messages.
        // Tool-pair safety: the kept window must never START with a Tool
        // message — its matching assistant tool_use call may have been cut,
        // and providers (MiniMax/OpenAI strict) reject orphaned tool results
        // ("tool call result does not follow tool call", live 400).
        let first = history.first().cloned();
        let mut recent: Vec<Message> = history
            .iter()
            .rev()
            .take(keep_recent)
            .cloned()
            .collect::<Vec<_>>();
        while recent
            .first()
            .is_some_and(|m| m.role == miniagent_core::message::MessageRole::Tool)
        {
            // Grow leftwards to swallow the whole orphaned tool sequence.
            let start = history.len() - recent.len();
            if start == 0 {
                break;
            }
            recent.insert(0, history[start - 1].clone());
        }
        recent.reverse();
        // The window must also be a contiguous slice of the original history;
        // insert() above keeps contiguity (we prepend the immediately
        // preceding message). Re-derive from history to be safe:
        if history.len() >= recent.len() {
            recent = history[history.len() - recent.len()..].to_vec();
        }

        // ── 保留窗口体积预算（trim 压缩下限的修复）──────────────────
        // last-N 盲留窗口的单条巨消息（数百万字节的历史层漏网之鱼）会
        // 让 trim 后的请求仍然超 provider 窗口。这里按预算从头收缩窗口，
        // 再对超预算的单条 tool result 做历史层头部截断（Tool 消息截断
        // 是安全的：模型只需摘要；截断不破坏 tool_use/tool_result 配对）。
        const RETENTION_TOKEN_BUDGET: usize = 24_000;
        let mut window_tokens = miniagent_core::budget::estimate_history_tokens(&recent);
        while recent.len() > 2 && window_tokens > RETENTION_TOKEN_BUDGET {
            recent.remove(0);
            window_tokens = miniagent_core::budget::estimate_history_tokens(&recent);
        }
        for msg in recent.iter_mut() {
            let t = miniagent_core::budget::estimate_history_tokens(std::slice::from_ref(msg));
            if t > RETENTION_TOKEN_BUDGET / 2 {
                let text = msg.text_content();
                let keep_bytes = RETENTION_TOKEN_BUDGET * 2; // ≈1/2 预算的字节量
                let mut head: String = text.chars().take(keep_bytes.max(1)).collect();
                let original_tokens = t;
                head.push_str(&format!(
                    "\n[...TRUNCATED here: this single result was ~{original_tokens} tokens; \
                     full content available via re-read with offset/limit...]"
                ));
                *msg = Message::tool(
                    // 保留原 tool_call_id：从文本前缀恢复
                    text.strip_prefix("[toolu_vrtx_")
                        .and_then(|s| s.split(']').next())
                        .unwrap_or("unknown"),
                    &head,
                );
            }
        }

        let mut trimmed = Vec::with_capacity(recent.len() + 2);
        if let Some(msg) = first {
            trimmed.push(msg);
        }
        trimmed.push(Message::assistant_text(format!(
            "[Context trimmed. Summary of earlier work:\n{summary}\n\n\
             Continue the task with the latest results below.]"
        )));
        trimmed.extend(recent);
        *history = trimmed;
        CompactionOutcome::Rebuilt
    }
}
