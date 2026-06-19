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

    /// 根据选手已赛场次计算自适应 K-factor。
    ///
    /// 新选手（<10 场）K 更大以快速收敛到真实水平；老选手（>30 场）K 更小以稳定评分。
    /// 返回值基于 `self.k_factor`（基础 K）按经验缩放。
    fn effective_k(&self, matches: usize) -> f64 {
        let scale = if matches < 10 {
            1.25 // 新选手：基础 K × 1.25（如基础 32 → 实际 40）
        } else if matches < 30 {
            1.0  // 中期：标准 K
        } else {
            0.75 // 老选手：基础 K × 0.75（如基础 32 → 实际 24）
        };
        self.k_factor * scale
    }

    /// 时间衰减后的有效评分。
    ///
    /// 利用 `rating_history` 最后一次更新的时间戳，套指数衰减：
    /// `effective = rating * (DECAY_FLOOR + (1 - DECAY_FLOOR) * exp(-DECAY_RATE * days))`
    ///
    /// - `DECAY_FLOOR = 0.5`：评分最多衰减到原始值的一半（不归零）
    /// - `DECAY_RATE = 0.02`：复用 memory crate 的 hypothesis 默认衰减率（~35 天半衰期）
    ///
    /// 近期比赛的选手评分几乎不衰减；长期无比赛的选手评分逐步回归。
    /// [`rating_of`](Self::rating_of) 返回原始评分（向后兼容），本方法返回时效性评分。
    pub fn decayed_rating_of(&self, id: &str, now: DateTime<Utc>) -> f64 {
        const DECAY_FLOOR: f64 = 0.5;
        const DECAY_RATE: f64 = 0.02;

        let raw = self.rating_of(id);
        let last_match = self.ratings.get(id)
            .and_then(|r| r.rating_history.last().map(|(ts, _)| *ts));

        match last_match {
            None => raw,
            Some(ts) => {
                let days = (now - ts).num_seconds() as f64 / 86400.0;
                if days <= 0.0 { return raw; }
                let factor = DECAY_FLOOR + (1.0 - DECAY_FLOOR) * (-DECAY_RATE * days).exp();
                raw * factor
            }
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
    ///
    /// K-factor 按双方各自已赛场次自适应（见 [`effective_k`]）：新选手波动大、
    /// 老选手稳定。这比固定 K 更符合实际——爆冷时新选手的评分调整更激进。
    pub fn update_after_match(
        &mut self,
        id_a: &str,
        id_b: &str,
        outcome: MatchOutcome,
    ) -> (f64, f64) {
        let rating_a = self.rating_of(id_a);
        let rating_b = self.rating_of(id_b);

        // 双方各自的自适应 K-factor（基于已赛场次）
        let matches_a = self.ratings.get(id_a).map(|r| r.matches).unwrap_or(0);
        let matches_b = self.ratings.get(id_b).map(|r| r.matches).unwrap_or(0);
        let k_a = self.effective_k(matches_a);
        let k_b = self.effective_k(matches_b);

        let expected_a = Self::expected_score(rating_a, rating_b);
        let expected_b = 1.0 - expected_a;

        let (score_a, score_b) = match outcome {
            MatchOutcome::WinA => (1.0, 0.0),
            MatchOutcome::WinB => (0.0, 1.0),
            MatchOutcome::Draw => (0.5, 0.5),
        };

        let delta_a = k_a * (score_a - expected_a);
        let delta_b = k_b * (score_b - expected_b);

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

    /// Get the top-K rated hypotheses, sorted by **time-decayed** rating.
    ///
    /// 排序用 [`decayed_rating_of`]（近期比赛权重更高），而非原始 `rating`——
    /// 这使长期无比赛的选手不会凭旧分数占据排名。
    /// `now` 通常传 `Utc::now()`；测试可传固定时间戳。
    pub fn top_k(&self, k: usize) -> Vec<&PlayerRating> {
        self.top_k_at(k, Utc::now())
    }

    /// 与 [`top_k`] 相同，但接受显式时间戳（便于测试）。
    pub fn top_k_at(&self, k: usize, now: DateTime<Utc>) -> Vec<&PlayerRating> {
        // 性能优化：预计算每个选手的 decayed rating 一次（O(M×H)），
        // 再按预计算值排序（O(M log M) 比较，每次 O(1)）。
        // 旧实现在 sort_by 闭包内每次比较都重算 decayed_rating_of（O(M log M × H)）。
        let mut scored: Vec<(&PlayerRating, f64)> = self.ratings.values()
            .map(|r| {
                let decayed = self.decayed_rating_of(&r.hypothesis_id, now);
                (r, decayed)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored.into_iter().map(|(r, _)| r).collect()
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

    #[test]
    fn test_k_factor_decreases_with_experience() {
        // 新选手（0 场）的 K-factor 应大于老选手（>30 场）
        let engine = EloEngine::new(32.0, 1000.0);
        let k_new = engine.effective_k(0);
        let k_mid = engine.effective_k(15);
        let k_old = engine.effective_k(40);
        assert!(k_new > k_mid, "new player K ({k_new}) should exceed mid ({k_mid})");
        assert!(k_mid > k_old, "mid K ({k_mid}) should exceed old ({k_old})");
        assert!((k_new - 40.0).abs() < 0.01, "new=32*1.25=40, got {k_new}");
        assert!((k_old - 24.0).abs() < 0.01, "old=32*0.75=24, got {k_old}");
    }

    #[test]
    fn test_upset_gain_larger_for_new_player() {
        // 爆冷时，新选手（少场次、大 K）的 delta 应大于老选手（多场次、小 K）
        let mut engine_new = EloEngine::new(32.0, 1000.0);
        engine_new.register("weak");
        engine_new.register("strong");
        // 先让 strong 积累优势（但不增加 weak 的场次过多）
        engine_new.update_after_match("strong", "weak", MatchOutcome::WinA);
        let (delta_new, _) = engine_new.update_after_match("weak", "strong", MatchOutcome::WinA);

        // 老选手场景：同样 setup 但双方都已踢 35 场
        let mut engine_old = EloEngine::new(32.0, 1000.0);
        engine_old.register("weak");
        engine_old.register("strong");
        // 模拟大量比赛使双方成为"老选手"
        for _ in 0..35 {
            engine_old.update_after_match("strong", "weak", MatchOutcome::WinA);
        }
        let (delta_old, _) = engine_old.update_after_match("weak", "strong", MatchOutcome::WinA);

        assert!(delta_new > delta_old,
            "upset delta for new player ({delta_new}) should exceed old player ({delta_old})");
    }

    #[test]
    fn test_decayed_rating_decreases_over_time() {
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("h1");
        engine.register("h2");
        engine.update_after_match("h1", "h2", MatchOutcome::WinA);

        let raw = engine.rating_of("h1");
        let now = Utc::now();
        // 刚比赛完：衰减后 ≈ 原始值（days≈0）
        let fresh = engine.decayed_rating_of("h1", now);
        assert!((fresh - raw).abs() < 0.1, "fresh match should not decay: raw={raw}, decayed={fresh}");

        // 60 天后：应有明显衰减（floor=0.5, rate=0.02 → 60天 factor≈0.5+0.5*exp(-1.2)≈0.65）
        let old = engine.decayed_rating_of("h1", now + chrono::Duration::days(60));
        assert!(old < raw, "60-day-old rating ({old}) should be below raw ({raw})");
        assert!(old > raw * 0.5, "but not below floor (50% of raw): {old} vs {}", raw * 0.5);
    }

    #[test]
    fn test_top_k_at_uses_decayed_rating() {
        // 两个选手：h_old 曾高分但 100 天未赛，h_recent 近期获胜。
        // 用 decayed 排序时 h_recent 应排前面（即使原始 rating h_old 更高）。
        let mut engine = EloEngine::new(32.0, 1000.0);
        engine.register("h_old");
        engine.register("h_recent");

        // h_old 先积累高分
        engine.update_after_match("h_old", "h_recent", MatchOutcome::WinA);
        engine.update_after_match("h_old", "h_recent", MatchOutcome::WinA);
        let old_raw = engine.rating_of("h_old");
        let recent_raw = engine.rating_of("h_recent");
        assert!(old_raw > recent_raw, "precondition: h_old raw > h_recent raw");

        // 模拟时间流逝 100 天，然后 h_recent 近期获胜
        let past = Utc::now() - chrono::Duration::days(100);
        // 手动篡改 h_old 的 history 时间戳到 100 天前（模拟长期未赛）
        if let Some(r) = engine.ratings.get_mut("h_old") {
            r.rating_history = vec![(past, old_raw)];
        }
        // h_recent 保持近期（现在）

        let now = Utc::now();
        let top = engine.top_k_at(2, now);
        // h_recent（近期）应排第一
        assert_eq!(top[0].hypothesis_id, "h_recent",
            "decayed top_k should favor recent player over stale high-rater");
    }
}
