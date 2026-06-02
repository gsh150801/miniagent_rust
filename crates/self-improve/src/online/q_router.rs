use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Q-Learning Router: learns optimal strategy choices over time.
/// 4 levels: search strategy, model selection, retrieval depth, proactivity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QLearningRouter {
    q_table: HashMap<StateActionKey, f64>,
    alpha: f64,    // learning rate
    gamma: f64,    // discount factor
    epsilon: f64,  // exploration rate
    total_steps: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct StateActionKey {
    state: RouterState,
    action: RouterAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouterState {
    pub task_type: TaskType,
    pub complexity_level: u8,
    pub memory_available: bool,
    pub budget_percent: u8,  // 0-100, discrete to allow Hash/Eq
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType { Qa, Summarization, Analysis, CodeGeneration, Research, Creative }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RouterAction {
    // Model selection
    UseFlash, UsePro,
    // Search strategy
    SearchFts, SearchVector, SearchHybrid, SearchGraph,
    // Retrieval depth
    ShallowOverview, DeepRecall, FullContext,
    // Proactivity level
    Passive, Active, Autonomous,
}

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub model: RouterAction,
    pub search_strategy: RouterAction,
    pub retrieval_depth: RouterAction,
    pub proactivity: RouterAction,
}

impl Default for RoutingDecision {
    fn default() -> Self {
        Self {
            model: RouterAction::UseFlash,
            search_strategy: RouterAction::SearchHybrid,
            retrieval_depth: RouterAction::ShallowOverview,
            proactivity: RouterAction::Passive,
        }
    }
}

impl QLearningRouter {
    pub fn new() -> Self {
        Self {
            q_table: HashMap::new(),
            alpha: 0.1,
            gamma: 0.9,
            epsilon: 0.15,
            total_steps: 0,
        }
    }

    /// Select the best action for a state (epsilon-greedy)
    pub fn select_action(&self, state: &RouterState, action_group: ActionGroup) -> RouterAction {
        let actions = action_group.actions();
        if actions.is_empty() {
            return RouterAction::UseFlash;
        }

        // Explore — simple deterministic pseudo-random
        let explore = (self.total_steps.wrapping_mul(6364136223846793005) >> 32) as f64 / (u32::MAX as f64);
        if explore < self.epsilon {
            let idx = (self.total_steps as usize) % actions.len();
            return actions[idx];
        }

        // Exploit: pick action with highest Q value
        let mut best_action = actions[0];
        let mut best_q = f64::NEG_INFINITY;

        for &action in &actions {
            let key = StateActionKey { state: *state, action };
            let q = self.q_table.get(&key).copied().unwrap_or(0.0);
            if q > best_q {
                best_q = q;
                best_action = action;
            }
        }

        best_action
    }

    /// Make a full routing decision for a state
    pub fn decide(&self, state: &RouterState) -> RoutingDecision {
        RoutingDecision {
            model: self.select_action(state, ActionGroup::Model),
            search_strategy: self.select_action(state, ActionGroup::Search),
            retrieval_depth: self.select_action(state, ActionGroup::Retrieval),
            proactivity: self.select_action(state, ActionGroup::Proactivity),
        }
    }

    /// Update Q-value based on reward
    pub fn update(
        &mut self,
        state: &RouterState,
        action: RouterAction,
        reward: f64,
        next_state: &RouterState,
    ) {
        let key = StateActionKey { state: *state, action };

        // Find max Q for next state across all actions
        let max_next_q = ActionGroup::all_actions()
            .iter()
            .map(|&a| {
                let nk = StateActionKey { state: *next_state, action: a };
                self.q_table.get(&nk).copied().unwrap_or(0.0)
            })
            .fold(f64::NEG_INFINITY, f64::max);

        let current_q = self.q_table.get(&key).copied().unwrap_or(0.0);
        let new_q = current_q + self.alpha * (reward + self.gamma * max_next_q - current_q);
        self.q_table.insert(key, new_q);

        self.total_steps += 1;
    }

    /// Decay epsilon over time for better exploitation
    pub fn decay_exploration(&mut self) {
        self.epsilon = (self.epsilon * 0.999).max(0.02);
    }

    pub fn stats(&self) -> RouterStats {
        RouterStats {
            total_entries: self.q_table.len() as u64,
            total_steps: self.total_steps,
            epsilon: self.epsilon,
        }
    }
}

impl Default for QLearningRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RouterStats {
    pub total_entries: u64,
    pub total_steps: u64,
    pub epsilon: f64,
}

#[derive(Debug, Clone, Copy)]
pub enum ActionGroup {
    Model,
    Search,
    Retrieval,
    Proactivity,
}

impl ActionGroup {
    fn actions(&self) -> Vec<RouterAction> {
        match self {
            ActionGroup::Model => vec![RouterAction::UseFlash, RouterAction::UsePro],
            ActionGroup::Search => vec![
                RouterAction::SearchFts,
                RouterAction::SearchVector,
                RouterAction::SearchHybrid,
                RouterAction::SearchGraph,
            ],
            ActionGroup::Retrieval => vec![
                RouterAction::ShallowOverview,
                RouterAction::DeepRecall,
                RouterAction::FullContext,
            ],
            ActionGroup::Proactivity => vec![
                RouterAction::Passive,
                RouterAction::Active,
                RouterAction::Autonomous,
            ],
        }
    }

    fn all_actions() -> Vec<RouterAction> {
        let mut all = Vec::new();
        all.extend(Self::Model.actions());
        all.extend(Self::Search.actions());
        all.extend(Self::Retrieval.actions());
        all.extend(Self::Proactivity.actions());
        all
    }
}
