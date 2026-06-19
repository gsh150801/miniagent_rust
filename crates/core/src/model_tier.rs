//! 模型等级（单一来源，跨 crate 共享）。
//!
//! 历史上 `ModelTier` 在 `miniagent-planning` 的 `state_graph` 和 `agent_profile`
//! 两个模块各定义了一份，是同名不同路径的两个独立类型，无法互相赋值。现统一到
//! `miniagent-core`，所有 crate 通过 `miniagent_core::ModelTier` 引用同一类型。

/// LLM 模型分级：Flash 用于简单/高频任务，Pro 用于深度推理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelTier {
    Flash,
    Pro,
}

#[cfg(test)]
mod tests {
    use super::ModelTier;

    #[test]
    fn modeltier_roundtrips_serde() {
        let json = serde_json::to_string(&ModelTier::Pro).unwrap();
        assert_eq!(json, "\"Pro\"");
        let back: ModelTier = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ModelTier::Pro);
    }

    #[test]
    fn modeltier_is_copy_and_eq() {
        let a = ModelTier::Flash;
        let b = a; // Copy
        assert_eq!(a, b);
        assert_ne!(a, ModelTier::Pro);
    }
}
