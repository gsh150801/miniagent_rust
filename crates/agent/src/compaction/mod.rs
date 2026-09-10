//! 可插拔历史记忆压缩框架（对照 Codex 的 context_management 体系）。
//!
//! 两种可插拔策略（模式定义见 [`MemoryMode`]）：
//! - [`LlmSummaryCompactor`]：传统路径——上下文超限时把被裁剪历史交给
//!   LLM 生成结构化摘要（四段式：约束/决策/产物/待办），重建窗口为
//!   首条 + 摘要 + 最近 N 条。
//! - [`NotesHistoryCompactor`]：Codex 式——模型经 write_note/notes.md
//!   主动维护状态记忆（notes.json），trim 时历史**归档不销毁**
//!   （history_archive.jsonl，item_id 连续），重建窗口不调 LLM；
//!   notes 缺失时机械提取 checkpoint 兜底（auto_compact_fallback 思路）。
//!
//! 模式解析优先级：运行时覆盖（前端开关，[`set_runtime_override`]）
//! > 配置文件（AGENT_MEMORY_MODE / AGENT_NOTES_MODE）> 默认 LlmSummary。

pub mod llm_summary;
pub mod notes_history;

use miniagent_core::message::Message;
use miniagent_core::settings::AppConfig;
use tokio_util::sync::CancellationToken;

use crate::Agent;
use crate::context::RunContext;

/// 历史记忆压缩模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMode {
    /// 传统 LLM 摘要压缩。
    LlmSummary,
    /// Codex 式 notes 状态记忆 + 历史归档按需检索。
    NotesHistory,
}

impl MemoryMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryMode::LlmSummary => "llm_summary",
            MemoryMode::NotesHistory => "notes_history",
        }
    }

    pub fn parse(s: &str) -> Option<MemoryMode> {
        match s.trim().to_lowercase().as_str() {
            "llm_summary" | "llm" | "summary" => Some(MemoryMode::LlmSummary),
            "notes_history" | "notes" => Some(MemoryMode::NotesHistory),
            _ => None,
        }
    }
}

/// 运行时覆盖：0=跟随配置，1=LlmSummary，2=NotesHistory。
/// 前端设置开关写入这里（热切换，立即影响所有后续 trim）。
static RUNTIME_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_runtime_override(mode: Option<MemoryMode>) {
    use std::sync::atomic::Ordering;
    let v = match mode {
        None => 0,
        Some(MemoryMode::LlmSummary) => 1,
        Some(MemoryMode::NotesHistory) => 2,
    };
    RUNTIME_OVERRIDE.store(v, Ordering::SeqCst);
    tracing::info!(mode = mode.map(|m| m.as_str()).unwrap_or("(config default)"),
        "memory mode runtime override updated");
}

pub fn runtime_override() -> Option<MemoryMode> {
    use std::sync::atomic::Ordering;
    match RUNTIME_OVERRIDE.load(Ordering::SeqCst) {
        1 => Some(MemoryMode::LlmSummary),
        2 => Some(MemoryMode::NotesHistory),
        _ => None,
    }
}

/// 生效模式：运行时覆盖优先，否则按配置（agent_notes_mode）。
pub fn effective_mode(config: Option<&AppConfig>) -> MemoryMode {
    if let Some(m) = runtime_override() {
        return m;
    }
    match config {
        Some(c) if c.agent_notes_mode => MemoryMode::NotesHistory,
        _ => MemoryMode::LlmSummary,
    }
}

/// 压缩结果：是否成功重建了窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionOutcome {
    /// 本 compactor 已重建窗口。
    Rebuilt,
    /// 本 compactor 放弃（调用方决定 fallback）。
    Declined,
}

/// 可插拔历史压缩策略。实现就地修改 `history`（首条消息必须保留）。
pub trait HistoryCompactor: Send + Sync {
    fn name(&self) -> &'static str;

    #[allow(async_fn_in_trait)]
    async fn compact(
        &self,
        agent: &Agent,
        history: &mut Vec<Message>,
        context: &RunContext,
        cancel: CancellationToken,
    ) -> CompactionOutcome;
}

/// 按生效模式执行压缩。notes_history 因 notes 完全不可用而放弃时，
/// 回落到 LLM 摘要（质量兜底优先于模式纯洁性）。
pub async fn run_compaction(
    agent: &Agent,
    history: &mut Vec<Message>,
    context: &RunContext,
    cancel: CancellationToken,
) {
    match effective_mode(agent.config.as_deref()) {
        MemoryMode::NotesHistory => {
            if !context.working_dir.is_empty() {
                let compactor = notes_history::NotesHistoryCompactor;
                if compactor.compact(agent, history, context, cancel_child(cancel.clone())).await
                    == CompactionOutcome::Rebuilt
                {
                    return;
                }
                tracing::warn!(
                    "notes_history compactor declined (no usable notes) — falling back to llm_summary"
                );
            }
            llm_summary::LlmSummaryCompactor
                .compact(agent, history, context, cancel_child(cancel))
                .await;
        }
        MemoryMode::LlmSummary => {
            llm_summary::LlmSummaryCompactor
                .compact(agent, history, context, cancel_child(cancel))
                .await;
        }
    }
}

/// clone 一个子 token（trait 签名取值语义）。
fn cancel_child(cancel: CancellationToken) -> CancellationToken {
    cancel.clone()
}
