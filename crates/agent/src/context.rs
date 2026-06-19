use miniagent_core::config::TaskComplexity;
use miniagent_core::types::ProjectId;
use miniagent_provider::router::ProviderChoice;

#[derive(Debug, Clone)]
pub struct RunContext {
    pub system_prompt: String,
    pub complexity: TaskComplexity,
    pub provider_override: Option<ProviderChoice>,
    pub max_tool_iterations: usize,
    pub max_tokens: Option<u32>,
    pub checkpoint_enabled: bool,
    pub checkpoint_interval: Option<usize>,
    pub project_id: Option<ProjectId>,
    pub working_dir: String,
    /// When set, only these tool names are exposed to the LLM.
    /// The executor still holds all tools, but the LLM only sees the allowed subset.
    pub allowed_tools: Option<Vec<String>>,
}

impl RunContext {
    pub fn new(system_prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            complexity: TaskComplexity::Moderate,
            provider_override: None,
            max_tool_iterations: 10,
            max_tokens: None,
            checkpoint_enabled: false,
            checkpoint_interval: Some(5),
            project_id: None,
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into()),
            allowed_tools: None,
        }
    }

    pub fn with_complexity(mut self, complexity: TaskComplexity) -> Self {
        self.complexity = complexity;
        self
    }

    pub fn with_provider(mut self, choice: ProviderChoice) -> Self {
        self.provider_override = Some(choice);
        self
    }

    pub fn with_checkpoint(mut self) -> Self {
        self.checkpoint_enabled = true;
        self
    }

    pub fn with_project(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self.checkpoint_enabled = true;
        self
    }

    /// Restrict the tools visible to the LLM to a specific subset by name.
    pub fn with_allowed_tools(mut self, names: Vec<String>) -> Self {
        self.allowed_tools = Some(names);
        self
    }
}

impl Default for RunContext {
    fn default() -> Self {
        Self::new("You are a helpful AI assistant.")
    }
}
