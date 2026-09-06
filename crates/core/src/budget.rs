use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Budget {
    pub tokens: TokenBudget,
    pub time: TimeBudget,
    pub money: MoneyBudget,
    pub iterations: IterationBudget,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    pub max_input_tokens: usize,
    pub max_output_tokens: usize,
    pub total_limit: Option<usize>,
    pub consumed: usize,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            max_input_tokens: 128_000,
            max_output_tokens: 16_000,
            total_limit: None,
            consumed: 0,
        }
    }
}

impl TokenBudget {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct TimeBudget {
    pub max_seconds: Option<u64>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub elapsed_secs: u64,
}


impl TimeBudget {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoneyBudget {
    pub max_usd: Option<f64>,
    pub consumed_usd: f64,
}

impl Default for MoneyBudget {
    fn default() -> Self {
        Self {
            max_usd: Some(5.0),
            consumed_usd: 0.0,
        }
    }
}

impl MoneyBudget {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationBudget {
    pub max_tool_iterations: usize,
    pub current_iteration: usize,
}

impl Default for IterationBudget {
    fn default() -> Self {
        Self {
            max_tool_iterations: 100,
            current_iteration: 0,
        }
    }
}

impl IterationBudget {
    pub fn increment(&mut self) {
        self.current_iteration += 1;
    }
}

// ── Token 估算与上下文预算（MemGPT 式内存压力模型的度量基础）────────
//
// 为什么不用 字节数/4：CJK 字符一个占 3 个 UTF-8 字节，除以 4 会低估
// 实际 token 3-4 倍（MiniMax/GLM 等国产模型的中文 tokenizer 通常
// ≈1-2 token/字）；同时 ToolUse 的 input JSON 常常比文本还大
// （live：写报告任务单次 write 参数 15KB+），必须计入。
//
// 精确 tokenizer 需要每模型词表，运行时不可得；这里用分段近似：
// CJK 按字计 1 token（保守），其余按 4 字节/token。高估约 30-50%
// 恰好是"提前触发 trim"的安全方向（MemGPT 的 memory-pressure
// 哲学：宁可早压，不可溢出）。

/// 近似 token 计数：CJK 字符按 1 token/字，其余按 UTF-8 字节/4。
pub fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other_bytes = 0usize;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else {
            other_bytes += ch.len_utf8();
        }
    }
    cjk + other_bytes / 4
}

/// 一段会话历史的近似 token 计数。ToolUse 的 input JSON 计入
/// （write/edit 等工具的参数往往承载全部交付内容），Thinking 块
/// 同样计入（也是请求体的一部分）。
pub fn estimate_history_tokens(messages: &[crate::message::Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.content.iter().map(|b| match b {
                crate::event::ContentBlock::Text { text } => estimate_tokens(text),
                crate::event::ContentBlock::ToolUse { input, .. } => {
                    estimate_tokens(&input.to_string())
                }
                crate::event::ContentBlock::Thinking { thinking, .. } => {
                    estimate_tokens(thinking)
                }
                _ => 0,
            }).sum::<usize>()
        })
        .sum()
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF    // CJK 统一表意文字
        | 0x3400..=0x4DBF  // 扩展 A
        | 0x3000..=0x303F  // CJK 标点（含全角逗号句号）
        | 0xFF00..=0xFFEF  // 全角形式
        | 0x3040..=0x30FF  // 日文假名
        | 0xAC00..=0xD7AF  // 韩文谚文
    )
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn cjk_counts_higher_than_byte_quarter() {
        let zh = "调研大型语言模型与智能体领域的最新进展并分析影响"; // 24 个汉字
        let ours = estimate_tokens(zh);
        let naive = zh.len() / 4; // 旧算法：72 字节/4 = 18
        assert!(ours > naive, "CJK 必须 > 旧字节/4 估算: {ours} vs {naive}");
        assert_eq!(ours, 24, "24 个汉字 ≈ 24 token, got {ours}");
    }

    #[test]
    fn ascii_roughly_bytes_over_four() {
        let en = "hello world this is a plain ascii sentence!!";
        assert_eq!(estimate_tokens(en), en.len() / 4);
    }

    #[test]
    fn mixed_content_estimates_both() {
        let s = "总结 report.md 的关键发现"; // 7 CJK + 11 ascii
        let t = estimate_tokens(s);
        assert!(t >= 8 && t <= 12, "got {t}");
    }
}
