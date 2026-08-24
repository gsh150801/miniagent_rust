use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct InferenceConfig {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub thinking_budget: Option<u32>,     // DeepSeek Pro extended thinking
    pub enable_thinking: bool,
    pub stop_sequences: Vec<String>,
}


impl InferenceConfig {
    pub fn flash() -> Self {
        Self {
            temperature: Some(0.7),
            max_tokens: Some(4096),
            enable_thinking: false,
            ..Default::default()
        }
    }

    pub fn pro() -> Self {
        Self {
            temperature: None, // reasoning models need temp=1.0
            max_tokens: Some(16_000),
            enable_thinking: true,
            thinking_budget: Some(8_000),
            ..Default::default()
        }
    }

    pub fn pro_deep() -> Self {
        Self {
            temperature: None,
            max_tokens: None, // let API use its default (32K for reasoner)
            enable_thinking: true,
            thinking_budget: Some(16_000),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
    DeepResearch,
}
