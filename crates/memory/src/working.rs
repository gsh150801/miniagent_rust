/// L0 Working Memory — manages what stays in the LLM context window.
/// Inspired by MemGPT/Letta: the agent itself decides what to keep/evict.
pub struct WorkingMemory {
    /// Maximum tokens allowed in the context window
    max_tokens: usize,
    /// Current token estimate
    current_tokens: usize,
    /// Fixed allocation proportions
    allocation: ContextAllocation,
}

#[derive(Debug, Clone)]
pub struct ContextAllocation {
    pub persona: f64,
    pub task_instruction: f64,
    pub index_table: f64,
    pub current_focus: f64,
    pub recalled_context: f64,
    pub tool_outputs: f64,
    pub reserve: f64,
}

impl Default for ContextAllocation {
    fn default() -> Self {
        Self {
            persona: 0.05,
            task_instruction: 0.05,
            index_table: 0.10,
            current_focus: 0.30,
            recalled_context: 0.30,
            tool_outputs: 0.15,
            reserve: 0.05,
        }
    }
}

impl WorkingMemory {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            current_tokens: 0,
            allocation: ContextAllocation::default(),
        }
    }

    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    pub fn remaining(&self) -> usize {
        self.max_tokens.saturating_sub(self.current_tokens)
    }

    pub fn budget_for(&self, section: ContextSection) -> usize {
        let fraction = match section {
            ContextSection::Persona => self.allocation.persona,
            ContextSection::TaskInstruction => self.allocation.task_instruction,
            ContextSection::IndexTable => self.allocation.index_table,
            ContextSection::CurrentFocus => self.allocation.current_focus,
            ContextSection::RecalledContext => self.allocation.recalled_context,
            ContextSection::ToolOutputs => self.allocation.tool_outputs,
            ContextSection::Reserve => self.allocation.reserve,
        };
        (self.max_tokens as f64 * fraction) as usize
    }

    pub fn track_usage(&mut self, tokens: usize) {
        self.current_tokens = self.current_tokens.saturating_add(tokens);
    }

    pub fn is_near_limit(&self) -> bool {
        self.current_tokens as f64 > self.max_tokens as f64 * 0.85
    }

    pub fn needs_compaction(&self) -> bool {
        self.is_near_limit()
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
