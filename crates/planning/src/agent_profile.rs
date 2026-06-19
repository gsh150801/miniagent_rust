use serde::{Deserialize, Serialize};

use miniagent_core::ModelTier;

use crate::tool_binding::{ToolCategory, ToolRegistry};

// ── Role Context Dependencies (single source of truth) ─────────
//
// 每个角色构建上下文时需要读取哪些其他角色的输出。
// 这是整个系统唯一的角色依赖表——AgentProfile 通过它填充
// `depends_on_agents` 字段，ContextManager/EventStream 也通过它
// 查询依赖（替代历史上两份不一致的 `role_dependencies()` 函数）。
//
// 语义说明：
// - "depends on" = 该角色的上下文构建需要参考这些角色的最近产出
// - observer 约定为"全可见"，由调用方特殊处理，此处留空
pub const ROLE_CONTEXT_DEPS: &[(&str, &[&str])] = &[
    ("supervisor",  &["planner", "evaluator"]),
    ("planner",     &["supervisor", "evaluator"]),
    ("researcher",  &["planner"]),
    ("critic",      &["researcher"]),
    ("synthesizer", &["researcher", "critic"]),
    ("executor",    &["planner"]),
    ("writer",      &["researcher", "critic", "synthesizer", "reviewer"]),
    ("reviewer",    &["researcher", "critic", "synthesizer", "writer"]),
    ("evaluator",   &["reviewer", "writer", "synthesizer"]),
    ("observer",    &[]),
    ("proposer",    &["researcher", "opponent", "judge"]),
    ("opponent",    &["proposer"]),
    ("judge",       &["proposer", "opponent"]),
];

/// 查询某个角色的上下文依赖（数据驱动）。
///
/// 数据源：[`ROLE_CONTEXT_DEPS`]。未知角色返回空切片。
pub fn role_context_deps(role: &str) -> &'static [&'static str] {
    ROLE_CONTEXT_DEPS
        .iter()
        .find(|(name, _)| *name == role)
        .map(|(_, deps)| *deps)
        .unwrap_or(&[])
}

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
    // 该角色在构建上下文时需要读取哪些其他角色的输出（数据驱动的依赖图）
    pub depends_on_agents: Vec<String>,
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

// `ModelTier` 统一从 `miniagent_core` 引入（见文件顶部 use），此处不再重复定义。

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
            can_message: vec![], depends_on_agents: vec![], resolved_tools: vec![],
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

    /// Declare which roles' outputs this agent reads when building its context.
    /// Replaces the legacy hardcoded `role_dependencies()` tables.
    pub fn with_context_deps(mut self, agents: Vec<&str>) -> Self {
        self.depends_on_agents = agents.into_iter().map(|s| s.to_string()).collect();
        self
    }

    /// Names of roles whose outputs feed this agent's context.
    pub fn context_deps(&self) -> &[String] {
        &self.depends_on_agents
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
        .with_context_deps(vec!["planner"])
}

pub fn critic_profile() -> AgentProfile {
    AgentProfile::new("critic", AgentRoleType::Critic)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["findings", "critique", "synthesis", "decisions"], vec!["critique", "decisions"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has_new('findings') && !blackboard.has('critique')".into()))
        .with_messaging(vec!["researcher", "synthesizer"])
        .with_context_deps(vec!["researcher"])
}

pub fn synthesizer_profile() -> AgentProfile {
    AgentProfile::new("synthesizer", AgentRoleType::Synthesizer)
        .with_capabilities(vec![ToolCategory::FileSystem, ToolCategory::DataAnalysis])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["findings", "critique", "synthesis", "hypotheses"], vec!["synthesis", "hypotheses"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('findings') && blackboard.has('critique')".into()))
        .with_messaging(vec!["reviewer"])
        .with_context_deps(vec!["researcher", "critic"])
}

pub fn reviewer_profile() -> AgentProfile {
    AgentProfile::new("reviewer", AgentRoleType::Reviewer)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["findings", "critique", "synthesis", "hypotheses", "decisions"], vec!["decisions"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('synthesis')".into()))
        .with_messaging(vec!["synthesizer"])
        .with_context_deps(vec!["researcher", "critic", "synthesizer", "writer"])
}

pub fn proposer_profile() -> AgentProfile {
    AgentProfile::new("proposer", AgentRoleType::Proposer)
        .with_capabilities(vec![ToolCategory::Literature, ToolCategory::DataRetrieval])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["opponent_critique"], vec!["hypothesis"])
        .with_activation(ActivationPolicy::AlwaysActive)
        .with_context_deps(vec!["researcher", "opponent", "judge"])
}

pub fn opponent_profile() -> AgentProfile {
    AgentProfile::new("opponent", AgentRoleType::Opponent)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["hypothesis"], vec!["opponent_critique", "scores"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('hypothesis')".into()))
        .with_context_deps(vec!["proposer"])
}

pub fn judge_profile() -> AgentProfile {
    AgentProfile::new("judge", AgentRoleType::Judge)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["hypothesis", "opponent_critique", "scores"], vec!["verdict", "decision"])
        .with_activation(ActivationPolicy::OnCondition(
            "blackboard.has('hypothesis') && blackboard.has('opponent_critique')".into()
        ))
        .with_context_deps(vec!["proposer", "opponent"])
}

pub fn supervisor_profile() -> AgentProfile {
    AgentProfile::new("supervisor", AgentRoleType::Supervisor)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["plan", "progress", "todo"], vec!["plan", "todo"])
        .with_activation(ActivationPolicy::AlwaysActive)
        .with_messaging(vec!["planner", "evaluator"])
        .with_context_deps(vec!["planner", "evaluator"])
}

pub fn planner_profile() -> AgentProfile {
    AgentProfile::new("planner", AgentRoleType::Planner)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["plan", "evaluation"], vec!["current_plan", "plan_v1"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('plan')".into()))
        .with_messaging(vec!["supervisor", "researcher", "executor"])
        .with_context_deps(vec!["supervisor", "evaluator"])
}

pub fn executor_profile() -> AgentProfile {
    AgentProfile::new("executor", AgentRoleType::Executor)
        .with_capabilities(vec![ToolCategory::FileSystem, ToolCategory::CodeGeneration, ToolCategory::DataRetrieval])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec!["current_plan"], vec!["output", "report"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('current_plan')".into()))
        .with_messaging(vec!["planner", "writer"])
        .with_context_deps(vec!["planner"])
}

pub fn writer_profile() -> AgentProfile {
    AgentProfile::new("writer", AgentRoleType::Writer)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["findings", "critique", "synthesis", "review"], vec!["draft", "report"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('synthesis')".into()))
        .with_messaging(vec!["researcher", "reviewer"])
        .with_context_deps(vec!["researcher", "critic", "synthesizer", "reviewer"])
}

pub fn evaluator_profile() -> AgentProfile {
    AgentProfile::new("evaluator", AgentRoleType::Evaluator)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Pro)
        .with_blackboard(vec!["review", "synthesis", "critique", "report"], vec!["evaluation"])
        .with_activation(ActivationPolicy::OnCondition("blackboard.has('review')".into()))
        .with_messaging(vec!["planner", "supervisor"])
        .with_context_deps(vec!["reviewer", "writer", "synthesizer"])
}

pub fn observer_profile() -> AgentProfile {
    AgentProfile::new("observer", AgentRoleType::Observer)
        .with_capabilities(vec![ToolCategory::FileSystem])
        .with_model(ModelTier::Flash)
        .with_blackboard(vec![], vec!["context_summary", "snapshot"])
        .with_activation(ActivationPolicy::AlwaysActive)
        .with_messaging(vec![])
        // observer 看到一切：空依赖列表由 ContextManager/EventStream 特殊处理
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

/// 查询某个角色的上下文依赖（数据驱动，替代旧的硬编码 role_dependencies 表）。
///
/// 优先读取 `profiles` 中该角色的 `depends_on_agents` 字段（允许自定义 profile
/// 覆盖默认依赖）；若 profile 未声明或不存在，回退到 [`ROLE_CONTEXT_DEPS`]。
/// `observer` 的依赖由调用方决定是否注入"全部"事件，此函数返回空。
pub fn context_dependencies_of<'a>(profiles: &'a [AgentProfile], role: &str) -> Vec<&'a str> {
    if let Some(p) = profiles.iter().find(|p| p.name == role) {
        if !p.depends_on_agents.is_empty() {
            return p.depends_on_agents.iter().map(|s| s.as_str()).collect();
        }
    }
    role_context_deps(role).iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_context_deps_known_roles() {
        // 验证静态表覆盖全部 13 个标准角色，且关键字段符合预期
        assert_eq!(role_context_deps("supervisor"), ["planner", "evaluator"]);
        assert_eq!(role_context_deps("planner"), ["supervisor", "evaluator"]);
        assert_eq!(role_context_deps("researcher"), ["planner"]);
        assert_eq!(role_context_deps("critic"), ["researcher"]);
        assert_eq!(role_context_deps("synthesizer"), ["researcher", "critic"]);
        assert_eq!(role_context_deps("executor"), ["planner"]);
        assert_eq!(
            role_context_deps("writer"),
            ["researcher", "critic", "synthesizer", "reviewer"]
        );
        assert_eq!(
            role_context_deps("reviewer"),
            ["researcher", "critic", "synthesizer", "writer"]
        );
        assert_eq!(
            role_context_deps("evaluator"),
            ["reviewer", "writer", "synthesizer"]
        );
        assert!(role_context_deps("observer").is_empty());
        assert_eq!(
            role_context_deps("proposer"),
            ["researcher", "opponent", "judge"]
        );
        assert_eq!(role_context_deps("opponent"), ["proposer"]);
        assert_eq!(role_context_deps("judge"), ["proposer", "opponent"]);
    }

    #[test]
    fn role_context_deps_unknown_role_is_empty() {
        assert!(role_context_deps("nonexistent_role").is_empty());
        assert!(role_context_deps("").is_empty());
    }

    #[test]
    fn default_profiles_declare_matching_deps() {
        // 每个 default profile 的 depends_on_agents 必须与静态表一致——
        // 这是"单一数据源"不变量的回归保护：防止任一侧被修改后漂移。
        for profile in [
            supervisor_profile(),
            planner_profile(),
            researcher_profile(),
            critic_profile(),
            synthesizer_profile(),
            executor_profile(),
            writer_profile(),
            reviewer_profile(),
            evaluator_profile(),
            observer_profile(),
            proposer_profile(),
            opponent_profile(),
            judge_profile(),
        ] {
            let from_profile: Vec<&str> = profile.depends_on_agents.iter().map(|s| s.as_str()).collect();
            let from_table = role_context_deps(&profile.name);
            assert_eq!(
                from_profile, from_table,
                "profile `{}` 声明的依赖与 ROLE_CONTEXT_DEPS 不一致",
                profile.name
            );
        }
    }

    #[test]
    fn context_dependencies_of_falls_back_to_static_table() {
        // 无 profile 列表时回退到静态表
        let deps = context_dependencies_of(&[], "researcher");
        assert_eq!(deps, ["planner"]);
    }

    #[test]
    fn context_dependencies_of_custom_profile_overrides() {
        // 自定义 profile 的 depends_on_agents 优先于静态表
        let custom = AgentProfile::new("researcher", AgentRoleType::Researcher)
            .with_context_deps(vec!["custom_dep"]);
        let profiles = [custom];
        let deps = context_dependencies_of(&profiles, "researcher");
        assert_eq!(deps, ["custom_dep"]);
    }
}
