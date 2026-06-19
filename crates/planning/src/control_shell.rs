use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::agent_profile::{AgentProfile, ActivationPolicy};
use crate::event_stream::EventStream;

// ── Activation Rule ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationRule {
    pub name: String,
    pub condition: Condition,
    pub activate: Vec<String>,
    pub priority: u8,
    pub cooldown_iterations: usize,
    pub last_activated: usize,
}

/// Typed conditions replace the old fragile string parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Condition {
    /// True if the specified file exists in the workspace
    FileExists(String),
    /// True if the file exists AND another file does NOT exist
    FileExistsAndNot(String, String),
    /// True if ALL specified files exist
    AllFilesExist(Vec<String>),
    /// True if the file exists AND its content contains `needle`.
    /// 用于基于产物内容做条件判断（如 judge verdict 是否为 "REVISE"）。
    FileContains(String, String),
    /// True if iteration count exceeds threshold
    IterationAbove(usize),
    /// True if a specific agent has completed at least N tasks
    AgentCompleted { agent: String, min_tasks: usize },
    /// True if any error was recorded by a specific agent
    AgentHasError(String),
    /// Always true (for always-active agents)
    Always,
    /// Composite AND: all conditions must be true
    And(Vec<Condition>),
}

impl ActivationRule {
    pub fn new(name: impl Into<String>, condition: Condition, activate: Vec<&str>) -> Self {
        Self {
            name: name.into(), condition,
            activate: activate.into_iter().map(|s| s.to_string()).collect(),
            priority: 0, cooldown_iterations: 0, last_activated: 0,
        }
    }

    pub fn with_priority(mut self, p: u8) -> Self { self.priority = p; self }
    pub fn with_cooldown(mut self, n: usize) -> Self { self.cooldown_iterations = n; self }

    /// Evaluate condition against workspace state and event stream.
    pub fn evaluate(&self, work_dir: &Path, iteration: usize, events: &EventStream) -> bool {
        self.eval_condition(&self.condition, work_dir, iteration, events)
    }

    fn eval_condition(&self, cond: &Condition, work_dir: &Path, iteration: usize, events: &EventStream) -> bool {
        match cond {
            Condition::Always => true,
            Condition::FileExists(path) => work_dir.join(path).exists(),
            Condition::FileExistsAndNot(exists, not_exists) => {
                work_dir.join(exists).exists() && !work_dir.join(not_exists).exists()
            }
            Condition::AllFilesExist(paths) => paths.iter().all(|p| work_dir.join(p).exists()),
            Condition::FileContains(path, needle) => {
                std::fs::read_to_string(work_dir.join(path))
                    .map(|content| content.contains(needle.as_str()))
                    .unwrap_or(false)
            }
            Condition::IterationAbove(n) => iteration > *n,
            Condition::AgentCompleted { agent, min_tasks } => {
                let completed = events.count_by_kind(crate::event_stream::EventKind::TaskCompleted);
                let agent_events = events.count_for_agent(agent);
                agent_events >= *min_tasks && completed > 0
            }
            Condition::AgentHasError(agent) => {
                events.recent(100, Some(agent))
                    .iter()
                    .any(|e| !e.success)
            }
            Condition::And(conditions) => {
                conditions.iter().all(|c| self.eval_condition(c, work_dir, iteration, events))
            }
        }
    }
}

// ── Control Shell ──────────────────────────────────────────────

pub struct ControlShell {
    rules: Vec<ActivationRule>,
    profiles: HashMap<String, AgentProfile>,
}

impl ControlShell {
    pub fn new() -> Self {
        Self { rules: Vec::new(), profiles: HashMap::new() }
    }

    pub fn register_profile(&mut self, profile: AgentProfile) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    pub fn add_rule(&mut self, rule: ActivationRule) {
        self.rules.push(rule);
        self.rules.sort_by_key(|r| -(r.priority as i32));
    }

    /// Evaluate all rules against workspace state and event stream.
    /// Returns the list of agents to activate.
    pub fn evaluate(&mut self, work_dir: &Path, iteration: usize, events: &EventStream) -> Vec<String> {
        let mut to_activate = Vec::new();

        // Always-active agents
        for (name, profile) in &self.profiles {
            if matches!(profile.activation, ActivationPolicy::AlwaysActive)
                && !to_activate.contains(name) {
                    to_activate.push(name.clone());
                }
        }

        // Condition-based rules
        for rule in &mut self.rules {
            if iteration < rule.last_activated + rule.cooldown_iterations {
                continue;
            }

            if rule.evaluate(work_dir, iteration, events) {
                for agent in &rule.activate {
                    if !to_activate.contains(agent) {
                        to_activate.push(agent.clone());
                    }
                }
                rule.last_activated = iteration;
            }
        }

        to_activate
    }

    pub fn profile(&self, name: &str) -> Option<&AgentProfile> {
        self.profiles.get(name)
    }

    /// Default rules for the scientific workflow pipeline.
    pub fn with_scientific_defaults(mut self) -> Self {
        self.add_rule(ActivationRule::new(
            "critique_on_findings",
            Condition::FileExistsAndNot(
                "researcher/findings.json".into(),
                "critic/critique.json".into(),
            ),
            vec!["critic"],
        ).with_priority(10));

        self.add_rule(ActivationRule::new(
            "synthesize_when_ready",
            Condition::AllFilesExist(vec![
                "researcher/findings.json".into(),
                "critic/critique.json".into(),
            ]),
            vec!["synthesizer"],
        ).with_priority(9));

        self.add_rule(ActivationRule::new(
            "review_after_synthesis",
            Condition::FileExists("synthesizer/synthesis.json".into()),
            vec!["reviewer"],
        ).with_priority(8));

        self.add_rule(ActivationRule::new(
            "opponent_on_hypothesis",
            Condition::FileExists("proposer/hypothesis.json".into()),
            vec!["opponent"],
        ).with_priority(10).with_cooldown(1));

        self.add_rule(ActivationRule::new(
            "judge_on_complete_debate",
            Condition::AllFilesExist(vec![
                "proposer/hypothesis.json".into(),
                "opponent/critique.json".into(),
            ]),
            vec!["judge"],
        ).with_priority(9));

        // 反向触发：judge 裁决为 REVISE → 重新激活 proposer 做第二轮反驳。
        // 这是"真实辩论"的核心——proposer 看到 opponent/judge 的批评后 refine 假设，
        // 而非单链路结束。proposer 的反驳逻辑（读 opponent critique）已存在，
        // 此前因缺触发规则而是死代码。
        //
        // 防无限循环：rebuttal.json 标记文件（proposer 第二轮写此文件后，
        // FileExistsAndNot 不再满足）。标记文件是主要防循环机制，故不设 cooldown。
        self.add_rule(ActivationRule::new(
            "proposer_revise_after_judge",
            Condition::And(vec![
                Condition::FileContains("judge/verdict.json".into(), "REVISE".into()),
                Condition::FileExistsAndNot(
                    "proposer/hypothesis.json".into(),
                    "proposer/rebuttal.json".into(),
                ),
            ]),
            vec!["proposer"],
        ).with_priority(11));

        self
    }

    /// Default rules for the orchestrator-workers pipeline.
    pub fn with_pipeline_defaults(mut self) -> Self {
        self.add_rule(ActivationRule::new(
            "research_after_plan",
            Condition::FileExistsAndNot(
                "planner/current_plan.json".into(),
                "researcher/findings.json".into(),
            ),
            vec!["researcher"],
        ).with_priority(10));

        self.add_rule(ActivationRule::new(
            "execute_after_research",
            Condition::FileExistsAndNot(
                "researcher/findings.json".into(),
                "executor/output.json".into(),
            ),
            vec!["executor"],
        ).with_priority(9));

        self.add_rule(ActivationRule::new(
            "write_after_review",
            Condition::FileExists("reviewer/review.json".into()),
            vec!["writer"],
        ).with_priority(8));

        self.add_rule(ActivationRule::new(
            "evaluate_after_write",
            Condition::FileExists("writer/draft.md".into()),
            vec!["evaluator"],
        ).with_priority(7));

        self
    }

    pub fn rule_count(&self) -> usize { self.rules.len() }
    pub fn profile_count(&self) -> usize { self.profiles.len() }
}

impl Default for ControlShell {
    fn default() -> Self { Self::new().with_scientific_defaults() }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "miniagent_cs_test_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn file_contains_matches_substring() {
        let dir = tmp_dir("fc_match");
        std::fs::create_dir_all(dir.join("judge")).ok();
        std::fs::write(dir.join("judge/verdict.json"), r#"{"verdict":"REVISE"}"#).ok();
        let work_dir = dir.as_path();

        let rule = ActivationRule::new(
            "test",
            Condition::FileContains("judge/verdict.json".into(), "REVISE".into()),
            vec!["proposer"],
        );
        let events = EventStream::new(work_dir);
        assert!(rule.evaluate(work_dir, 0, &events), "should match when file contains needle");
    }

    #[test]
    fn file_contains_no_match() {
        let dir = tmp_dir("fc_nomatch");
        std::fs::write(dir.join("verdict.json"), r#"{"verdict":"ACCEPT"}"#).ok();
        let work_dir = dir.as_path();

        let rule = ActivationRule::new(
            "test",
            Condition::FileContains("verdict.json".into(), "REVISE".into()),
            vec!["proposer"],
        );
        let events = EventStream::new(work_dir);
        assert!(!rule.evaluate(work_dir, 0, &events), "should not match when needle absent");
    }

    #[test]
    fn file_contains_missing_file_is_false() {
        let dir = tmp_dir("fc_missing");
        let work_dir = dir.as_path();
        let rule = ActivationRule::new(
            "test",
            Condition::FileContains("nonexistent.json".into(), "x".into()),
            vec!["proposer"],
        );
        let events = EventStream::new(work_dir);
        assert!(!rule.evaluate(work_dir, 0, &events), "missing file → false");
    }

    #[test]
    fn scientific_defaults_include_revise_rule() {
        // 确认 with_scientific_defaults 注册了反向触发规则
        let shell = ControlShell::new().with_scientific_defaults();
        assert!(shell.rule_count() >= 6, "should have revise rule + 5 originals");
    }

    #[test]
    fn revise_rule_triggers_on_judge_revise_verdict() {
        // 模拟辩论场景：proposer hypothesis + opponent critique + judge REVISE 存在，
        // 但 rebuttal.json 不存在 → 应激活 proposer（第二轮反驳）
        let dir = tmp_dir("revise_trigger");
        std::fs::create_dir_all(dir.join("proposer")).ok();
        std::fs::create_dir_all(dir.join("opponent")).ok();
        std::fs::create_dir_all(dir.join("judge")).ok();
        std::fs::write(dir.join("proposer/hypothesis.json"), r#"{"h":"x"}"#).ok();
        std::fs::write(dir.join("opponent/critique.json"), r#"{"c":"y"}"#).ok();
        std::fs::write(dir.join("judge/verdict.json"), r#"{"verdict":"REVISE"}"#).ok();
        // rebuttal.json 故意不存在

        let mut shell = ControlShell::new().with_scientific_defaults();
        let events = EventStream::new(&dir);
        let activated = shell.evaluate(&dir, 1, &events);
        assert!(activated.contains(&"proposer".to_string()),
            "REVISE verdict should re-activate proposer for rebuttal, got: {activated:?}");
    }

    #[test]
    fn revise_rule_inhibited_by_rebuttal_marker() {
        // proposer 已写 rebuttal.json → FileExistsAndNot 不满足 → 不应重激活
        let dir = tmp_dir("revise_inhibit");
        std::fs::create_dir_all(dir.join("proposer")).ok();
        std::fs::create_dir_all(dir.join("opponent")).ok();
        std::fs::create_dir_all(dir.join("judge")).ok();
        std::fs::write(dir.join("proposer/hypothesis.json"), r#"{"h":"x"}"#).ok();
        std::fs::write(dir.join("opponent/critique.json"), r#"{"c":"y"}"#).ok();
        std::fs::write(dir.join("judge/verdict.json"), r#"{"verdict":"REVISE"}"#).ok();
        std::fs::write(dir.join("proposer/rebuttal.json"), r#"{"round":"rebuttal"}"#).ok();

        let mut shell = ControlShell::new().with_scientific_defaults();
        let events = EventStream::new(&dir);
        let activated = shell.evaluate(&dir, 1, &events);
        assert!(!activated.contains(&"proposer".to_string()),
            "rebuttal.json marker should inhibit re-activation, got: {activated:?}");
    }
}
