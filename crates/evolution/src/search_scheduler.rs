use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Search Strategy ────────────────────────────────────────────

/// The current search strategy selected by the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchStrategy {
    /// Standard explore → plan → dispatch → evaluate loop.
    Normal,
    /// Use elite plan variants to exploit known good approaches.
    EliteExploitation,
    /// Inject elite success patterns into Explore prompt.
    CrossBranchReference,
    /// Reset with aggregated multi-branch plan (global stagnation).
    MultiBranchAggregation,
}

// ── Elite Entry ────────────────────────────────────────────────

/// A plan that achieved high fitness, stored in the elite set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EliteEntry {
    /// Role distribution signature for deduplication.
    pub role_signature: String,
    /// Fitness score when this plan was recorded.
    pub fitness: f64,
    /// Which loop this plan was born in.
    pub loop_born: usize,
    /// Success rate (0.0-1.0).
    pub success_rate: f64,
}

// ── Search Scheduler ───────────────────────────────────────────

/// SearchScheduler implements the MLEvolve-inspired Progressive MCGS
/// scheduling layer.
///
/// It controls *how* the loop pipeline searches for solutions:
/// - Entropy-driven exploration/exploitation balance
/// - Elite set for exploiting known-good plans
/// - Stagnation detection (branch-level and global-level)
/// - Cross-branch knowledge injection
///
/// This is Phase 4 of the MLEvolve integration.
pub struct SearchScheduler {
    /// Initial exploration weight w(0). Default 1.0 (pure exploration).
    pub entropy_initial: f64,
    /// Minimum exploration weight. Default 0.1 (mostly exploitation).
    pub entropy_min: f64,
    /// Decay factor per loop: w(t) = entropy_initial * entropy_decay^t
    pub entropy_decay: f64,
    /// K-factor for Elo-like scoring.
    pub elo_k_factor: f64,

    /// Elite set: top-K best plans seen so far.
    pub elite_set: Vec<EliteEntry>,
    /// Max size of elite set.
    pub elite_max_size: usize,

    /// Per-branch best fitness tracking.
    pub branch_best: HashMap<String, f64>,
    /// Per-branch stagnation counter (consecutive loops without improvement).
    pub branch_stagnation: HashMap<String, usize>,
    /// Global stagnation counter.
    pub global_stagnation: usize,

    /// Threshold for branch-level stagnation detection.
    pub branch_stagnation_threshold: usize,
    /// Threshold for global stagnation detection.
    pub global_stagnation_threshold: usize,

    /// Current loop count.
    pub loop_count: usize,
}

impl Default for SearchScheduler {
    fn default() -> Self {
        Self {
            entropy_initial: 1.0,
            entropy_min: 0.1,
            entropy_decay: 0.9,
            elo_k_factor: 32.0,
            elite_set: Vec::new(),
            elite_max_size: 10,
            branch_best: HashMap::new(),
            branch_stagnation: HashMap::new(),
            global_stagnation: 0,
            branch_stagnation_threshold: 3,
            global_stagnation_threshold: 5,
            loop_count: 0,
        }
    }
}

impl SearchScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_entropy(mut self, initial: f64, min: f64, decay: f64) -> Self {
        self.entropy_initial = initial;
        self.entropy_min = min;
        self.entropy_decay = decay;
        self
    }

    pub fn with_elite_size(mut self, max_size: usize) -> Self {
        self.elite_max_size = max_size;
        self
    }

    pub fn with_stagnation_thresholds(mut self, branch: usize, global: usize) -> Self {
        self.branch_stagnation_threshold = branch;
        self.global_stagnation_threshold = global;
        self
    }

    // ── Public API ─────────────────────────────────────────────

    /// Call at the start of each loop iteration to determine the search strategy.
    ///
    /// Returns `SearchStrategy::Normal` if no special action is needed.
    /// Returns other strategies when stagnation is detected or exploitation is favored.
    pub fn select_strategy(&mut self, branch: &str) -> SearchStrategy {
        self.loop_count += 1;

        // Check branch-level stagnation
        if let Some(stag) = self.branch_stagnation.get_mut(branch) {
            if *stag >= self.branch_stagnation_threshold {
                tracing::warn!(
                    branch = branch,
                    stagnation = *stag,
                    "Branch stagnation detected → CrossBranchReference"
                );
                return SearchStrategy::CrossBranchReference;
            }
        }

        // Check global stagnation
        if self.global_stagnation >= self.global_stagnation_threshold {
            tracing::warn!(
                global_stagnation = self.global_stagnation,
                "Global stagnation detected → MultiBranchAggregation"
            );
            return SearchStrategy::MultiBranchAggregation;
        }

        // Entropy-driven exploration/exploitation
        let w_t = self.current_entropy();
        let use_exploitation = rand::random::<f64>() > w_t;

        if use_exploitation && !self.elite_set.is_empty() {
            SearchStrategy::EliteExploitation
        } else {
            SearchStrategy::Normal
        }
    }

    /// Record the result of a loop iteration for a given branch.
    ///
    /// Call this after Evaluate to update stagnation counters and elite set.
    pub fn record_branch_result(
        &mut self,
        branch: &str,
        fitness: f64,
        success_rate: f64,
        role_signature: String,
    ) {
        // Update branch best
        let prev_best = self.branch_best.get(branch).copied().unwrap_or(0.0);
        if fitness > prev_best {
            self.branch_best.insert(branch.to_string(), fitness);
            self.branch_stagnation.insert(branch.to_string(), 0);
            self.global_stagnation = 0;

            // Update elite set
            self.update_elite_set(EliteEntry {
                role_signature,
                fitness,
                loop_born: self.loop_count,
                success_rate,
            });
        } else {
            // Increment stagnation counter
            let stag = self.branch_stagnation.entry(branch.to_string()).or_insert(0);
            *stag += 1;
            self.global_stagnation += 1;
        }
    }

    /// Get context for cross-branch reference (elite successes).
    pub fn elite_context(&self) -> Vec<&EliteEntry> {
        self.elite_set.iter().collect()
    }

    /// Get current entropy value w(t).
    pub fn current_entropy(&self) -> f64 {
        let w = self.entropy_initial * self.entropy_decay.powi(self.loop_count as i32);
        w.max(self.entropy_min)
    }

    /// Reset stagnation counters (e.g., after MultiBranchAggregation).
    pub fn reset_stagnation(&mut self) {
        self.branch_stagnation.clear();
        self.global_stagnation = 0;
    }

    // ── Private: Elite Set Management ──────────────────────────

    fn update_elite_set(&mut self, entry: EliteEntry) {
        // Check if this role signature already exists in elite set
        if let Some(pos) = self.elite_set.iter().position(|e| e.role_signature == entry.role_signature) {
            // Replace if new entry is better
            if entry.fitness > self.elite_set[pos].fitness {
                self.elite_set[pos] = entry;
            }
            return;
        }

        // Add new entry
        self.elite_set.push(entry);

        // Trim to max size, keeping highest fitness
        if self.elite_set.len() > self.elite_max_size {
            self.elite_set.sort_by(|a, b| b.fitness.partial_cmp(&a.fitness).unwrap_or(std::cmp::Ordering::Equal));
            self.elite_set.truncate(self.elite_max_size);
        }
    }
}
