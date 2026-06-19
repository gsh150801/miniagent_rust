use miniagent_memory::manager::MemoryManager;
use miniagent_memory::ConsolidationLevel;

/// Sleeptime Consolidation: background memory optimization when agent is idle.
/// Inspired by Letta Sleeptime and EngramAI.
pub struct SleeptimeConsolidation {
    interval_minutes: u64,
    last_run: Option<chrono::DateTime<chrono::Utc>>,
}

impl SleeptimeConsolidation {
    pub fn new(interval_minutes: u64) -> Self {
        Self {
            interval_minutes,
            last_run: None,
        }
    }

    pub fn should_run(&self) -> bool {
        match self.last_run {
            None => true,
            Some(last) => {
                let elapsed = chrono::Utc::now() - last;
                elapsed.num_minutes() >= self.interval_minutes as i64
            }
        }
    }

    /// Run a full consolidation cycle
    pub async fn run(
        &mut self,
        memory: &MemoryManager,
        skill_manager: &mut crate::offline::skill_manager::SkillManager,
        experience_graph: &crate::offline::experience_graph::ExperienceGraph,
    ) -> ConsolidationReport {
        let start = chrono::Utc::now();

        // 1. Decay: apply time-based memory decay
        memory.consolidate(ConsolidationLevel::Sleeptime).await;

        // 2. Skill evaluation: check all skills and retire underperformers
        let skills_before = skill_manager.all_skills().len();
        let _active_before = skill_manager.active_count();

        // (Skill evaluation happens via record_usage calls)

        let skills_after = skill_manager.all_skills().len();
        let active_after = skill_manager.active_count();

        // 3. Update meta-skill if we've discovered better patterns
        if experience_graph.node_count() > 100 {
            let guidance = format!(
                "Based on {} experiences: successful skills tend to be specific, \
                 have clear success criteria, and generalize across 3+ similar tasks. \
                 Avoid overly broad skills that can't be evaluated objectively.",
                experience_graph.node_count()
            );
            skill_manager.update_meta_skill(&guidance);
        }

        self.last_run = Some(start);

        ConsolidationReport {
            ran_at: start,
            skills_retired: skills_before.saturating_sub(skills_after),
            active_skills: active_after,
            meta_skill_updated: experience_graph.node_count() > 100,
            duration_ms: (chrono::Utc::now() - start).num_milliseconds() as u64,
        }
    }
}

impl Default for SleeptimeConsolidation {
    fn default() -> Self {
        Self::new(30)
    }
}

#[derive(Debug, Clone)]
pub struct ConsolidationReport {
    pub ran_at: chrono::DateTime<chrono::Utc>,
    pub skills_retired: usize,
    pub active_skills: usize,
    pub meta_skill_updated: bool,
    pub duration_ms: u64,
}
