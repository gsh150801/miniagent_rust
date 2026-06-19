use miniagent_core::message::Message;

/// Re-export for use within the crate
#[derive(Debug, Clone)]
pub struct AgentDelta {
    pub new_messages: Vec<Message>,
    pub stop_reason: miniagent_core::event::StopReason,
    pub usage: miniagent_core::event::Usage,
}
use miniagent_memory::manager::MemoryManager;
use tokio_util::sync::CancellationToken;

use crate::offline::consolidation::{ConsolidationReport, SleeptimeConsolidation};
use crate::offline::experience_graph::ExperienceGraph;
use crate::offline::skill_manager::SkillManager;
use crate::online::lifecycle_guard::{GuardDecision, LifecycleGuard, SkillPerformance};
use crate::online::q_router::{QLearningRouter, RouterState, TaskType};
use crate::online::reflection::StepReflection;
use crate::online::tool_tracker::ToolReliabilityTracker;

/// Unified self-improvement system combining online and offline components.
pub struct SelfImprover {
    // Online (in-session)
    pub step_reflector: crate::online::reflection::StepReflector,
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
            step_reflector: crate::online::reflection::StepReflector::new(),
            q_router: QLearningRouter::new(),
            tool_tracker: ToolReliabilityTracker::default(),
            lifecycle_guard: LifecycleGuard::new(),
            experience_graph: ExperienceGraph::new(),
            skill_manager: SkillManager::new(),
            consolidation: SleeptimeConsolidation::new(30),
        }
    }

    // ── Online Loop (called every agent step) ──────────────────

    pub async fn on_step(
        &mut self,
        history: &[Message],
        delta: &AgentDelta,
        cancel: CancellationToken,
    ) -> StepReflection {
        let reflection = self.step_reflector.reflect(history, delta, cancel).await;

        // Record to experience graph if significant
        if reflection.self_score < 0.3 || reflection.error_detected {
            let sig = self.task_signature_from_history(history);
            if reflection.error_detected {
                let id = self.experience_graph.add_experience(
                    crate::offline::experience_graph::NodeType::FailurePattern,
                    &format!("Step error: {:?}", reflection.error_root_cause),
                    &[],
                    &sig,
                );
                // Link failure to potential causes
                if let Some(root_cause) = &reflection.error_root_cause {
                    let cause_id = self.experience_graph.add_experience(
                        crate::offline::experience_graph::NodeType::Insight,
                        root_cause,
                        &[format!("Root cause: {root_cause}")],
                        &sig,
                    );
                    self.experience_graph.link(
                        id,
                        cause_id,
                        crate::offline::experience_graph::EdgeType::CausedBy,
                        1.0,
                    );
                }
            } else if reflection.self_score > 0.8 {
                self.experience_graph.add_experience(
                    crate::offline::experience_graph::NodeType::SuccessPattern,
                    "High-quality response generated",
                    &["Maintain response depth".to_string(), "Verify sources".to_string()],
                    &sig,
                );
            }
        }

        reflection
    }

    pub fn on_tool_success(&mut self, tool_name: &str, latency_ms: u64) {
        self.tool_tracker.record_success(tool_name, latency_ms);
    }

    pub fn on_tool_failure(&mut self, tool_name: &str, error: &str) {
        self.tool_tracker.record_failure(tool_name, error);
    }

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

    pub fn find_relevant_experiences(
        &self,
        signature: &[f64],
    ) -> (Vec<&crate::offline::experience_graph::ExperienceNode>, Vec<&crate::offline::experience_graph::ExperienceNode>) {
        let successes = self.experience_graph.query_success_patterns(signature);
        let pitfalls = self.experience_graph.query_pitfalls(signature);
        (successes, pitfalls)
    }

    // ── Offline Loop (called on idle / episode end) ────────────

    pub async fn on_idle(
        &mut self,
        memory: &MemoryManager,
    ) -> Option<ConsolidationReport> {
        if self.consolidation.should_run() {
            let report = self.consolidation.run(
                memory,
                &mut self.skill_manager,
                &self.experience_graph,
            ).await;
            Some(report)
        } else {
            None
        }
    }

    pub fn stats(&self) -> SelfImproverStats {
        SelfImproverStats {
            router_entries: self.q_router.stats().total_entries,
            experiences: self.experience_graph.node_count(),
            skills_active: self.skill_manager.active_count(),
            skills_total: self.skill_manager.all_skills().len(),
            tools_tracked: self.tool_tracker.all().len(),
        }
    }

    fn task_signature_from_history(&self, history: &[Message]) -> Vec<f64> {
        // Simple feature: average message length, count, tool usage count
        let msg_count = history.len() as f64;
        let avg_len = if msg_count > 0.0 {
            history.iter().map(|m| m.text_content().len() as f64).sum::<f64>() / msg_count
        } else {
            0.0
        };
        let tool_count = history.iter()
            .filter(|m| m.role == miniagent_core::message::MessageRole::Tool)
            .count() as f64;
        vec![msg_count / 100.0, avg_len / 1000.0, tool_count / 10.0]
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
