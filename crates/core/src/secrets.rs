use std::sync::Arc;

/// A secret API key whose value is never exposed in [`Debug`] or [`Display`]
/// output, preventing accidental leakage in logs, error messages, or panic
/// backtraces.
///
/// Cloning is cheap (Arc bump). The only way to access the raw key string is
/// [`ApiKey::as_str`].
#[derive(Clone)]
pub struct ApiKey(Arc<str>);

impl ApiKey {
    /// Create a new `ApiKey` from any string-like value.
    pub fn new(key: impl Into<String>) -> Self {
        let s: String = key.into();
        Self(Arc::from(s))
    }

    /// Create from an env-var name: reads the var, returns `None` if unset or
    /// empty.
    pub fn from_env(var_name: &str) -> Option<Self> {
        std::env::var(var_name)
            .ok()
            .filter(|v| !v.is_empty())
            .map(Self::new)
    }

    /// Access the raw key string. This is the **only** way to read the value.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return a masked representation suitable for logging.
    /// Shows the first 4 and last 4 characters with `****` in between.
    /// Short keys (< 8 chars) are fully masked as `****`.
    pub fn masked(&self) -> String {
        let s = &self.0;
        if s.len() <= 8 {
            "****".to_string()
        } else {
            let head: String = s.chars().take(4).collect();
            let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            format!("{head}****{tail}")
        }
    }

    /// Returns `true` if the key is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ── Manual trait impls that never reveal the secret ──────────────

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiKey({})", self.masked())
    }
}

impl std::fmt::Display for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiKey({})", self.masked())
    }
}

impl PartialEq for ApiKey {
    fn eq(&self, other: &Self) -> bool {
        // Constant-time comparison would be ideal, but for non-crypto use this is fine.
        self.0 == other.0
    }
}

impl Eq for ApiKey {}

impl std::hash::Hash for ApiKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_masks_value() {
        let key = ApiKey::new("sk-abcd1234efgh5678");
        let dbg = format!("{:?}", key);
        assert!(!dbg.contains("sk-abcd1234efgh5678"));
        assert!(dbg.contains("****"));
        assert!(dbg.contains("ApiKey("));
    }

    #[test]
    fn test_display_masks_value() {
        let key = ApiKey::new("sk-abcd1234efgh5678");
        let dsp = format!("{key}");
        assert!(!dsp.contains("sk-abcd1234efgh5678"));
        assert!(dsp.contains("****"));
    }

    #[test]
    fn test_as_str_returns_value() {
        let key = ApiKey::new("sk-secret");
        assert_eq!(key.as_str(), "sk-secret");
    }

    #[test]
    fn test_masked_shows_head_and_tail() {
        let key = ApiKey::new("sk-abcd1234efgh5678");
        let masked = key.masked();
        assert!(masked.starts_with("sk-a"));
        assert!(masked.ends_with("5678"));
        assert!(masked.contains("****"));
    }

    #[test]
    fn test_short_key_fully_masked() {
        let key = ApiKey::new("abc");
        assert_eq!(key.masked(), "****");
    }

    #[test]
    fn test_clone_is_cheap() {
        let key = ApiKey::new("sk-test");
        let cloned = key.clone();
        assert_eq!(key, cloned);
        assert_eq!(cloned.as_str(), "sk-test");
    }

    #[test]
    fn test_eq_and_hash() {
        let a = ApiKey::new("sk-same");
        let b = ApiKey::new("sk-same");
        let c = ApiKey::new("sk-different");
        assert_eq!(a, b);
        assert_ne!(a, c);

        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(a.clone(), 1);
        assert_eq!(map.get(&b), Some(&1));
    }
}
