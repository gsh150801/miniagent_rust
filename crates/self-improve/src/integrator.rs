use miniagent_core::message::Message;

/// Re-export for use within the crate
#[derive(Debug, Clone)]
pub struct AgentDelta {
    pub new_messages: Vec<Message>,
    pub stop_reason: miniagent_core::event::StopReason,
    pub usage: miniagent_core::event::Usage,
}

use crate::offline::consolidation::SleeptimeConsolidation;
use crate::offline::experience_graph::ExperienceGraph;
use crate::offline::skill_manager::SkillManager;
use crate::online::lifecycle_guard::{GuardDecision, LifecycleGuard, SkillPerformance};
use crate::online::q_router::{QLearningRouter, RouterState, TaskType};
use crate::online::tool_tracker::ToolReliabilityTracker;

/// Unified self-improvement system combining online and offline components.
pub struct SelfImprover {
    // Online (in-session)
    pub q_router: QLearningRouter,
    pub tool_tracker: ToolReliabilityTracker,
    pub lifecycle_guard: LifecycleGuard,
    // Offline (sleeptime)
    pub experience_graph: ExperienceGraph,
    pub skill_manager: SkillManager,
    pub consolidation: SleeptimeConsolidation,
}

impl SelfImprover {
    pub fn new() -> Self {
        Self {
            q_router: QLearningRouter::new(),
            tool_tracker: ToolReliabilityTracker::default(),
            lifecycle_guard: LifecycleGuard::new(),
            experience_graph: ExperienceGraph::new(),
            skill_manager: SkillManager::new(),
            consolidation: SleeptimeConsolidation::new(),
        }
    }

    // ── Online Loop (called every agent step) ──────────────────

    pub fn decide_routing(&mut self, complexity: u8, budget_pct: u8) -> RouterState {
        let state = RouterState {
            task_type: TaskType::Research,
            complexity_level: complexity,
            memory_available: true,
            budget_percent: budget_pct,
        };
        let _decision = self.q_router.decide(&state);

        // Decay exploration over time
        self.q_router.decay_exploration();

        state
    }

    pub fn guard_skill(
        &self,
        active_count: usize,
        skill_id: uuid::Uuid,
        recent_score: f64,
        eval_count: usize,
    ) -> GuardDecision {
        let perf = SkillPerformance {
            skill_id,
            recent_score,
            eval_count,
            streak: 0,
        };
        self.lifecycle_guard.guard_skill_change(active_count, &perf)
    }

    // ── Offline Loop (called on idle / episode end) ────────────

    pub fn stats(&self) -> SelfImproverStats {
        SelfImproverStats {
            router_entries: self.q_router.stats().total_entries,
            experiences: self.experience_graph.node_count(),
            skills_active: self.skill_manager.active_count(),
            skills_total: self.skill_manager.all_skills().len(),
            tools_tracked: self.tool_tracker.all().len(),
        }
    }

}

impl Default for SelfImprover {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SelfImproverStats {
    pub router_entries: u64,
    pub experiences: usize,
    pub skills_active: usize,
    pub skills_total: usize,
    pub tools_tracked: usize,
}
