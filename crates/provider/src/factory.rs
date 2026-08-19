//! Profile-driven provider construction.
//!
//! Single entry point for turning a [`ModelProfile`] (from the runtime model
//! registry) into an [`LlmProvider`] instance. All call sites that used to
//! hardcode `DeepSeekFlash::new` / `StepFunFlash::new` / `MiniMaxFlash::new`
//! should go through [`build_provider`] / [`build_provider_pair`] instead.

use std::sync::Arc;

use miniagent_core::models::{DebateRole, ModelKind, ModelProfile, ModelRegistry};
use miniagent_core::settings::AppConfig;

use crate::deepseek::DeepSeekClient;
use crate::minimax::MiniMaxClient;
use crate::stepfun::StepFunClient;
use crate::traits::LlmProvider;

/// Which tier to build — flash (default) or pro (reasoning-heavy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTier {
    Flash,
    Pro,
}

/// Build a single provider for `profile` at the given tier.
pub fn build_provider(profile: &ModelProfile, tier: ProviderTier) -> Result<Box<dyn LlmProvider>, String> {
    let key = profile.resolve_key().ok_or_else(|| {
        format!(
            "模型配置 “{}” 缺少 API key（{} 未设置）",
            profile.display_name,
            profile.api_key_env.as_deref().unwrap_or("且未内嵌 key")
        )
    })?;

    let model = match tier {
        ProviderTier::Flash => &profile.model_name,
        ProviderTier::Pro => profile.pro_model(),
    };

    let provider: Box<dyn LlmProvider> = match profile.kind {
        ModelKind::DeepSeek => {
            let is_reasoner = tier == ProviderTier::Pro;
            Box::new(
                DeepSeekClient::new(&key, model, is_reasoner)
                    .with_base_url(profile.base_url.clone())
                    .with_model_name(model),
            )
        }
        ModelKind::StepFun => Box::new(
            StepFunClient::with_model(&key, model)
                .with_base_url(profile.base_url.clone())
                .with_model_name(model),
        ),
        // MiniMaxClient speaks both OpenAI-chat-completions and Anthropic
        // Messages protocols (auto-detected from base_url), so it serves
        // minimax / openai-compatible / anthropic-compatible profiles alike.
        ModelKind::MiniMax | ModelKind::OpenAiCompatible | ModelKind::AnthropicCompatible => {
            Box::new(
                MiniMaxClient::with_model(&key, model)
                    .with_base_url(profile.base_url.clone())
                    .with_model_name(model),
            )
        }
    };
    Ok(provider)
}

/// Build the (flash, pro) pair used by `Agent::new` / `Agent::replace_providers`.
pub fn build_provider_pair(
    profile: &ModelProfile,
) -> Result<(Arc<dyn LlmProvider>, Arc<dyn LlmProvider>), String> {
    let flash: Arc<dyn LlmProvider> = build_provider(profile, ProviderTier::Flash)?.into();
    let pro: Arc<dyn LlmProvider> = build_provider(profile, ProviderTier::Pro)?.into();
    Ok((flash, pro))
}

/// Build the provider serving a debate role from an already-resolved
/// profile (caller resolves via [`ModelRegistry::role_profile`]). Debate
/// calls are reasoning-heavy → Pro tier.
pub fn resolve_role_provider_from(
    profile: &ModelProfile,
) -> Result<Box<dyn LlmProvider>, String> {
    build_provider(profile, ProviderTier::Pro)
}

/// Build the provider serving a debate role. Resolution order:
/// ⚙️-persisted selection (models.json) → `DEBATE_*_MODEL` env var → active
/// main model. Debate calls are reasoning-heavy, so the Pro tier of the
/// resolved profile is used.
pub fn resolve_role_provider(
    registry: &ModelRegistry,
    role: DebateRole,
) -> Result<Box<dyn LlmProvider>, String> {
    let profile = registry.role_profile(role);
    let provider = build_provider(profile, ProviderTier::Pro)?;
    if profile.id != registry.active_id() {
        tracing::info!(
            role = role.key(),
            profile = %profile.display_name,
            model = profile.pro_model(),
            "debate role routed to non-default profile"
        );
    }
    Ok(provider)
}

/// Convenience: registry from `config` + all three debate-role providers in
/// `(proposer, opponent, judge)` order.
pub fn resolve_debate_providers(
    config: &AppConfig,
) -> Result<
    (
        Box<dyn LlmProvider>,
        Box<dyn LlmProvider>,
        Box<dyn LlmProvider>,
    ),
    String,
> {
    let registry = ModelRegistry::load(config);
    Ok((
        resolve_role_provider(&registry, DebateRole::Proposer)?,
        resolve_role_provider(&registry, DebateRole::Opponent)?,
        resolve_role_provider(&registry, DebateRole::Judge)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(kind: ModelKind) -> ModelProfile {
        ModelProfile {
            id: "t".into(),
            display_name: "test".into(),
            kind,
            base_url: "https://example.invalid".into(),
            model_name: "m-flash".into(),
            pro_model_name: Some("m-pro".into()),
            api_key: Some("sk-test".into()),
            api_key_env: None,
            builtin: false,
        }
    }

    #[test]
    fn builds_every_kind() {
        for kind in [
            ModelKind::DeepSeek,
            ModelKind::StepFun,
            ModelKind::MiniMax,
            ModelKind::OpenAiCompatible,
            ModelKind::AnthropicCompatible,
        ] {
            let (flash, pro) = build_provider_pair(&profile(kind)).expect("build pair");
            let _ = (flash, pro); // constructed without panic is all we assert
        }
    }

    #[test]
    fn missing_key_is_an_error() {
        let mut p = profile(ModelKind::DeepSeek);
        p.api_key = None;
        assert!(build_provider_pair(&p).is_err());
    }
}
