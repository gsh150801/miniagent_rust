/// L0 Working Memory — manages what stays in the LLM context window.
/// Inspired by MemGPT/Letta: the agent itself decides what to keep/evict.
pub struct WorkingMemory {
    /// Maximum tokens allowed in the context window
    max_tokens: usize,
    /// Current token estimate
    current_tokens: usize,
}

impl WorkingMemory {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            current_tokens: 0,
        }
    }

    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    pub fn remaining(&self) -> usize {
        self.max_tokens.saturating_sub(self.current_tokens)
    }

    pub fn reset(&mut self) {
        self.current_tokens = 0;
    }
}

pub enum ContextSection {
    Persona,
    TaskInstruction,
    IndexTable,
    CurrentFocus,
    RecalledContext,
    ToolOutputs,
    Reserve,
}
