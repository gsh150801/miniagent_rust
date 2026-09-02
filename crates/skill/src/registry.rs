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

    pub fn get(&self, id: &SkillId) -> Option<&SkillBundle> {
        self.skills.get(id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&SkillBundle> {
        self.skills.values().find(|s| s.metadata.name == name)
    }

    /// 返回所有已注册技能的名字列表（用于"技能未找到"时帮助 LLM 自纠错）。
    pub fn all_skill_names(&self) -> Vec<String> {
        self.skills.values()
            .map(|s| s.metadata.name.clone())
            .collect()
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
            // CJK substring matching: Chinese text has no whitespace, so the
            // word-overlap checks above never fire for 中文 queries. Require
            // the full CJK part of a trigger (>=2 chars) to appear inside a
            // contiguous CJK run of the query.
            for run in cjk_runs(&lower_query) {
                for trigger in &skill.metadata.triggers {
                    let t_cjk: String = trigger.to_lowercase().chars()
                        .filter(|c| (*c as u32) >= 0x2E80)
                        .collect();
                    if t_cjk.chars().count() >= 2 && run.contains(&t_cjk) {
                        score += 0.5;
                    }
                }
                let name_cjk: String = skill.metadata.name.chars()
                    .filter(|c| (*c as u32) >= 0x2E80)
                    .collect();
                if name_cjk.chars().count() >= 2 && run.contains(&name_cjk.to_lowercase()) {
                    score += 0.25;
                }
            }

            // Fuzzy name matching (typo tolerance): a query token whose
            // levenshtein distance to the skill name is within 25% (min 1
            // edit) counts as a name match. Live: user typed
            // "bioinf-verigy-report" (typo of bioinf-verify-report) and the
            // skill was not found.
            for token in lower_query.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
                if token.len() < 6 {
                    continue;
                }
                let name = skill.metadata.name.to_lowercase();
                let dist = levenshtein(token, &name);
                let tol = ((name.len() as f64) * 0.25).ceil() as usize;
                if dist <= tol.max(1) {
                    score += 0.45;
                    break;
                }
            }


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

/// Extract contiguous CJK runs from a string (for substring trigger matching).
fn cjk_runs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if (c as u32) >= 0x2E80 {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Levenshtein edit distance (small inputs; O(mn)).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() { return b.len(); }
    if b.is_empty() { return a.len(); }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod fuzzy_tests {
    use super::*;

    #[test]
    fn levenshtein_basic() {
        // verigy → verify: single substitution at position 12 (g→f... i.e.
        // "verigy" vs "verify" differ by one char).
        assert_eq!(levenshtein("bioinf-verigy-report", "bioinf-verify-report"), 1);
    }

    #[test]
    fn cjk_runs_splits() {
        assert_eq!(cjk_runs("校验你所总结的报告").len(), 1);
        assert_eq!(cjk_runs("abc校验def报告"), vec!["校验", "报告"]);
    }
}

#[cfg(test)]
mod fuzzy_match_tests {
    use super::*;

    #[test]
    fn levenshtein_catches_single_substitution() {
        // verigy vs verify: single substitution (g→f... actually i→e/y→i
        // positions) — distance 1.
        let d = levenshtein("bioinf-verigy-report", "bioinf-verify-report");
        assert_eq!(d, 1, "typo distance should be exactly 1");
        let tol = (("bioinf-verify-report".len() as f64) * 0.25).ceil() as usize;
        assert!(d <= tol.max(1), "within 25% tolerance");
    }

    #[test]
    fn cjk_runs_split_correctly() {
        assert_eq!(cjk_runs("校验你所总结的报告").len(), 1);
        assert_eq!(cjk_runs("abc校验def报告ghi"), vec!["校验", "报告"]);
    }
}
