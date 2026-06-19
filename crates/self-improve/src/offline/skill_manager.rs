use serde::{Deserialize, Serialize};

/// Skill Lifecycle Manager — create, evaluate, promote, and retire skills.
/// Inspired by Ratchet: outcome-driven retirement + meta-skill guidance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManager {
    skills: Vec<Skill>,
    meta_skill: MetaSkill,
    active_cap: usize,
    retirement_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: uuid::Uuid,
    pub name: String,
    pub content: String,
    pub status: SkillStatus,
    pub performance: SkillPerformanceHistory,
    pub created_from: Vec<uuid::Uuid>,  // experience IDs
    pub version: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillStatus {
    Draft,
    Active,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPerformanceHistory {
    pub scores: Vec<f64>,
    pub average: f64,
    pub trend: f64,     // positive = improving
    pub use_count: u64,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaSkill {
    pub content: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SkillManager {
    pub fn new() -> Self {
        Self {
            skills: Vec::new(),
            meta_skill: MetaSkill {
                content: "Create skills that are specific, reusable, and have clear success criteria. \
                          Each skill should include: purpose, prerequisites, step-by-step instructions, \
                          and expected outcomes. Prefer skills that generalize across similar tasks."
                    .into(),
                updated_at: chrono::Utc::now(),
            },
            active_cap: 10,
            retirement_threshold: 0.3,
        }
    }

    pub fn create_skill(
        &mut self,
        name: &str,
        content: &str,
        from_experiences: &[uuid::Uuid],
    ) -> &Skill {
        let skill = Skill {
            id: uuid::Uuid::new_v4(),
            name: name.to_string(),
            content: content.to_string(),
            status: SkillStatus::Draft,
            performance: SkillPerformanceHistory {
                scores: Vec::new(),
                average: 1.0,
                trend: 0.0,
                use_count: 0,
                last_used: None,
            },
            created_from: from_experiences.to_vec(),
            version: 1,
            created_at: chrono::Utc::now(),
        };
        self.skills.push(skill);
        self.skills.last().unwrap()
    }

    pub fn record_usage(&mut self, skill_id: &uuid::Uuid, score: f64) {
        // Update the skill record first
        {
            if let Some(skill) = self.skills.iter_mut().find(|s| s.id == *skill_id) {
                skill.performance.use_count += 1;
                skill.performance.last_used = Some(chrono::Utc::now());
                skill.performance.scores.push(score);

                let window = &skill.performance.scores[skill.performance.scores.len().saturating_sub(20)..];
                skill.performance.average = window.iter().sum::<f64>() / window.len() as f64;

                if window.len() >= 5 {
                    let n = window.len() as f64;
                    let mean_x = (n - 1.0) / 2.0;
                    let mean_y = skill.performance.average;
                    let num: f64 = window.iter().enumerate()
                        .map(|(i, &y)| (i as f64 - mean_x) * (y - mean_y))
                        .sum();
                    let den: f64 = (0..window.len()).map(|i| (i as f64 - mean_x).powi(2)).sum();
                    skill.performance.trend = if den != 0.0 { num / den } else { 0.0 };
                }
            }
        }

        // Then evaluate status (separate borrow)
        self.evaluate_skill(skill_id);
    }

    fn evaluate_skill(&mut self, skill_id: &uuid::Uuid) {
        // Collect status to apply in one immutable pass
        let action: Option<(SkillStatus, bool)> = {
            self.skills.iter().find(|s| s.id == *skill_id).map(|s| {
                if s.performance.use_count >= 20
                    && s.performance.average < self.retirement_threshold
                    && s.performance.trend <= 0.0
                {
                    (SkillStatus::Deprecated, false)
                } else if s.status == SkillStatus::Draft
                    && s.performance.use_count >= 5
                    && s.performance.average >= 0.6
                {
                    (SkillStatus::Active, true)
                } else {
                    (s.status, false)
                }
            })
        };

        if let Some((new_status, needs_check)) = action {
            if needs_check {
                let active_cnt = self.skills.iter().filter(|s| s.status == SkillStatus::Active).count();
                if active_cnt < self.active_cap
                    && let Some(skill) = self.skills.iter_mut().find(|s| s.id == *skill_id) {
                        skill.status = new_status;
                    }
            } else {
                if let Some(skill) = self.skills.iter_mut().find(|s| s.id == *skill_id) {
                    skill.status = new_status;
                }
            }
        }
    }

    pub fn active_count(&self) -> usize {
        self.skills.iter().filter(|s| s.status == SkillStatus::Active).count()
    }

    pub fn find_relevant(&self, task_description: &str) -> Vec<&Skill> {
        let lower = task_description.to_lowercase();
        self.skills
            .iter()
            .filter(|s| {
                s.status == SkillStatus::Active
                    && (s.name.to_lowercase().contains(&lower)
                        || s.content.to_lowercase().contains(&lower))
            })
            .collect()
    }

    pub fn update_meta_skill(&mut self, new_guidance: &str) {
        self.meta_skill = MetaSkill {
            content: new_guidance.to_string(),
            updated_at: chrono::Utc::now(),
        };
    }

    pub fn meta_skill_content(&self) -> &str {
        &self.meta_skill.content
    }

    pub fn all_skills(&self) -> &[Skill] {
        &self.skills
    }
}

impl Default for SkillManager {
    fn default() -> Self { Self::new() }
}
