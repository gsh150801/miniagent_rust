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
    pub fn remaining_input(&self) -> usize {
        self.max_input_tokens
            .saturating_sub(self.consumed)
            .min(self.total_limit.unwrap_or(usize::MAX).saturating_sub(self.consumed))
    }

    pub fn is_exhausted(&self) -> bool {
        self.remaining_input() == 0
            || self
                .total_limit
                .is_some_and(|limit| self.consumed >= limit)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct TimeBudget {
    pub max_seconds: Option<u64>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub elapsed_secs: u64,
}


impl TimeBudget {
    pub fn is_expired(&self) -> bool {
        self.max_seconds
            .is_some_and(|max| self.elapsed_secs >= max)
    }
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
    pub fn is_exhausted(&self) -> bool {
        self.max_usd
            .is_some_and(|max| self.consumed_usd >= max)
    }
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
    pub fn is_exhausted(&self) -> bool {
        self.current_iteration >= self.max_tool_iterations
    }

    pub fn increment(&mut self) {
        self.current_iteration += 1;
    }
}
