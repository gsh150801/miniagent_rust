//! Runtime LLM model registry.
//!
//! Replaces the old "one hardcoded model name per provider crate" scheme with
//! a persisted set of [`ModelProfile`]s. Profiles come from two sources:
//!
//! * built-ins, derived from `AppConfig` / `.env` on startup (deepseek,
//!   stepfun, minimax), and
//! * user-defined ones, added at runtime via the server UI and persisted to
//!   `models.json` in the working directory (gitignored — contains API keys).
//!
//! The active profile decides which provider/model every component uses, and
//! can be switched at runtime without restarting the process.

use crate::secrets::ApiKey;
use crate::settings::AppConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Protocol family of a profile. Determines which client implementation
/// serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// DeepSeek OpenAI-compatible API (flash/pro tier split).
    DeepSeek,
    /// StepFun plan API.
    StepFun,
    /// MiniMax (protocol auto-detected from base_url).
    MiniMax,
    /// Any OpenAI-compatible endpoint (siliconflow, openrouter, vllm, ...).
    #[serde(alias = "openai_compatible")]
    OpenAiCompatible,
    /// Any Anthropic Messages-compatible endpoint.
    #[serde(alias = "anthropic_compatible")]
    AnthropicCompatible,
}

impl ModelKind {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().replace('-', "_").as_str() {
            "deepseek" | "deep_seek" => Some(Self::DeepSeek),
            "stepfun" | "step_fun" => Some(Self::StepFun),
            "minimax" | "mini_max" => Some(Self::MiniMax),
            "openai" | "openai_compatible" => Some(Self::OpenAiCompatible),
            "anthropic" | "anthropic_compatible" => Some(Self::AnthropicCompatible),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek",
            Self::StepFun => "StepFun",
            Self::MiniMax => "MiniMax",
            Self::OpenAiCompatible => "OpenAI 兼容",
            Self::AnthropicCompatible => "Anthropic 兼容",
        }
    }

    /// Emoji/short glyph shown in the UI next to the provider name. Chosen
    /// by family (deepseek=🐳, stepfun=⚡, etc.) so the user can spot the
    /// family at a glance without reading the label text.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::DeepSeek => "🐳",
            Self::StepFun => "⚡",
            Self::MiniMax => "🌊",
            Self::OpenAiCompatible => "🔌",
            Self::AnthropicCompatible => "🧠",
        }
    }

    /// Short identifier for client-side grouping/filtering.
    pub fn slug(&self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
            Self::StepFun => "stepfun",
            Self::MiniMax => "minimax",
            Self::OpenAiCompatible => "openai_compatible",
            Self::AnthropicCompatible => "anthropic_compatible",
        }
    }

    /// Iterate every supported family (server-driven; the frontend asks via
    /// /api/models for the kinds list, never hardcodes the enums).
    pub fn all() -> [ModelKind; 5] {
        [Self::DeepSeek, Self::StepFun, Self::MiniMax,
         Self::OpenAiCompatible, Self::AnthropicCompatible]
    }
}

/// A single configurable LLM endpoint/model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Stable identifier ("builtin-deepseek", "custom-<uuid8>", ...).
    pub id: String,
    /// Human-readable name shown in the UI.
    pub display_name: String,
    /// Protocol family.
    pub kind: ModelKind,
    /// API base URL (protocol derived from kind + URL for MiniMax).
    pub base_url: String,
    /// Model name sent to the API (flash tier).
    pub model_name: String,
    /// Optional separate pro/reasoning-tier model; defaults to `model_name`.
    #[serde(default)]
    pub pro_model_name: Option<String>,
    /// API key. Built-ins resolve lazily from env; customs store it here
    /// (models.json is gitignored).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Env var name to resolve the key from (built-ins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// True for built-in profiles (cannot be deleted via the UI).
    #[serde(default)]
    pub builtin: bool,
}

impl ModelProfile {
    /// Resolve the effective API key: explicit value first, env var second.
    pub fn resolve_key(&self) -> Option<ApiKey> {
        if let Some(k) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
            return Some(ApiKey::new(k));
        }
        if let Some(var) = self.api_key_env.as_deref() {
            return ApiKey::from_env(var);
        }
        None
    }

    /// Masked key for API responses (never leak the full secret to the UI).
    pub fn masked_key(&self) -> String {
        match self.resolve_key() {
            Some(k) => k.masked(),
            None => "(未设置)".into(),
        }
    }

    pub fn pro_model(&self) -> &str {
        self.pro_model_name.as_deref().unwrap_or(&self.model_name)
    }
}

/// Debate role. Each role can be served by a different model profile; all
/// default to the active main model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebateRole {
    Proposer,
    Opponent,
    Judge,
}

impl DebateRole {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Proposer => "proposer",
            Self::Opponent => "opponent",
            Self::Judge => "judge",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Proposer => "正方 (Proposer)",
            Self::Opponent => "反方 (Opponent)",
            Self::Judge => "裁判 (Judge)",
        }
    }
}

/// Per-role profile selection (None = active main model). Persisted inside
/// `models.json` so both the server UI and the CLI read the same choice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebateRoleSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opponent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
}

impl DebateRoleSelection {
    pub fn get(&self, role: DebateRole) -> Option<&String> {
        match role {
            DebateRole::Proposer => self.proposer.as_ref(),
            DebateRole::Opponent => self.opponent.as_ref(),
            DebateRole::Judge => self.judge.as_ref(),
        }
    }

    pub fn set(&mut self, role: DebateRole, id: Option<String>) {
        match role {
            DebateRole::Proposer => self.proposer = id,
            DebateRole::Opponent => self.opponent = id,
            DebateRole::Judge => self.judge = id,
        }
    }
}

/// Serializable state persisted to `models.json`.
#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryFile {
    #[serde(default)]
    active_id: Option<String>,
    #[serde(default)]
    custom: Vec<ModelProfile>,
    /// Per-role debate model selection (server ⚙️ settings). Missing on
    /// legacy files → all roles fall back to the env / active main model.
    #[serde(default)]
    debate: DebateRoleSelection,
}

/// Registry of all known profiles + the active selection.
#[derive(Debug)]
pub struct ModelRegistry {
    builtins: Vec<ModelProfile>,
    custom: Vec<ModelProfile>,
    active_id: String,
    /// Debate role selection persisted via the server UI (models.json).
    debate: DebateRoleSelection,
    /// Env-var defaults (`.env` DEBATE_*_MODEL); lower priority than `debate`.
    env_debate: DebateRoleSelection,
    path: PathBuf,
}

impl ModelRegistry {
    /// Load the registry: built-ins from `AppConfig`, customs + active
    /// selection from `models.json` (if present).
    pub fn load(config: &AppConfig) -> Self {
        let builtins = Self::builtin_profiles(config);
        let path = crate::paths::models_file();
        // A malformed registry must never silently fall back to the default
        // provider — that sends traffic to the wrong endpoint with no trace
        // of why. Log loudly and continue with the default so the operator
        // can fix the file.
        let file: RegistryFile = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| match serde_json::from_str::<RegistryFile>(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!(
                        path = %path.display(),
                        error = %e,
                        "models.json failed to parse — falling back to the default provider; \
                         fix the file to restore custom model selection"
                    );
                    None
                }
            })
            .unwrap_or_default();

        let default_active = builtins
            .iter()
            .find(|p| match config.provider.as_str() {
                "stepfun" => p.kind == ModelKind::StepFun,
                "minimax" => p.kind == ModelKind::MiniMax,
                _ => p.kind == ModelKind::DeepSeek,
            })
            .map(|p| p.id.clone())
            .unwrap_or_else(|| "builtin-deepseek".into());

        let active_id = file
            .active_id
            .filter(|id| {
                builtins.iter().any(|p| &p.id == id)
                    || file.custom.iter().any(|p| &p.id == id)
            })
            .unwrap_or(default_active);

        let env_debate = DebateRoleSelection {
            proposer: config.debate_proposer_model.clone(),
            opponent: config.debate_opponent_model.clone(),
            judge: config.debate_judge_model.clone(),
        };

        Self {
            builtins,
            custom: file.custom,
            active_id,
            debate: file.debate,
            env_debate,
            path,
        }
    }

    /// Built-in profiles seeded from environment configuration. These are the
    /// only place default model names live — nothing else in the codebase
    /// should embed model-name strings.
    fn builtin_profiles(config: &AppConfig) -> Vec<ModelProfile> {
        vec![
            ModelProfile {
                id: "builtin-deepseek".into(),
                display_name: "DeepSeek".into(),
                kind: ModelKind::DeepSeek,
                base_url: config.deepseek_base_url.clone(),
                model_name: config
                    .deepseek_model_name
                    .clone()
                    .unwrap_or_else(|| "deepseek-chat".into()),
                pro_model_name: Some(
                    config
                        .deepseek_model_name
                        .clone()
                        .unwrap_or_else(|| "deepseek-reasoner".into()),
                ),
                api_key: None,
                api_key_env: Some("DEEPSEEK_API_KEY".into()),
                builtin: true,
            },
            ModelProfile {
                id: "builtin-stepfun".into(),
                display_name: "StepFun".into(),
                kind: ModelKind::StepFun,
                base_url: config.stepfun_base_url.clone(),
                model_name: config
                    .stepfun_model_name
                    .clone()
                    .unwrap_or_else(|| "step-3.7-flash".into()),
                pro_model_name: None,
                api_key: None,
                api_key_env: Some("STEPFUN_API_KEY".into()),
                builtin: true,
            },
            ModelProfile {
                id: "builtin-minimax".into(),
                display_name: "MiniMax".into(),
                kind: ModelKind::MiniMax,
                base_url: config.minimax_base_url.clone(),
                model_name: config
                    .minimax_model_name
                    .clone()
                    .unwrap_or_else(|| "MiniMax-M3".into()),
                pro_model_name: None,
                api_key: None,
                api_key_env: Some("MINIMAX_API_KEY".into()),
                builtin: true,
            },
        ]
    }

    pub fn list(&self) -> Vec<&ModelProfile> {
        self.builtins.iter().chain(self.custom.iter()).collect()
    }

    pub fn get(&self, id: &str) -> Option<&ModelProfile> {
        self.list().into_iter().find(|p| p.id == id)
    }

    pub fn active(&self) -> &ModelProfile {
        self.get(&self.active_id)
            .unwrap_or(&self.builtins[0])
    }

    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    /// Add a custom profile. Returns its generated id.
    pub fn add(&mut self, mut profile: ModelProfile) -> String {
        let id = format!("custom-{}", uuid::Uuid::new_v4().simple().to_string()[..8].to_string());
        profile.id = id.clone();
        profile.builtin = false;
        self.custom.push(profile);
        let _ = self.save();
        id
    }

    /// Update an existing custom profile's mutable fields (name/url/model/key).
    pub fn update(&mut self, id: &str, patch: ModelProfile) -> Result<(), String> {
        let prof = self
            .custom
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| "只能修改自定义模型配置".to_string())?;
        prof.display_name = patch.display_name;
        prof.base_url = patch.base_url;
        prof.model_name = patch.model_name;
        prof.pro_model_name = patch.pro_model_name;
        if patch.api_key.as_deref().map_or(false, |k| !k.is_empty()) {
            prof.api_key = patch.api_key;
        }
        let _ = self.save();
        Ok(())
    }

    /// Remove a custom profile. Fails for built-ins or the active profile.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if self.builtins.iter().any(|p| p.id == id) {
            return Err("内置模型配置不可删除".into());
        }
        if self.active_id == id {
            return Err("该模型正在使用中，请先切换到其他模型".into());
        }
        self.custom.retain(|p| p.id != id);
        let _ = self.save();
        Ok(())
    }

    /// Switch the active profile and persist the selection.
    pub fn set_active(&mut self, id: &str) -> Result<(), String> {
        if self.get(id).is_none() {
            return Err(format!("未找到模型配置: {id}"));
        }
        self.active_id = id.to_string();
        let _ = self.save();
        Ok(())
    }

    /// Effective debate role selection: UI-persisted choice > env default.
    pub fn debate_selection(&self) -> DebateRoleSelection {
        DebateRoleSelection {
            proposer: self
                .debate
                .proposer
                .clone()
                .or_else(|| self.env_debate.proposer.clone()),
            opponent: self
                .debate
                .opponent
                .clone()
                .or_else(|| self.env_debate.opponent.clone()),
            judge: self
                .debate
                .judge
                .clone()
                .or_else(|| self.env_debate.judge.clone()),
        }
    }

    /// Resolve the profile serving a debate role: explicit selection (UI >
    /// env), else the active main model. Unresolvable selections (deleted
    /// profile) fall back to the main model rather than erroring.
    pub fn role_profile(&self, role: DebateRole) -> &ModelProfile {
        if let Some(id) = self.debate_selection().get(role)
            && let Some(p) = self.get(id)
        {
            return p;
        }
        self.active()
    }

    /// Persist a per-role selection (from the server ⚙️ settings). `None`
    /// clears the role back to the main model. Fails if the profile id is
    /// unknown.
    pub fn set_debate_role(&mut self, role: DebateRole, id: Option<String>) -> Result<(), String> {
        if let Some(ref id) = id && self.get(id).is_none() {
            return Err(format!("未找到模型配置: {id}"));
        }
        self.debate.set(role, id);
        let _ = self.save();
        Ok(())
    }

    /// Persist custom profiles + active selection to `models.json`.
    fn save(&self) -> std::io::Result<()> {
        let file = RegistryFile {
            active_id: Some(self.active_id.clone()),
            custom: self.custom.clone(),
            debate: self.debate.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing() {
        assert_eq!(ModelKind::from_str_loose("openai_compatible"), Some(ModelKind::OpenAiCompatible));
        assert_eq!(ModelKind::from_str_loose("Anthropic"), Some(ModelKind::AnthropicCompatible));
        assert_eq!(ModelKind::from_str_loose("nope"), None);
    }

    #[test]
    fn registry_active_fallback() {
        let config = AppConfig::load();
        let reg = ModelRegistry::load(&config);
        // active profile must resolve to something with a sane kind
        let active = reg.active();
        assert!(!active.id.is_empty());
        assert!(!active.model_name.is_empty());
    }

    #[test]
    fn model_kind_deserializes_both_spellings() {
        // serde snake_case renders OpenAiCompatible as "open_ai_compatible";
        // hand-written models.json files commonly use "openai_compatible".
        // Both must deserialize (alias), or the whole registry file silently
        // fails to parse and the default provider takes over.
        for spelling in ["open_ai_compatible", "openai_compatible"] {
            let v: ModelKind =
                serde_json::from_str(&format!("\"{spelling}\"")).expect("deserializes");
            assert_eq!(v, ModelKind::OpenAiCompatible);
        }
        let v: ModelKind =
            serde_json::from_str("\"anthropic_compatible\"").expect("deserializes");
        assert_eq!(v, ModelKind::AnthropicCompatible);
    }
}
