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

/// Build the (flash, pro) provider pair used by `Agent::new` / `Agent::replace_providers`.
pub fn build_provider_pair(
    profile: &ModelProfile,
) -> Result<(Arc<dyn LlmProvider>, Arc<dyn LlmProvider>), String> {
    let flash: Arc<dyn LlmProvider> = build_provider(profile, ProviderTier::Flash)?.into();
    let pro: Arc<dyn LlmProvider> = build_provider(profile, ProviderTier::Pro)?.into();
    Ok((flash, pro))
}

/// Build the boxed (flash, pro) pair for the *active* profile of the runtime
/// registry loaded from `config`. Single replacement for the `make_providers`
/// helpers that used to be duplicated across the CLI and research pipeline.
pub fn active_provider_pair(
    config: &AppConfig,
) -> Result<(Box<dyn LlmProvider>, Box<dyn LlmProvider>), String> {
    let registry = ModelRegistry::load(config);
    let active = registry.active().clone();
    let flash = build_provider(&active, ProviderTier::Flash)?;
    let pro = build_provider(&active, ProviderTier::Pro)?;
    Ok((flash, pro))
}

/// Build a code-generation fallback provider from a *different* vendor
/// family than the active one (DeepSeek → StepFun → MiniMax preference).
///
/// Long-form code generation is the stage most sensitive to a single
/// provider's degradation: when the active model repeatedly returns empty
/// content even after its built-in larger-budget retry (observed live:
/// entire analysis tasks died on empty output during a MiniMax episode),
/// an alternate-vendor client recovers the task. Returns `None` when no
/// other family has an API key configured, or when the active family IS
/// deepseek and nothing else is available.
pub fn codegen_fallback_provider(config: &AppConfig) -> Option<Box<dyn LlmProvider>> {
    codegen_fallback_providers(config).into_iter().next()
}

/// All cross-family code-generation fallback providers, in preference order
/// (DeepSeek → StepFun → MiniMax, never the active family). Callers should
/// try them in order: when the first fallback ALSO fails (observed live:
/// MiniMax hit its token cap while the DeepSeek account was out of balance),
/// the next family still rescues the task.
/// Pure selection: ordered families minus banned ones. Split from the
/// constructor so the ban filtering is unit-testable without building
/// clients.
fn fallback_chain(active: CodegenFamily, available: [bool; 3]) -> Vec<CodegenFamily> {
    pick_fallback_order(active, available)
        .into_iter()
        .filter(|f| !is_family_banned(*f))
        .collect()
}

pub fn codegen_fallback_providers(config: &AppConfig) -> Vec<Box<dyn LlmProvider>> {
    use crate::deepseek::DeepSeekFlash;
    use crate::minimax::MiniMaxFlash;
    use crate::stepfun::StepFunFlash;

    let active = if config.is_stepfun() {
        CodegenFamily::StepFun
    } else if config.is_minimax() {
        CodegenFamily::MiniMax
    } else {
        CodegenFamily::DeepSeek
    };
    let available = [
        config.deepseek_api_key.is_some(),
        config.stepfun_api_key.is_some(),
        config.minimax_api_key.is_some(),
    ];
    let mut out: Vec<Box<dyn LlmProvider>> = Vec::new();
    for family in fallback_chain(active, available) {
        match family {
            CodegenFamily::DeepSeek => {
                if let Some(key) = config.deepseek_api_key.as_ref() {
                    out.push(Box::new(DeepSeekFlash::new(key)));
                }
            }
            CodegenFamily::StepFun => {
                if let Some(key) = config.stepfun_api_key.as_ref() {
                    out.push(Box::new(StepFunFlash::new(key)));
                }
            }
            CodegenFamily::MiniMax => {
                if let Some(key) = config.minimax_api_key.as_ref() {
                    out.push(Box::new(MiniMaxFlash::new(key)));
                }
            }
        }
    }
    out
}

/// Vendor families usable for cross-family code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenFamily {
    DeepSeek,
    StepFun,
    MiniMax,
}

/// Process-global ban list for vendor families whose accounts are unusable —
/// 401 (bad key) / 402 (out of balance) / 403 (forbidden). Live incident: the
/// fallback chain walked DeepSeek → 402 "Insufficient Balance" → StepFun →
/// 400 "no active step plan subscription" on EVERY MiniMax transient
/// failure, because fallback selection only checked key PRESENCE. Mirrors
/// the web_search backend health-disable pattern. Bans last for the process
/// lifetime: an unpaid account does not recover mid-run.
static FAMILY_BANS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn family_bit(family: CodegenFamily) -> u8 {
    match family {
        CodegenFamily::DeepSeek => 1 << 0,
        CodegenFamily::StepFun => 1 << 1,
        CodegenFamily::MiniMax => 1 << 2,
    }
}

/// Ban a vendor family for the remainder of this process (billing/auth
/// failure observed). Idempotent.
pub fn ban_family(family: CodegenFamily, reason: &str) {
    use std::sync::atomic::Ordering;
    FAMILY_BANS.fetch_or(family_bit(family), Ordering::SeqCst);
    let name = match family {
        CodegenFamily::DeepSeek => "deepseek",
        CodegenFamily::StepFun => "stepfun",
        CodegenFamily::MiniMax => "minimax",
    };
    tracing::warn!(
        family = name,
        reason = %reason,
        "provider family BANNED for the rest of this process (auth/billing error); fallback chain will skip it"
    );
}

/// Whether a family is currently banned.
pub fn is_family_banned(family: CodegenFamily) -> bool {
    use std::sync::atomic::Ordering;
    FAMILY_BANS.load(Ordering::SeqCst) & family_bit(family) != 0
}

/// True when the HTTP status is an account-level failure that will not
/// recover on retry within this run.
pub fn is_account_error(status: reqwest::StatusCode) -> bool {
    matches!(
        status.as_u16(),
        401 | 402 | 403
    )
}

/// Pure fallback selection: prefer DeepSeek, then StepFun, then MiniMax —
/// never the active family itself. `available` = [deepseek, stepfun,
/// minimax] key presence. Returns ALL usable families in order so callers
/// can walk past a broken first choice. Unit-testable without real config.
fn pick_fallback_order(
    active: CodegenFamily,
    available: [bool; 3],
) -> Vec<CodegenFamily> {
    const ORDER: [CodegenFamily; 3] = [
        CodegenFamily::DeepSeek,
        CodegenFamily::StepFun,
        CodegenFamily::MiniMax,
    ];
    ORDER.iter()
        .zip(available)
        .filter(|(family, present)| **family != active && *present)
        .map(|(family, _)| *family)
        .collect()
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
    use CodegenFamily as F;

    fn codegen_fallback_providers_for_test(active: CodegenFamily, available: [bool; 3]) -> Vec<CodegenFamily> {
        fallback_chain(active, available)
    }

    #[test]
    fn account_error_bans_family_out_of_fallback_chain() {
        // 402/401/403 must ban the family for the process lifetime; the
        // fallback builder then skips it. (Live incident: every MiniMax
        // transient failure walked into an out-of-balance DeepSeek 402.)
        assert!(is_account_error(reqwest::StatusCode::from_u16(402).unwrap()));
        assert!(is_account_error(reqwest::StatusCode::from_u16(401).unwrap()));
        assert!(!is_account_error(reqwest::StatusCode::from_u16(429).unwrap()));
        assert!(!is_account_error(reqwest::StatusCode::from_u16(400).unwrap()));

        // Start from a clean slate for DeepSeek.
        let before = is_family_banned(F::DeepSeek);
        ban_family(F::DeepSeek, "test: 402 Insufficient Balance");
        assert!(is_family_banned(F::DeepSeek), "ban is visible immediately");

        // The ban affects the BUILT chain, not the pure order function.
        let chain = codegen_fallback_providers_for_test(F::MiniMax, [true, true, true]);
        assert!(
            !chain.contains(&F::DeepSeek),
            "banned family must be filtered from the fallback chain: {chain:?}"
        );
        assert!(chain.contains(&F::StepFun));
        if !before {
            // best effort unban for other tests in the same process
            FAMILY_BANS.fetch_and(!family_bit(F::DeepSeek), std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn codegen_fallback_prefers_other_family() {
        use CodegenFamily as F;
        // Preference order DeepSeek > StepFun > MiniMax, never the active one.
        let first = |active, avail| {
            pick_fallback_order(active, avail).into_iter().next()
        };
        assert_eq!(first(F::MiniMax, [true, true, false]), Some(F::DeepSeek));
        // Full order: when the first fallback is broken the caller walks on.
        assert_eq!(
            pick_fallback_order(F::MiniMax, [true, true, true]),
            vec![F::DeepSeek, F::StepFun]
        );
        assert_eq!(
            pick_fallback_order(F::MiniMax, [false, true, true]),
            vec![F::StepFun]
        );
        assert_eq!(first(F::StepFun, [true, true, true]), Some(F::DeepSeek));
        // Active deepseek with only its own key → no cross-family option.
        assert_eq!(pick_fallback_order(F::DeepSeek, [true, false, false]), vec![]);
        // Active deepseek with only minimax key → MiniMax fallback.
        assert_eq!(
            first(F::DeepSeek, [false, false, true]),
            Some(F::MiniMax)
        );
        // Nothing available anywhere.
        assert_eq!(first(F::MiniMax, [false, false, false]), None);
    }

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
