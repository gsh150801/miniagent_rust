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
