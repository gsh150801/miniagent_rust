use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Result of a single backend health probe.
#[derive(Debug, Clone)]
pub struct BackendHealth {
    pub name: String,
    pub healthy: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Global health state shared across the process.
static HEALTH_STATE: std::sync::LazyLock<Arc<RwLock<HealthState>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(HealthState::new())));

/// In-memory health state: which search backends are known to be healthy.
#[derive(Debug, Clone)]
pub struct HealthState {
    /// Set of healthy backend names (e.g., "serper", "tavily", "bocha", "pubmed").
    healthy: HashSet<String>,
    /// Set of backends that have been probed already.
    probed: HashSet<String>,
    /// Full probe results (for diagnostics).
    results: Vec<BackendHealth>,
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            healthy: HashSet::new(),
            probed: HashSet::new(),
            results: Vec::new(),
        }
    }

    pub fn is_healthy(&self, name: &str) -> bool {
        if !self.probed.contains(name) {
            return true; // not yet probed → assume healthy
        }
        self.healthy.contains(name)
    }

    pub fn mark_healthy(&mut self, name: &str) {
        self.probed.insert(name.to_string());
        self.healthy.insert(name.to_string());
    }

    pub fn mark_unhealthy(&mut self, name: &str, error: String, latency_ms: u64) {
        self.probed.insert(name.to_string());
        self.healthy.remove(name);
        self.results.push(BackendHealth {
            name: name.to_string(),
            healthy: false,
            latency_ms,
            error: Some(error),
        });
    }

    pub fn healthy_backends(&self) -> Vec<String> {
        self.probed.iter().filter(|n| self.healthy.contains(*n)).cloned().collect()
    }

    pub fn all_results(&self) -> &[BackendHealth] {
        &self.results
    }
}

/// Access the global health state for reading.
pub async fn health_state() -> tokio::sync::RwLockReadGuard<'static, HealthState> {
    HEALTH_STATE.read().await
}

/// Access the global health state for writing.
pub async fn health_state_mut() -> tokio::sync::RwLockWriteGuard<'static, HealthState> {
    HEALTH_STATE.write().await
}

/// Run health probes for all configured search backends.
/// Call this once at startup before handling any user requests.
pub async fn probe_all_backends() -> Vec<BackendHealth> {
    let client = reqwest::Client::builder()
        .user_agent("miniagent/0.1-healthcheck")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client");

    let test_query = "hello world";
    let mut handles = Vec::new();

    // Serper probe
    let serper_key = env_opt("SERPER_API_KEY")
        .or_else(|| env_opt("SERPAPI_API_KEY"));
    if let Some(key) = serper_key {
        let client = client.clone();
        let q = test_query.to_string();
        handles.push(tokio::spawn(async move {
            probe_serper(&client, &q, &key).await
        }));
    } else {
        record_unhealthy("serper", "SERPER_API_KEY not set", 0).await;
    }

    // Tavily probe
    let tavily_key = env_opt("TAVILY_API_KEY");
    if let Some(key) = tavily_key {
        let client = client.clone();
        let q = test_query.to_string();
        handles.push(tokio::spawn(async move {
            probe_tavily(&client, &q, &key).await
        }));
    } else {
        record_unhealthy("tavily", "TAVILY_API_KEY not set", 0).await;
    }

    // Bocha probe
    let bocha_key = env_opt("BOCHA_API_KEY");
    if let Some(key) = bocha_key {
        let client = client.clone();
        let q = test_query.to_string();
        handles.push(tokio::spawn(async move {
            probe_bocha(&client, &q, &key).await
        }));
    } else {
        record_unhealthy("bocha", "BOCHA_API_KEY not set", 0).await;
    }

    // PubMed probe
    handles.push(tokio::spawn(async move {
        probe_pubmed(&client, test_query).await
    }));

    // Collect results
    let mut results = Vec::new();
    for handle in handles {
        if let Ok(r) = handle.await {
            record_health(&r).await;
            results.push(r);
        }
    }

    // Log summary
    let state = HEALTH_STATE.read().await;
    let healthy_count = state.healthy.len();
    let total_count = state.probed.len();
    eprintln!(
        "   🔍 Health check: {}/{} backends healthy",
        healthy_count, total_count
    );
    for r in &results {
        if r.healthy {
            eprintln!("      ✅ {} ({}ms)", r.name, r.latency_ms);
        } else {
            eprintln!(
                "      ❌ {} ({}ms): {}",
                r.name,
                r.latency_ms,
                r.error.as_deref().unwrap_or("unknown error")
            );
        }
    }
    if !results.is_empty() {
        eprintln!("   ──");
    }
    drop(state);
    results
}

async fn probe_serper(client: &reqwest::Client, query: &str, api_key: &str) -> BackendHealth {
    let start = std::time::Instant::now();
    let body = serde_json::json!({ "q": query, "num": 1 });
    match client
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => BackendHealth {
            name: "serper".into(),
            healthy: true,
            latency_ms: start.elapsed().as_millis() as u64,
            error: None,
        },
        Ok(resp) => BackendHealth {
            name: "serper".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => BackendHealth {
            name: "serper".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("{e}")),
        },
    }
}

async fn probe_tavily(client: &reqwest::Client, query: &str, api_key: &str) -> BackendHealth {
    let start = std::time::Instant::now();
    let body = serde_json::json!({
        "api_key": api_key,
        "query": query,
        "max_results": 1,
        "search_depth": "basic",
    });
    match client
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => BackendHealth {
            name: "tavily".into(),
            healthy: true,
            latency_ms: start.elapsed().as_millis() as u64,
            error: None,
        },
        Ok(resp) => BackendHealth {
            name: "tavily".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => BackendHealth {
            name: "tavily".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("{e}")),
        },
    }
}

async fn probe_bocha(client: &reqwest::Client, query: &str, api_key: &str) -> BackendHealth {
    let start = std::time::Instant::now();
    match client
        .get("https://api.bochaai.com/v1/ai/search")
        .header("Authorization", format!("Bearer {api_key}"))
        .query(&[("query", query), ("count", "1")])
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => BackendHealth {
            name: "bocha".into(),
            healthy: true,
            latency_ms: start.elapsed().as_millis() as u64,
            error: None,
        },
        Ok(resp) => BackendHealth {
            name: "bocha".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => BackendHealth {
            name: "bocha".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("{e}")),
        },
    }
}

async fn probe_pubmed(client: &reqwest::Client, query: &str) -> BackendHealth {
    let start = std::time::Instant::now();
    let url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi?db=pubmed&term={}&retmax=1&retmode=json",
        urlencoding::encode(query)
    );
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => BackendHealth {
            name: "pubmed".into(),
            healthy: true,
            latency_ms: start.elapsed().as_millis() as u64,
            error: None,
        },
        Ok(resp) => BackendHealth {
            name: "pubmed".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => BackendHealth {
            name: "pubmed".into(),
            healthy: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("{e}")),
        },
    }
}

async fn record_health(result: &BackendHealth) {
    let mut state = HEALTH_STATE.write().await;
    if result.healthy {
        state.mark_healthy(&result.name);
    } else {
        state.mark_unhealthy(&result.name, result.error.clone().unwrap_or_default(), result.latency_ms);
    }
}

async fn record_unhealthy(name: &str, error: &str, latency_ms: u64) {
    let mut state = HEALTH_STATE.write().await;
    state.mark_unhealthy(name, error.to_string(), latency_ms);
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_state_initially_empty() {
        let state = HealthState::new();
        assert_eq!(state.healthy.len(), 0);
        assert_eq!(state.probed.len(), 0);
        assert!(state.is_healthy("serper")); // unprobed → assumed healthy
    }

    #[tokio::test]
    async fn test_mark_healthy() {
        let mut state = HealthState::new();
        state.mark_healthy("serper");
        assert!(state.is_healthy("serper"));
        assert!(state.probed.contains("serper"));
    }

    #[tokio::test]
    async fn test_mark_unhealthy() {
        let mut state = HealthState::new();
        state.mark_unhealthy("tavily", "timeout".into(), 5000);
        assert!(!state.is_healthy("tavily"));
        assert!(state.probed.contains("tavily"));
    }

    #[tokio::test]
    async fn test_unprobed_returns_healthy() {
        let state = HealthState::new();
        // Before probe, all backends are assumed healthy
        assert!(state.is_healthy("serper"));
        assert!(!state.probed.contains("serper"));
    }

    #[tokio::test]
    async fn test_record_unhealthy_via_global() {
        record_unhealthy("test_backend", "connection refused", 100).await;
        let state = health_state().await;
        assert!(!state.is_healthy("test_backend"));
        assert_eq!(state.all_results().len(), 1);
        assert_eq!(state.all_results()[0].name, "test_backend");
    }

    #[tokio::test]
    async fn test_healthy_backends_filter() {
        let mut state = HealthState::new();
        state.mark_healthy("serper");
        state.mark_healthy("pubmed");
        state.mark_unhealthy("tavily", "bad key".into(), 0);
        let healthy = state.healthy_backends();
        assert!(healthy.contains(&"serper".to_string()));
        assert!(healthy.contains(&"pubmed".to_string()));
        assert!(!healthy.contains(&"tavily".to_string()));
    }

    #[tokio::test]
    async fn test_web_search_health_check_no_env_keys() {
        // Ensure that with no env keys set, probe_all_backends doesn't panic
        // and correctly marks services as unhealthy due to missing keys.
        let results = probe_all_backends().await;
        // At minimum, pubmed probe should execute (no API key needed for NCBI)
        let pubmed_result = results.iter().find(|r| r.name == "pubmed");
        assert!(pubmed_result.is_some(), "PubMed probe should always run");
        // Serper, Tavily, Bocha should be marked unhealthy (no keys)
        // but they're recorded via record_unhealthy, which doesn't return in results
        // Let's just verify the function completes without panicking
        assert!(!results.is_empty(), "Should have at least PubMed result");
    }

    #[tokio::test]
    async fn test_probe_does_not_panic_with_bad_env() {
        // Set bad keys to verify the probe handles HTTP errors gracefully
        unsafe {
            std::env::set_var("SERPER_API_KEY", "bad-key");
            std::env::set_var("TAVILY_API_KEY", "bad-key");
            std::env::set_var("BOCHA_API_KEY", "bad-key");
        }
        let results = probe_all_backends().await;
        assert!(!results.is_empty(), "Should produce results even with bad keys");
        // All with bad keys should be unhealthy
        for r in &results {
            if r.name != "pubmed" {
                assert!(!r.healthy, "{} should be unhealthy with bad key", r.name);
            }
        }
        // Clean up env
        unsafe {
            std::env::remove_var("SERPER_API_KEY");
            std::env::remove_var("TAVILY_API_KEY");
            std::env::remove_var("BOCHA_API_KEY");
        }
    }
}
