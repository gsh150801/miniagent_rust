use crate::secrets::ApiKey;

/// Unified application configuration loaded from `.env` and environment
/// variables.
///
/// Call [`AppConfig::load`] once at program startup (typically in `main`).
/// This calls `dotenvy::dotenv()` internally, so all vars in `.env` are
/// available via `std::env::var` for crates that read them directly (e.g.
/// the `tool` crate's search-backend keys).
#[derive(Debug, Clone)]
pub struct AppConfig {
    // ── Provider selection ───────────────────────────────────────
    /// Which LLM provider to use: "deepseek" (default), "stepfun", or "minimax".
    /// Set via PROVIDER env var. Allows switching providers without code changes.
    pub provider: String,

    // ── DeepSeek ──────────────────────────────────────────────────
    pub deepseek_api_key: Option<ApiKey>,
    pub deepseek_base_url: String,
    /// Overrides the model name for both Flash and Pro tiers when set.
    pub deepseek_model_name: Option<String>,

    // ── StepFun ───────────────────────────────────────────────────
    pub stepfun_api_key: Option<ApiKey>,
    pub stepfun_base_url: String,
    pub stepfun_model_name: Option<String>,

    // ── MiniMax (Token-Plan subscription, OpenAI-compatible API) ──
    pub minimax_api_key: Option<ApiKey>,
    pub minimax_base_url: String,
    pub minimax_model_name: Option<String>,

    // ── 辩论角色模型 ─────────────────────────────────────────────
    /// Per-role model profile IDs for the debate (Proposer / Opponent /
    /// Judge). Empty = the active main model. Values reference profiles in
    /// `models.json` (e.g. "builtin-deepseek", "custom-ab12cd34").
    pub debate_proposer_model: Option<String>,
    pub debate_opponent_model: Option<String>,
    pub debate_judge_model: Option<String>,

    // ── Search backends ───────────────────────────────────────────
    pub bocha_api_key: Option<ApiKey>,
    pub tavily_api_key: Option<ApiKey>,
    pub serpapi_api_key: Option<ApiKey>,
    pub serper_api_key: Option<ApiKey>,
    pub langsearch_api_key: Option<ApiKey>,
    pub anysearch_api_key: Option<ApiKey>,

    // ── Academic APIs ─────────────────────────────────────────────
    pub pubmed_api_key: Option<ApiKey>,

    // ── Agent limits ──────────────────────────────────────────────
    pub max_iterations: usize,
    pub max_tokens: u32,
    /// Max estimated tokens before history trimming kicks in.
    pub agent_history_token_limit: usize,
    /// Number of recent messages kept verbatim during summarization.
    pub agent_keep_recent_msgs: usize,
    /// Max consecutive all-error tool rounds before breaking the agent loop.
    pub agent_max_consecutive_errors: usize,

    // ── Loop pipeline ─────────────────────────────────────────────
    pub loop_max_loops: usize,
    /// Consecutive stagnant loops before forced termination.
    pub loop_no_progress_limit: usize,
    /// Per-loop token cost threshold for early-stop (cost-control).
    pub loop_cost_token_threshold: usize,
    /// Minimum progress % required when cost exceeds threshold.
    pub loop_cost_min_progress: f64,
    /// Max tool iterations for the Explore stage.
    pub loop_explore_max_iterations: usize,
    /// Max tool iterations for dispatched tasks.
    pub loop_dispatch_max_iterations: usize,
    /// Max concurrently dispatched tasks within a single wave. Caps the
    /// `FuturesUnordered` width in `dispatch.rs` so a 50-task wave does
    /// not flood the LLM provider and OOM the Tokio runtime.
    pub loop_dispatch_wave_concurrency: usize,
    /// Max output tokens for stage LLM calls.
    pub loop_plan_max_tokens: u32,
    pub loop_explore_max_tokens: u32,
    pub loop_evaluate_max_tokens: u32,
    pub loop_repair_max_tokens: u32,
    pub loop_critic_max_tokens: u32,
    pub loop_judge_max_tokens: u32,

    // ── Budget ────────────────────────────────────────────────────
    pub token_budget: usize,

    // ── Server ────────────────────────────────────────────────────
    pub server_host: String,
    pub server_port: u16,
    /// Wall-clock budget for one server-driven research run. The full
    /// goals-1..4 pipeline (literature → KG → hypotheses → debate →
    /// validation plans → GEO downloads → notebook executions) routinely
    /// exceeds an hour — the old fixed 60-min cap aborted live runs mid
    /// analysis. Default 3 h, override via `RESEARCH_TIMEOUT_SECS`.
    pub research_timeout_secs: u64,
}

impl AppConfig {
    /// Load configuration from `.env` file and environment variables.
    ///
    /// Calls `dotenvy::dotenv()` first, then reads all known variables.
    /// Missing optional keys are `None`; required numeric settings fall back
    /// to documented defaults.
    pub fn load() -> Self {
        let _ = dotenvy::dotenv();

        Self {
            // ── Provider selection ──
            provider: Self::var("PROVIDER")
                .unwrap_or_else(|| "deepseek".into())
                .to_lowercase(),

            deepseek_api_key: ApiKey::from_env("DEEPSEEK_API_KEY"),
            deepseek_base_url: Self::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|| "https://api.deepseek.com".into()),
            deepseek_model_name: Self::var("DEEPSEEK_MODEL_NAME"),

            stepfun_api_key: ApiKey::from_env("STEPFUN_API_KEY"),
            stepfun_base_url: Self::var("STEPFUN_BASE_URL")
                .unwrap_or_else(|| "https://api.stepfun.com/step_plan/v1".into()),
            stepfun_model_name: Self::var("STEPFUN_MODEL_NAME"),

            minimax_api_key: ApiKey::from_env("MINIMAX_API_KEY"),
            minimax_base_url: Self::var("MINIMAX_BASE_URL")
                .unwrap_or_else(|| "https://api.minimaxi.com/v1".into()),
            minimax_model_name: Self::var("MINIMAX_MODEL_NAME"),

            debate_proposer_model: Self::var("DEBATE_PROPOSER_MODEL"),
            debate_opponent_model: Self::var("DEBATE_OPPONENT_MODEL"),
            debate_judge_model: Self::var("DEBATE_JUDGE_MODEL"),

            bocha_api_key: ApiKey::from_env("BOCHA_API_KEY"),
            tavily_api_key: ApiKey::from_env("TAVILY_API_KEY"),
            serpapi_api_key: ApiKey::from_env("SERPAPI_API_KEY"),
            serper_api_key: ApiKey::from_env("SERPER_API_KEY"),
            langsearch_api_key: ApiKey::from_env("LANGSEARCH_API_KEY"),
            anysearch_api_key: ApiKey::from_env("ANYSEARCH_API_KEY"),

            pubmed_api_key: ApiKey::from_env("PUBMED_API_KEY"),

            // ── Agent limits ──
            max_iterations: Self::parsed("MAX_ITERATIONS", 35),
            max_tokens: Self::parsed("MAX_TOKENS", 393_216),
            agent_history_token_limit: Self::parsed("AGENT_HISTORY_TOKEN_LIMIT", 96_000),
            agent_keep_recent_msgs: Self::parsed("AGENT_KEEP_RECENT_MSGS", 5),
            agent_max_consecutive_errors: Self::parsed("AGENT_MAX_CONSECUTIVE_ERRORS", 3),

            // ── Loop pipeline ──
            loop_max_loops: Self::parsed("LOOP_MAX_LOOPS", 10),
            loop_no_progress_limit: Self::parsed("LOOP_NO_PROGRESS_LIMIT", 3),
            loop_cost_token_threshold: Self::parsed("LOOP_COST_TOKEN_THRESHOLD", 30_000),
            loop_cost_min_progress: Self::parsed("LOOP_COST_MIN_PROGRESS", 10.0),
            loop_explore_max_iterations: Self::parsed("LOOP_EXPLORE_MAX_ITERATIONS", 10),
            loop_dispatch_max_iterations: Self::parsed("LOOP_DISPATCH_MAX_ITERATIONS", 15),
            loop_dispatch_wave_concurrency: Self::parsed("LOOP_DISPATCH_WAVE_CONCURRENCY", 4),
            loop_plan_max_tokens: Self::parsed("LOOP_PLAN_MAX_TOKENS", 4000),
            loop_explore_max_tokens: Self::parsed("LOOP_EXPLORE_MAX_TOKENS", 4000),
            loop_evaluate_max_tokens: Self::parsed("LOOP_EVALUATE_MAX_TOKENS", 2000),
            loop_repair_max_tokens: Self::parsed("LOOP_REPAIR_MAX_TOKENS", 1500),
            loop_critic_max_tokens: Self::parsed("LOOP_CRITIC_MAX_TOKENS", 2000),
            loop_judge_max_tokens: Self::parsed("LOOP_JUDGE_MAX_TOKENS", 1000),

            // ── Budget ──
            token_budget: Self::parsed("TOKEN_BUDGET", 3_000_000),

            // ── Server ──
            server_host: Self::var("SERVER_HOST").unwrap_or_else(|| "0.0.0.0".into()),
            server_port: Self::parsed("SERVER_PORT", 3002),
            research_timeout_secs: Self::parsed("RESEARCH_TIMEOUT_SECS", 3 * 3600),
        }
    }

    /// Require the DeepSeek API key, returning a descriptive error if unset.
    ///
    /// Most entry points (CLI commands, server startup) need this key.
    /// Use this helper to fail fast with a clear message.
    pub fn require_deepseek_key(&self) -> Result<&ApiKey, String> {
        self.deepseek_api_key
            .as_ref()
            .ok_or_else(|| "DEEPSEEK_API_KEY not set. Add it to .env or export it.".to_string())
    }

    /// Require the StepFun API key, returning a descriptive error if unset.
    pub fn require_stepfun_key(&self) -> Result<&ApiKey, String> {
        self.stepfun_api_key
            .as_ref()
            .ok_or_else(|| "STEPFUN_API_KEY not set. Add it to .env or export it.".to_string())
    }

    /// Returns true if the active provider is StepFun.
    pub fn is_stepfun(&self) -> bool {
        self.provider == "stepfun"
    }

    /// Require the MiniMax subscription API key, returning a descriptive error if unset.
    pub fn require_minimax_key(&self) -> Result<&ApiKey, String> {
        self.minimax_api_key
            .as_ref()
            .ok_or_else(|| "MINIMAX_API_KEY not set. Add it to .env or export it.".to_string())
    }

    /// Returns true if the active provider is MiniMax.
    pub fn is_minimax(&self) -> bool {
        self.provider == "minimax"
    }

    /// Require the API key for whichever provider is currently active.
    pub fn require_active_key(&self) -> Result<&ApiKey, String> {
        if self.is_stepfun() {
            self.require_stepfun_key()
        } else if self.is_minimax() {
            self.require_minimax_key()
        } else {
            self.require_deepseek_key()
        }
    }

    // ── Internal helpers ──────────────────────────────────────────

    fn var(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }

    fn parsed<T: std::str::FromStr>(name: &str, default: T) -> T {
        Self::var(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::load()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_loads_without_crash() {
        // Should not panic even if .env is missing
        let config = AppConfig::load();
        // deepseek_api_key may or may not be set depending on test env
        let _ = config.deepseek_api_key.is_some();
    }

    #[test]
    fn test_config_has_defaults() {
        // Test the parsing helpers directly, independent of .env file contents
        // (AppConfig::load reads .env which may override these defaults)
        assert_eq!(AppConfig::parsed("NONEXISTENT_VAR_12345", 35usize), 35);
        assert_eq!(AppConfig::parsed("NONEXISTENT_VAR_12345", 393_216u32), 393_216);
        assert_eq!(AppConfig::parsed("NONEXISTENT_VAR_12345", 10usize), 10);
        assert_eq!(AppConfig::parsed("NONEXISTENT_VAR_12345", 3002u16), 3002);
    }

    #[test]
    fn test_require_deepseek_key_returns_value_or_error() {
        // This test is environment-dependent: if .env has DEEPSEEK_API_KEY,
        // require_deepseek_key returns Ok; otherwise Err.
        let config = AppConfig::load();
        let result = config.require_deepseek_key();
        // Either way, the function should not panic
        match &result {
            Ok(key) => assert!(!key.as_str().is_empty(), "key should be non-empty"),
            Err(msg) => assert!(msg.contains("DEEPSEEK_API_KEY"), "error should mention the var name"),
        }
    }
}
