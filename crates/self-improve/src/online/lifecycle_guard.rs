use serde::{Deserialize, Serialize};

/// Lifecycle Guard — prevents performance degradation (Ratchet non-divergence).
/// Inspired by the Ratchet paper: bounded active-cap + retirement threshold
/// guarantees expected performance never drops below baseline.
#[derive(Debug, Clone)]
pub struct LifecycleGuard {
    baseline_score: f64,
    active_skill_cap: usize,
    retirement_threshold: f64,
    evaluation_window: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardDecision {
    Accept,
    Rollback { skill_id: uuid::Uuid },
    RetireWeakest { skill_id: uuid::Uuid },
    CapExceeded { skill_id: uuid::Uuid },
}

impl LifecycleGuard {
    pub fn new() -> Self {
        Self {
            baseline_score: 0.5,
            active_skill_cap: 10,
            retirement_threshold: 0.3,
            evaluation_window: 20,
        }
    }

    /// Guard a skill change — decides whether to accept, rollback, or retire
    pub fn guard_skill_change(
        &self,
        active_count: usize,
        skill: &SkillPerformance,
    ) -> GuardDecision {
        // Check if skill performance dropped below baseline significantly
        if skill.recent_score < self.baseline_score * 0.95 {
            return GuardDecision::Rollback {
                skill_id: skill.skill_id,
            };
        }

        // Check if skill should be retired
        if skill.recent_score < self.retirement_threshold && skill.eval_count >= self.evaluation_window {
            return GuardDecision::RetireWeakest {
                skill_id: skill.skill_id,
            };
        }

        // Check cap
        if active_count >= self.active_skill_cap {
            return GuardDecision::CapExceeded {
                skill_id: skill.skill_id,
            };
        }

        GuardDecision::Accept
    }

}

impl Default for LifecycleGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPerformance {
    pub skill_id: uuid::Uuid,
    pub recent_score: f64,
    pub eval_count: usize,
    pub streak: i32,
}
