use serde::{Deserialize, Serialize};

use crate::tool_binding::{ToolCategory, ToolRegistry};

// ── Agent Profile ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub role: AgentRoleType,
    pub capabilities: Vec<ToolCategory>,
    pub model_tier: ModelTier,
    pub tool_budget: usize,             // max tool calls per execution
    pub max_tokens_per_call: usize,
    // Blackboard 权限
    pub read_keys: Vec<String>,         // 可读的黑板 key
    pub write_keys: Vec<String>,        // 可写的黑板 key
    // 激活策略
    pub activation: ActivationPolicy,
    // 可通信的其他Agent
    pub can_message: Vec<String>,
    // 自动解析的工具列表 (运行时填充)
    pub resolved_tools: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentRoleType {
    Supervisor, Planner, Researcher, Critic, Synthesizer,
    Executor, Writer, Reviewer, Evaluator, Observer,
    Proposer, Opponent, Judge,
    Engineer, Analyst, PI, Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTier { Flash, Pro }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivationPolicy {
    AlwaysActive,
    OnCondition(String),      // condition expression
    OnSchedule(String),       // cron
    OnDemand,
}

impl Default for AgentProfile {
    fn default() -> Self {
        Self {
            name: String::new(), role: AgentRoleType::Custom,
            capabilities: vec![], model_tier: ModelTier::Flash,
            tool_budget: 20, max_tokens_per_call: 3000,
            read_keys: vec![], write_keys: vec![],
            activation: ActivationPolicy::AlwaysActive,
            can_message: vec![], resolved_tools: vec![],
        }
    }
}

impl AgentProfile {
    pub fn new(name: impl Into<String>, role: AgentRoleType) -> Self {
        Self { name: name.into(), role, ..Default::default() }
    }

    pub fn with_capabilities(mut self, caps: Vec<ToolCategory>) -> Self {
        self.capabilities = caps; self
    }

    pub fn with_model(mut self, tier: ModelTier) -> Self {
        self.model_tier = tier; self
    }

    pub fn with_blackboard(mut self, read: Vec<&str>, write: Vec<&str>) -> Self {
        self.read_keys = read.into_iter().map(|s| s.to_string()).collect();
        self.write_keys = write.into_iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn with_activation(mut self, policy: ActivationPolicy) -> Self {
        self.activation = policy; self
    }

    pub fn with_messaging(mut self, agents: Vec<&str>) -> Self {
        self.can_message = agents.into_iter().map(|s| s.to_string()).collect();
        self
    }

    /// Auto-resolve tools: match capabilities ∩ registry categories
    pub fn resolve_tools(&mut self, registry: &ToolRegistry) {
        let mut tools = Vec::new();
        for cap in &self.capabilities {
            for tool in registry.by_category(*cap) {
                if !tools.contains(&tool.name) {
                    tools.push(tool.name.clone());
                }
            }
        }
        self.resolved_tools = tools;
    }

    /// Check if this agent can read a blackboard key
    pub fn can_read(&self, key: &str) -> bool {
        self.read_keys.contains(&key.to_string())
    }

    /// Check if this agent can write to a blackboard key
    pub fn can_write(&self, key: &str) -> bool {
        self.write_keys.contains(&key.to_string())
    }
}

// ── Standard Profiles ──────────────────────────────────────────

pub fn researcher_profile() -> AgentProfile {
    AgentProfile::new("researcher", AgentRoleType::Researcher)
        .with_capabilities(vec![ToolCategory::Literature, ToolCategory::DataRetrieval, ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["search_results", "abstracts", "findings"], vec!["search_results", "abstracts", "findings"])
        .with_activation(ActivationPolicy::AlwaysActive)
        .with_messaging(vec!["critic", "synthesizer"])
}

pub fn critic_profile() -> AgentProfile {
    AgentProfile::new("critic", AgentRoleType::Critic)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["findings", "critique", "synthesis", "decisions"], vec!["critique", "decisions"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has_new('findings') && !blackboard.has('critique')".into()))
        .with_messaging(vec!["researcher", "synthesizer"])
}

pub fn synthesizer_profile() -> AgentProfile {
    AgentProfile::new("synthesizer", AgentRoleType::Synthesizer)
        .with_capabilities(vec![ToolCategory::FileSystem, ToolCategory::DataAnalysis])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["findings", "critique", "synthesis", "hypotheses"], vec!["synthesis", "hypotheses"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('findings') && blackboard.has('critique')".into()))
        .with_messaging(vec!["reviewer"])
}

pub fn reviewer_profile() -> AgentProfile {
    AgentProfile::new("reviewer", AgentRoleType::Reviewer)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["findings", "critique", "synthesis", "hypotheses", "decisions"], vec!["decisions"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('synthesis')".into()))
        .with_messaging(vec!["synthesizer"])
}

pub fn proposer_profile() -> AgentProfile {
    AgentProfile::new("proposer", AgentRoleType::Proposer)
        .with_capabilities(vec![ToolCategory::Literature, ToolCategory::DataRetrieval])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["opponent_critique"], vec!["hypothesis"])
        .with_activation(ActivationPolicy::AlwaysActive)
}

pub fn opponent_profile() -> AgentProfile {
    AgentProfile::new("opponent", AgentRoleType::Opponent)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["hypothesis"], vec!["opponent_critique", "scores"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('hypothesis')".into()))
}

pub fn judge_profile() -> AgentProfile {
    AgentProfile::new("judge", AgentRoleType::Judge)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["hypothesis", "opponent_critique", "scores"], vec!["verdict", "decision"])
        .with_activation(ActivationPolicy::OnCondition(
            "blackboard.has('hypothesis') && blackboard.has('opponent_critique')".into()
        ))
}

pub fn supervisor_profile() -> AgentProfile {
    AgentProfile::new("supervisor", AgentRoleType::Supervisor)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["plan", "progress", "todo"], vec!["plan", "todo"])
        .with_activation(ActivationPolicy::AlwaysActive)
        .with_messaging(vec!["planner", "evaluator"])
}

pub fn planner_profile() -> AgentProfile {
    AgentProfile::new("planner", AgentRoleType::Planner)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["plan", "evaluation"], vec!["current_plan", "plan_v1"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('plan')".into()))
        .with_messaging(vec!["supervisor", "researcher", "executor"])
}

pub fn executor_profile() -> AgentProfile {
    AgentProfile::new("executor", AgentRoleType::Executor)
        .with_capabilities(vec![ToolCategory::FileSystem, ToolCategory::CodeGeneration, ToolCategory::DataRetrieval])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["current_plan"], vec!["output", "report"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('current_plan')".into()))
        .with_messaging(vec!["planner", "writer"])
}

pub fn writer_profile() -> AgentProfile {
    AgentProfile::new("writer", AgentRoleType::Writer)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["findings", "critique", "synthesis", "review"], vec!["draft", "report"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('synthesis')".into()))
        .with_messaging(vec!["researcher", "reviewer"])
}

pub fn evaluator_profile() -> AgentProfile {
    AgentProfile::new("evaluator", AgentRoleType::Evaluator)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["review", "synthesis", "critique", "report"], vec!["evaluation"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('review')".into()))
        .with_messaging(vec!["planner", "supervisor"])
}

pub fn observer_profile() -> AgentProfile {
    AgentProfile::new("observer", AgentRoleType::Observer)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec![], vec!["context_summary", "snapshot"])
        .with_activation(ActivationPolicy::AlwaysActive)
        .with_messaging(vec![])
}

/// Build default profiles and auto-resolve tools
pub fn default_profiles(registry: &ToolRegistry) -> Vec<AgentProfile> {
    let mut profiles = vec![
        supervisor_profile(), planner_profile(),
        researcher_profile(), critic_profile(), synthesizer_profile(),
        executor_profile(), writer_profile(), reviewer_profile(), evaluator_profile(),
        observer_profile(),
        proposer_profile(), opponent_profile(), judge_profile(),
    ];
    for p in &mut profiles { p.resolve_tools(registry); }
    profiles
}
