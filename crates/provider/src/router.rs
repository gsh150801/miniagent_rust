use std::sync::Arc;

use miniagent_core::config::{InferenceConfig, TaskComplexity};
use miniagent_core::error::AgentError;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamResponse};

pub struct ProviderRouter {
    flash: Arc<dyn LlmProvider>,
    pro: Arc<dyn LlmProvider>,
    default: ProviderChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderChoice {
    Flash,
    Pro,
    Auto,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderInfo {
    pub name: &'static str,
    pub model: &'static str,
    pub has_thinking: bool,
    pub cost_per_1k_tokens: f64,
}

impl ProviderRouter {
    pub fn new(flash: Box<dyn LlmProvider>, pro: Box<dyn LlmProvider>) -> Self {
        Self::new_arc(flash.into(), pro.into())
    }

    /// Build a router from shared providers. Arc allows the same provider
    /// instance to be reused after runtime provider swaps.
    pub fn new_arc(flash: Arc<dyn LlmProvider>, pro: Arc<dyn LlmProvider>) -> Self {
        Self {
            flash,
            pro,
            default: ProviderChoice::Auto,
        }
    }

    pub fn with_default(mut self, choice: ProviderChoice) -> Self {
        self.default = choice;
        self
    }

    pub fn select(&self, complexity: TaskComplexity, force: Option<ProviderChoice>) -> &dyn LlmProvider {
        match force.unwrap_or(ProviderChoice::Auto) {
            ProviderChoice::Flash => self.flash.as_ref(),
            ProviderChoice::Pro => self.pro.as_ref(),
            ProviderChoice::Auto => match complexity {
                TaskComplexity::Simple | TaskComplexity::Moderate => self.flash.as_ref(),
                TaskComplexity::Complex | TaskComplexity::DeepResearch => self.pro.as_ref(),
            },
        }
    }

    /// Like [`select`] but returns an owned handle, so callers can hold the
    /// provider across `.await` points without borrowing from a lock guard.
    pub fn select_arc(&self, complexity: TaskComplexity, force: Option<ProviderChoice>) -> Arc<dyn LlmProvider> {
        match force.unwrap_or(ProviderChoice::Auto) {
            ProviderChoice::Flash => self.flash.clone(),
            ProviderChoice::Pro => self.pro.clone(),
            ProviderChoice::Auto => match complexity {
                TaskComplexity::Simple | TaskComplexity::Moderate => self.flash.clone(),
                TaskComplexity::Complex | TaskComplexity::DeepResearch => self.pro.clone(),
            },
        }
    }

    pub fn flash(&self) -> &dyn LlmProvider {
        self.flash.as_ref()
    }

    pub fn pro(&self) -> &dyn LlmProvider {
        self.pro.as_ref()
    }

    /// Owned handles for callers that need to hold the provider across awaits.
    pub fn flash_arc(&self) -> Arc<dyn LlmProvider> {
        self.flash.clone()
    }

    pub fn pro_arc(&self) -> Arc<dyn LlmProvider> {
        self.pro.clone()
    }

    pub async fn complete(
        &self,
        request: &CompletionRequest,
        complexity: TaskComplexity,
        force: Option<ProviderChoice>,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        self.select(complexity, force).complete(request, cancel).await
    }

    pub async fn stream(
        &self,
        request: &CompletionRequest,
        complexity: TaskComplexity,
        force: Option<ProviderChoice>,
        cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError> {
        self.select(complexity, force).stream(request, cancel).await
    }
}

// ── Composite implementations for Agent convenience ──────────

/// Configuration-aware inference that auto-selects Flash vs Pro
pub async fn routed_complete(
    router: &ProviderRouter,
    system: &str,
    messages: &[miniagent_core::message::Message],
    complexity: TaskComplexity,
    force: Option<ProviderChoice>,
    cancel: CancellationToken,
) -> Result<CompletionResponse, AgentError> {
    let provider = router.select(complexity, force);
    let config = match complexity {
        TaskComplexity::Simple => InferenceConfig::flash(),
        TaskComplexity::Moderate => InferenceConfig::flash(),
        TaskComplexity::Complex => InferenceConfig::pro(),
        TaskComplexity::DeepResearch => InferenceConfig::pro_deep(),
    };

    let request = CompletionRequest {
        system: system.to_string(),
        messages: messages.to_vec(),
        tools: vec![],
        config,
    };

    provider.complete(&request, cancel).await
}
