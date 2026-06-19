use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Tracks tool reliability over time.
/// Maintains sliding window stats: success rate, latency, common failure modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReliabilityTracker {
    tools: HashMap<String, ToolReliability>,
    window_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReliability {
    pub tool_name: String,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub call_count: u64,
    pub failure_count: u64,
    pub common_failure_modes: Vec<String>,
    recent_outcomes: Vec<bool>,
}

impl ToolReliabilityTracker {
    pub fn new(window_size: usize) -> Self {
        Self {
            tools: HashMap::new(),
            window_size,
        }
    }

    pub fn record_success(&mut self, tool_name: &str, latency_ms: u64) {
        let entry = self.tools.entry(tool_name.to_string()).or_insert_with(|| {
            ToolReliability {
                tool_name: tool_name.to_string(),
                success_rate: 1.0,
                avg_latency_ms: latency_ms as f64,
                call_count: 0,
                failure_count: 0,
                common_failure_modes: Vec::new(),
                recent_outcomes: Vec::new(),
            }
        });

        entry.call_count += 1;
        entry.recent_outcomes.push(true);
        if entry.recent_outcomes.len() > self.window_size {
            entry.recent_outcomes.remove(0);
        }
        entry.success_rate = entry.recent_outcomes.iter().filter(|&&o| o).count() as f64
            / entry.recent_outcomes.len() as f64;
        entry.avg_latency_ms = (entry.avg_latency_ms * (entry.call_count - 1) as f64 + latency_ms as f64)
            / entry.call_count as f64;
    }

    pub fn record_failure(&mut self, tool_name: &str, error: &str) {
        let entry = self.tools.entry(tool_name.to_string()).or_insert_with(|| {
            ToolReliability {
                tool_name: tool_name.to_string(),
                success_rate: 1.0,
                avg_latency_ms: 0.0,
                call_count: 0,
                failure_count: 0,
                common_failure_modes: Vec::new(),
                recent_outcomes: Vec::new(),
            }
        });

        entry.call_count += 1;
        entry.failure_count += 1;
        entry.recent_outcomes.push(false);
        if entry.recent_outcomes.len() > self.window_size {
            entry.recent_outcomes.remove(0);
        }
        entry.success_rate = entry.recent_outcomes.iter().filter(|&&o| o).count() as f64
            / entry.recent_outcomes.len() as f64;

        if !entry.common_failure_modes.contains(&error.to_string()) {
            entry.common_failure_modes.push(error.to_string());
            entry.common_failure_modes.truncate(10);
        }
    }

    pub fn get(&self, tool_name: &str) -> Option<&ToolReliability> {
        self.tools.get(tool_name)
    }

    pub fn recommend_avoid(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|(_, r)| r.call_count >= 10 && r.success_rate < 0.5)
            .map(|(n, _)| n.clone())
            .collect()
    }

    pub fn all(&self) -> Vec<&ToolReliability> {
        self.tools.values().collect()
    }
}

impl Default for ToolReliabilityTracker {
    fn default() -> Self {
        Self::new(50)
    }
}
