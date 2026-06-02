use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Outcome of a pairwise match between two hypotheses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchOutcome {
    WinA,
    WinB,
    Draw,
}

/// Tracks Elo rating and match statistics for a single hypothesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerRating {
    pub hypothesis_id: String,
    pub rating: f64,
    pub matches: usize,
    pub wins: usize,
    pub losses: usize,
    pub draws: usize,
    pub rating_history: Vec<(DateTime<Utc>, f64)>,
}

impl PlayerRating {
    pub fn new(hypothesis_id: String, initial_rating: f64) -> Self {
        Self {
            hypothesis_id,
            rating: initial_rating,
            matches: 0,
            wins: 0,
            losses: 0,
            draws: 0,
            rating_history: vec![(Utc::now(), initial_rating)],
        }
    }

    pub fn win_rate(&self) -> f64 {
        if self.matches == 0 {
            0.0
        } else {
            (self.wins as f64 + 0.5 * self.draws as f64) / self.matches as f64
        }
    }
}

/// Elo rating engine for hypothesis tournament.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EloEngine {
    pub k_factor: f64,
    pub initial_rating: f64,
    pub ratings: HashMap<String, PlayerRating>,
}

impl Default for EloEngine {
    fn default() -> Self {
        Self {
            k_factor: 32.0,
            initial_rating: 1000.0,
            ratings: HashMap::new(),
        }
    }
}

impl EloEngine {
    pub fn new(k_factor: f64, initial_rating: f64) -> Self {
        Self {
            k_factor,
            initial_rating,
            ratings: HashMap::new(),
        }
    }

    /// Register a new hypothesis in the rating system.
    pub fn register(&mut self, id: impl Into<String>) {
        let id = id.into();
        self.ratings.entry(id.clone()).or_insert_with(|| {
            PlayerRating::new(id, self.initial_rating)
        });
    }

    /// Expected score for player A against player B (0.0 to 1.0).
    /// Uses the standard Elo formula: E_A = 1 / (1 + 10^((R_B - R_A) / 400))
    pub fn expected_score(rating_a: f64, rating_b: f64) -> f64 {
        1.0 / (1.0 + 10.0_f64.powf((rating_b - rating_a) / 400.0))
    }

    /// Update ratings after a match between two hypotheses.
    /// Returns the rating changes (delta_a, delta_b).
    pub fn update_after_match(
        &mut self,
        id_a: &str,
        id_b: &str,
        outcome: MatchOutcome,
    ) -> (f64, f64) {
        let rating_a = self.rating_of(id_a);
        let rating_b = self.rating_of(id_b);

        let expected_a = Self::expected_score(rating_a, rating_b);
        let expected_b = 1.0 - expected_a;

        let (score_a, score_b) = match outcome {
            MatchOutcome::WinA => (1.0, 0.0),
            MatchOutcome::WinB => (0.0, 1.0),
            MatchOutcome::Draw => (0.5, 0.5),
        };

        let delta_a = self.k_factor * (score_a - expected_a);
        let delta_b = self.k_factor * (score_b - expected_b);

        let now = Utc::now();

        if let Some(a) = self.ratings.get_mut(id_a) {
            a.rating += delta_a;
            a.matches += 1;
            match outcome {
                MatchOutcome::WinA => a.wins += 1,
                MatchOutcome::WinB => a.losses += 1,
                MatchOutcome::Draw => a.draws += 1,
            }
            a.rating_history.push((now, a.rating));
        }

        if let Some(b) = self.ratings.get_mut(id_b) {
            b.rating += delta_b;
            b.matches += 1;
            match outcome {
                MatchOutcome::WinA => b.losses += 1,
                MatchOutcome::WinB => b.wins += 1,
                MatchOutcome::Draw => b.draws += 1,
            }
            b.rating_history.push((now, b.rating));
        }

        (delta_a, delta_b)
    }

    /// Get current rating for a hypothesis. Returns initial rating if not registered.
    pub fn rating_of(&self, id: &str) -> f64 {
        self.ratings.get(id).map(|r| r.rating).unwrap_or(self.initial_rating)
    }

    /// Get the top-K rated hypotheses.
    pub fn top_k(&self, k: usize) -> Vec<&PlayerRating> {
        let mut sorted: Vec<&PlayerRating> = self.ratings.values().collect();
        sorted.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(k);
        sorted
    }

    /// Compute rating variance among the top-K hypotheses.
    /// Used for Nash equilibrium detection.
    pub fn rating_variance_top_k(&self, k: usize) -> f64 {
        let top = self.top_k(k);
        if top.is_empty() {
            return 0.0;
        }
        let mean = top.iter().map(|r| r.rating).sum::<f64>() / top.len() as f64;
        
        top.iter()
            .map(|r| (r.rating - mean).powi(2))
            .sum::<f64>()
            / top.len() as f64
    }

    /// Total number of registered hypotheses.
    pub fn len(&self) -> usize {
        self.ratings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ratings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expected_score_symmetric() {
        let e = EloEngine::expected_score(1000.0, 1000.0);
        assert!((e - 0.5).abs() < 0.01, "Equal ratings should give ~0.5, got {e}");
    }

    #[test]
    fn test_expected_score_higher_favored() {
        let e = EloEngine::expected_score(1200.0, 800.0);
        assert!(e > 0.5, "Higher rated player should be favored, got {e}");
        assert!(e < 1.0, "Should not be certain, got {e}");
    }

    #[test]
    fn test_update_after_win() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("h1");
        engine.register("h2");

        let (delta_a, delta_b) = engine.update_after_match("h1", "h2", MatchOutcome::WinA);
        assert!(delta_a > 0.0, "Winner should gain rating");
        assert!(delta_b < 0.0, "Loser should lose rating");

        assert_eq!(engine.rating_of("h1"), 1000.0 + delta_a);
        assert_eq!(engine.rating_of("h2"), 1000.0 + delta_b);
    }

    #[test]
    fn test_upset_gains_more() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("weak");
        engine.register("strong");

        // Boost strong's rating
        engine.update_after_match("strong", "weak", MatchOutcome::WinA);
        engine.update_after_match("strong", "weak", MatchOutcome::WinA);
        engine.update_after_match("strong", "weak", MatchOutcome::WinA);

        let strong_rating = engine.rating_of("strong");
        let weak_rating = engine.rating_of("weak");

        // Now weak wins (upset)
        let (delta_weak, _) = engine.update_after_match("weak", "strong", MatchOutcome::WinA);
        // Upset win should give more points than a typical win
        assert!(delta_weak > 16.0, "Upset should give significant gain, got {delta_weak}");
        assert!(strong_rating > weak_rating, "Strong should still be rated higher before upset");
    }

    #[test]
    fn test_draw_between_equals() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("h1");
        engine.register("h2");

        let (delta_a, delta_b) = engine.update_after_match("h1", "h2", MatchOutcome::Draw);
        assert!((delta_a - 0.0).abs() < 0.01, "Draw between equals should give ~0 delta");
        assert!((delta_b - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_top_k_ordering() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("a");
        engine.register("b");
        engine.register("c");

        engine.update_after_match("a", "b", MatchOutcome::WinA);
        engine.update_after_match("a", "c", MatchOutcome::WinA);
        engine.update_after_match("b", "c", MatchOutcome::WinA);

        let top = engine.top_k(2);
        assert_eq!(top[0].hypothesis_id, "a");
        assert_eq!(top[1].hypothesis_id, "b");
    }

    #[test]
    fn test_rating_variance_decreases_with_convergence() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        for i in 0..4 {
            engine.register(format!("h{i}"));
        }

        // Initial variance should be 0 (all equal)
        let initial_var = engine.rating_variance_top_k(4);
        assert!(initial_var < 0.01);

        // Create spread
        engine.update_after_match("h0", "h1", MatchOutcome::WinA);
        engine.update_after_match("h0", "h2", MatchOutcome::WinA);
        engine.update_after_match("h0", "h3", MatchOutcome::WinA);

        let spread_var = engine.rating_variance_top_k(4);
        assert!(spread_var > initial_var, "Spread should increase variance");
    }

    #[test]
    fn test_player_rating_stats() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("h1");
        engine.register("h2");

        engine.update_after_match("h1", "h2", MatchOutcome::WinA);
        engine.update_after_match("h1", "h2", MatchOutcome::Draw);
        engine.update_after_match("h1", "h2", MatchOutcome::WinB);

        let r = engine.ratings.get("h1").unwrap();
        assert_eq!(r.matches, 3);
        assert_eq!(r.wins, 1);
        assert_eq!(r.losses, 1);
        assert_eq!(r.draws, 1);
        assert_eq!(r.rating_history.len(), 4); // initial + 3 matches
    }
}
