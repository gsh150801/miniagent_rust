/// Ebbinghaus forgetting curve model for memory decay.
/// Inspired by Engram-RS and cognitive science.

#[derive(Default)]
pub struct MemoryDecay;

impl MemoryDecay {
    /// Calculate current memory strength after elapsed time.
    /// Uses exponential decay: strength = floor + (1-floor) * exp(-decay_rate * days)
    pub fn calculate(
        initial_strength: f64,
        decay_rate: f64,
        retention_floor: f64,
        days_since_access: f64,
    ) -> f64 {
        let retention = retention_floor.clamp(0.001, 0.5);
        let lost = (initial_strength - retention).max(0.0);
        retention + lost * (-decay_rate * days_since_access).exp()
    }

    /// Activation boost when a memory is recalled (use-it-or-lose-it).
    pub fn activation_boost(strength: f64, boost: f64, max_strength: f64) -> f64 {
        (strength + boost).min(max_strength)
    }

    /// Default decay rates by memory type.
    pub fn default_rate(memory_type: &str) -> f64 {
        match memory_type {
            "factual" => 0.01,    // facts decay slowly
            "method" => 0.005,    // methods last longer
            "hypothesis" => 0.02, // hypotheses need frequent reinforcement
            "episodic" => 0.015,  // medium decay
            _ => 0.01,
        }
    }

    /// Default retention floor by memory type.
    pub fn default_floor(memory_type: &str) -> f64 {
        match memory_type {
            "factual" => 0.02,
            "method" => 0.03,    // methods shouldn't be fully forgotten
            "hypothesis" => 0.01, // hypotheses can be discarded
            "episodic" => 0.015,
            _ => 0.01,
        }
    }
}
