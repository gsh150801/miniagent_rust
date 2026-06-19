/// L3 Procedural Memory — SKILL.md repository and Meta-Skill.
/// Used by MemoryManager to inject relevant skill prompts into agent context.

#[derive(Debug, Clone)]
pub struct Skill {
    pub id: uuid::Uuid,
    pub name: String,
    pub content: String,
    pub success_rate: f64,
    pub use_count: u64,
}

pub struct ProceduralMemory {
    skills: Vec<Skill>,
    meta_skill: Option<String>,
}

impl ProceduralMemory {
    pub fn new() -> Self {
        Self {
            skills: Vec::new(),
            meta_skill: None,
        }
    }

    pub fn add_skill(&mut self, name: impl Into<String>, content: impl Into<String>) -> &Skill {
        let skill = Skill {
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            content: content.into(),
            success_rate: 1.0,
            use_count: 0,
        };
        self.skills.push(skill);
        self.skills.last().unwrap()
    }

    pub fn find_relevant(&self, _task_description: &str) -> Vec<&Skill> {
        // Phase 3: semantic matching against task
        self.skills.iter().collect()
    }

    pub fn set_meta_skill(&mut self, content: impl Into<String>) {
        self.meta_skill = Some(content.into());
    }

    pub fn meta_skill(&self) -> Option<&str> {
        self.meta_skill.as_deref()
    }
}

impl Default for ProceduralMemory {
    fn default() -> Self {
        Self::new()
    }
}
