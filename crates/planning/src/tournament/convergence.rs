use serde::{Deserialize, Serialize};

/// Detects Nash equilibrium by tracking rating variance over a rolling window.
/// When the variance among top-K hypotheses stabilizes below a threshold for
/// N consecutive rounds, the tournament is considered converged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NashEquilibriumDetector {
    /// Variance threshold for convergence (default 15.0).
    pub threshold: f64,
    /// Number of consecutive rounds below threshold to declare convergence (default 3).
    pub window: usize,
    /// Recent variance values (rolling window).
    pub variance_history: Vec<f64>,
}

impl Default for NashEquilibriumDetector {
    fn default() -> Self {
        Self {
            threshold: 15.0,
            window: 3,
            variance_history: Vec::new(),
        }
    }
}

impl NashEquilibriumDetector {
    pub fn new(threshold: f64, window: usize) -> Self {
        Self {
            threshold,
            window: window.max(1),
            variance_history: Vec::new(),
        }
    }

    /// Record a new variance measurement and check convergence.
    /// Returns true if Nash equilibrium is detected.
    pub fn check(&mut self, variance: f64) -> bool {
        self.variance_history.push(variance);

        // Keep only the rolling window
        if self.variance_history.len() > self.window {
            self.variance_history.remove(0);
        }

        // Need at least `window` measurements
        if self.variance_history.len() < self.window {
            return false;
        }

        // All measurements in the window must be below threshold
        self.variance_history.iter().all(|&v| v < self.threshold)
    }

    /// Current convergence status without recording.
    pub fn is_converged(&self) -> bool {
        if self.variance_history.len() < self.window {
            return false;
        }
        self.variance_history.iter().all(|&v| v < self.threshold)
    }

    /// Reset the detector (e.g., when new hypotheses enter the tournament).
    pub fn reset(&mut self) {
        self.variance_history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_converged_initially() {
        let detector = NashEquilibriumDetector::default();
        assert!(!detector.is_converged());
    }

    #[test]
    fn test_convergence_after_stable_window() {
        let mut detector = NashEquilibriumDetector::new(15.0, 3);

        // Below threshold but not enough measurements
        assert!(!detector.check(10.0));
        assert!(!detector.check(12.0));

        // Third measurement below threshold → converged
        assert!(detector.check(11.0));
    }

    #[test]
    fn test_no_convergence_with_high_variance() {
        let mut detector = NashEquilibriumDetector::new(15.0, 3);

        assert!(!detector.check(10.0));
        assert!(!detector.check(20.0)); // above threshold
        assert!(!detector.check(12.0)); // window has the 20.0 spike
    }

    #[test]
    fn test_convergence_resets() {
        let mut detector = NashEquilibriumDetector::new(15.0, 2);
        assert!(!detector.check(10.0)); // only 1 measurement, need 2
        assert!(detector.check(12.0));  // converged

        detector.reset();
        assert!(!detector.is_converged());
    }

    #[test]
    fn test_rolling_window_evicts_old() {
        let mut detector = NashEquilibriumDetector::new(15.0, 2);
        detector.check(100.0); // high, old
        detector.check(10.0);  // low
        // window = [100.0, 10.0] → not converged
        assert!(!detector.is_converged());

        detector.check(12.0); // low, evicts 100.0
        // window = [10.0, 12.0] → converged
        assert!(detector.is_converged());
    }
}
