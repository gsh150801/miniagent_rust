use std::collections::HashMap;
use crate::bundle::{SkillBundle, SkillId};

/// Dynamic registry for loaded skills.
/// Skills can be registered/unregistered at runtime.
pub struct SkillRegistry {
    skills: HashMap<SkillId, SkillBundle>,
    /// Maps trigger phrases to skill IDs for fast matching
    trigger_index: HashMap<String, Vec<SkillId>>,
    /// Maps tool names to skill IDs (which skills need which tools)
    tool_index: HashMap<String, Vec<SkillId>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            trigger_index: HashMap::new(),
            tool_index: HashMap::new(),
        }
    }

    pub fn register(&mut self, bundle: SkillBundle) {
        let id = bundle.id;

        // Index triggers
        for trigger in &bundle.metadata.triggers {
            let lower = trigger.to_lowercase();
            self.trigger_index.entry(lower).or_default().push(id);
        }

        // Index tools
        for tool in &bundle.metadata.tools_needed {
            let lower = tool.to_lowercase();
            self.tool_index.entry(lower).or_default().push(id);
        }

        self.skills.insert(id, bundle);
    }

    pub fn unregister(&mut self, id: &SkillId) -> Option<SkillBundle> {
        let bundle = self.skills.remove(id)?;

        // Clean up indices
        for trigger in &bundle.metadata.triggers {
            let lower = trigger.to_lowercase();
            if let Some(ids) = self.trigger_index.get_mut(&lower) {
                ids.retain(|i| *i != *id);
                if ids.is_empty() {
                    self.trigger_index.remove(&lower);
                }
            }
        }

        Some(bundle)
    }

    pub fn get(&self, id: &SkillId) -> Option<&SkillBundle> {
        self.skills.get(id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&SkillBundle> {
        self.skills.values().find(|s| s.metadata.name == name)
    }

    /// Find skills matching a user query based on trigger similarity.
    /// Returns skills sorted by relevance: trigger match + priority.
    pub fn find_matching(&self, user_query: &str, max_results: usize) -> Vec<&SkillBundle> {
        let lower_query = user_query.to_lowercase();
        let mut scored: Vec<(f64, &SkillBundle)> = Vec::new();

        for skill in self.skills.values() {
            let mut score: f64 = 0.0;

            // Exact trigger match
            for trigger in &skill.metadata.triggers {
                let lower_trigger = trigger.to_lowercase();
                if lower_query.contains(&lower_trigger) {
                    score += 0.5;
                }
                // Word overlap between query and trigger
                let trigger_words: Vec<&str> = lower_trigger.split_whitespace().collect();
                let query_words: Vec<&str> = lower_query.split_whitespace().collect();
                let overlap = trigger_words.iter().filter(|w| query_words.contains(w)).count();
                score += overlap as f64 * 0.15;
            }

            // Name match
            if lower_query.contains(&skill.metadata.name.to_lowercase()) {
                score += 0.3;
            }

            // Description word overlap
            let desc_lower = skill.metadata.description.to_lowercase();
            let desc_words: Vec<&str> = desc_lower.split_whitespace().collect();
            let query_words: Vec<&str> = lower_query.split_whitespace().collect();
            let overlap = query_words.iter().filter(|w| desc_words.contains(w)).count();
            score += overlap as f64 * 0.05;

            // Priority bonus
            score += skill.metadata.priority as f64 * 0.01;

            if score > 0.0 {
                scored.push((score, skill));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_results);
        scored.into_iter().map(|(_, s)| s).collect()
    }

    /// Get all skills that need a specific tool.
    pub fn skills_needing_tool(&self, tool_name: &str) -> Vec<&SkillBundle> {
        self.tool_index
            .get(&tool_name.to_lowercase())
            .map(|ids| ids.iter().filter_map(|id| self.skills.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn all(&self) -> Vec<&SkillBundle> {
        self.skills.values().collect()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self { Self::new() }
}
