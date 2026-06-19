use miniagent_evolution::search_scheduler::{EliteEntry, SearchScheduler, SearchStrategy};

// ── SearchScheduler: Basic ─────────────────────────────────────

#[test]
fn test_scheduler_default() {
    let scheduler = SearchScheduler::new();
    assert_eq!(scheduler.entropy_initial, 1.0);
    assert_eq!(scheduler.entropy_min, 0.1);
    assert_eq!(scheduler.entropy_decay, 0.9);
    assert_eq!(scheduler.elite_max_size, 10);
    assert_eq!(scheduler.branch_stagnation_threshold, 3);
    assert_eq!(scheduler.global_stagnation_threshold, 5);
}

#[test]
fn test_current_entropy_decays() {
    let scheduler = SearchScheduler::new();
    let e0 = scheduler.current_entropy();
    assert!((e0 - 1.0).abs() < 0.01, "Initial entropy should be ~1.0, got {}", e0);

    // Simulate loop_count incrementing
    let mut s = SearchScheduler::new();
    s.loop_count = 5;
    let e5 = s.current_entropy();
    assert!(e5 < e0, "Entropy should decay: {} < {}", e5, e0);
    assert!(e5 >= 0.1, "Entropy should not go below min: {}", e5);
}

#[test]
fn test_entropy_respects_min() {
    let mut scheduler = SearchScheduler::new();
    scheduler.loop_count = 1000;
    let e = scheduler.current_entropy();
    assert!((e - 0.1).abs() < 0.01, "Entropy should floor at min: {}", e);
}

#[test]
fn test_select_strategy_normal_initially() {
    let mut scheduler = SearchScheduler::new();
    // First call: loop_count becomes 1, no stagnation
    let strategy = scheduler.select_strategy("main");
    assert_eq!(strategy, SearchStrategy::Normal);
}

// ── SearchScheduler: Elite Set ─────────────────────────────────

#[test]
fn test_elite_set_accumulates() {
    let mut scheduler = SearchScheduler::new();
    scheduler.record_branch_result("main", 0.8, 0.8, "role_a".into());
    scheduler.record_branch_result("main", 0.9, 0.9, "role_b".into());
    assert_eq!(scheduler.elite_set.len(), 2);
}

#[test]
fn test_elite_set_max_size() {
    let mut scheduler = SearchScheduler::with_elite_size(SearchScheduler::new(), 3);
    for i in 0..5 {
        scheduler.record_branch_result(
            "main",
            0.5 + i as f64 * 0.1,
            0.5 + i as f64 * 0.1,
            format!("role_{}", i),
        );
    }
    assert_eq!(scheduler.elite_set.len(), 3, "Should trim to max size");
    // Should keep the top 3 fitness entries (0.7, 0.8, 0.9)
    let fitnesses: Vec<f64> = scheduler.elite_set.iter().map(|e| e.fitness).collect();
    assert!(fitnesses.contains(&0.9));
    assert!(fitnesses.contains(&0.8));
    assert!(fitnesses.contains(&0.7));
}

#[test]
fn test_elite_set_replaces_on_better() {
    let mut scheduler = SearchScheduler::new();
    // First entry with role_a
    scheduler.record_branch_result("main", 0.5, 0.5, "role_a".into());
    assert_eq!(scheduler.elite_set.len(), 1);
    assert!((scheduler.elite_set[0].fitness - 0.5).abs() < 0.01);

    // Better entry with same role signature should replace
    scheduler.record_branch_result("main", 0.9, 0.9, "role_a".into());
    assert_eq!(scheduler.elite_set.len(), 1);
    assert!((scheduler.elite_set[0].fitness - 0.9).abs() < 0.01);
}

#[test]
fn test_elite_context_returns_refs() {
    let mut scheduler = SearchScheduler::new();
    scheduler.record_branch_result("main", 0.8, 0.8, "role_a".into());
    let ctx = scheduler.elite_context();
    assert_eq!(ctx.len(), 1);
    assert!((ctx[0].fitness - 0.8).abs() < 0.01);
}

// ── SearchScheduler: Stagnation Detection ─────────────────────

#[test]
fn test_branch_stagnation_triggers_cross_branch() {
    let mut scheduler = SearchScheduler::new();
    scheduler.branch_stagnation_threshold = 3;

    // First call: loop_count=1, no stagnation
    let s1 = scheduler.select_strategy("main");
    assert_eq!(s1, SearchStrategy::Normal);

    // Manually set stagnation to trigger CrossBranchReference
    {
        let stag = scheduler.branch_stagnation.entry("main".into()).or_insert(0);
        *stag = 3; // At threshold
    }

    let s2 = scheduler.select_strategy("main");
    assert_eq!(s2, SearchStrategy::CrossBranchReference);
}

#[test]
fn test_global_stagnation_triggers_multi_branch() {
    let mut scheduler = SearchScheduler::new();
    scheduler.global_stagnation_threshold = 5;
    scheduler.global_stagnation = 5; // At threshold

    let strategy = scheduler.select_strategy("main");
    assert_eq!(strategy, SearchStrategy::MultiBranchAggregation);
}

#[test]
fn test_reset_stagnation() {
    let mut scheduler = SearchScheduler::new();
    scheduler.global_stagnation = 10;
    scheduler.branch_stagnation.insert("main".into(), 5);

    scheduler.reset_stagnation();

    assert_eq!(scheduler.global_stagnation, 0);
    assert!(scheduler.branch_stagnation.is_empty());
}

#[test]
fn test_record_branch_improvement_resets_stagnation() {
    let mut scheduler = SearchScheduler::new();
    scheduler.branch_stagnation.insert("main".into(), 2);
    scheduler.global_stagnation = 3;

    // Improvement resets both
    scheduler.record_branch_result("main", 0.9, 0.9, "role_a".into());

    assert_eq!(scheduler.branch_stagnation.get("main"), Some(&0));
    assert_eq!(scheduler.global_stagnation, 0);
}

#[test]
fn test_record_branch_no_improvement_increments_stagnation() {
    let mut scheduler = SearchScheduler::new();
    scheduler.branch_best.insert("main".into(), 0.9);
    scheduler.branch_stagnation.insert("main".into(), 1);
    scheduler.global_stagnation = 2;

    // No improvement (fitness 0.8 < 0.9)
    scheduler.record_branch_result("main", 0.8, 0.8, "role_b".into());

    assert_eq!(scheduler.branch_stagnation.get("main"), Some(&2));
    assert_eq!(scheduler.global_stagnation, 3);
}

// ── SearchScheduler: Elite Exploitation Selection ──────────────

#[test]
fn test_elite_exploitation_when_elite_exists() {
    let mut scheduler = SearchScheduler::new();
    scheduler.record_branch_result("main", 0.9, 0.9, "role_a".into());

    // With entropy ~0.9 (loop_count=1), rand() > 0.9 is ~10%
    // With entropy ~0.1 (loop_count=100), rand() > 0.1 is ~90%
    // Test at high loop_count where exploitation should dominate
    scheduler.loop_count = 100;
    let mut elite_count = 0;
    let mut normal_count = 0;
    for _ in 0..100 {
        let strategy = scheduler.select_strategy("main");
        match strategy {
            SearchStrategy::EliteExploitation => elite_count += 1,
            SearchStrategy::Normal => normal_count += 1,
            _ => {}
        }
    }

    // With entropy=0.1, ~90% should be EliteExploitation
    assert!(elite_count > 50, "Should frequently exploit with low entropy: {} / 100", elite_count);
    assert!(normal_count < 50, "Should rarely explore with low entropy: {} / 100", normal_count);
}

#[test]
fn test_elite_exploitation_with_low_entropy() {
    let mut scheduler = SearchScheduler::new();
    scheduler.loop_count = 100; // entropy will be ~0.1 (min)
    scheduler.record_branch_result("main", 0.9, 0.9, "role_a".into());

    let mut elite_count = 0;
    let mut normal_count = 0;
    for _ in 0..100 {
        let strategy = scheduler.select_strategy("main");
        match strategy {
            SearchStrategy::EliteExploitation => elite_count += 1,
            SearchStrategy::Normal => normal_count += 1,
            _ => {}
        }
    }

    // With entropy=0.1, rand() > 0.1 ~90% of the time
    assert!(elite_count > 50, "Should frequently exploit with low entropy: {} / 100", elite_count);
}

// ── SearchScheduler: Edge Cases ────────────────────────────────

#[test]
fn test_empty_elite_no_exploitation() {
    let mut scheduler = SearchScheduler::new();
    // No elite entries
    let strategy = scheduler.select_strategy("main");
    assert_eq!(strategy, SearchStrategy::Normal);
}

#[test]
fn test_multiple_branches_tracked() {
    let mut scheduler = SearchScheduler::new();
    scheduler.record_branch_result("branch_a", 0.8, 0.8, "role_a".into());
    scheduler.record_branch_result("branch_b", 0.7, 0.7, "role_b".into());

    assert_eq!(scheduler.branch_best.get("branch_a"), Some(&0.8));
    assert_eq!(scheduler.branch_best.get("branch_b"), Some(&0.7));
    assert_eq!(scheduler.elite_set.len(), 2);
}

#[test]
fn test_elite_entry_fields() {
    let entry = EliteEntry {
        role_signature: "[\"executor\", \"writer\"]".into(),
        fitness: 0.85,
        loop_born: 3,
        success_rate: 0.9,
    };
    assert_eq!(entry.role_signature, "[\"executor\", \"writer\"]");
    assert!((entry.fitness - 0.85).abs() < 0.01);
    assert_eq!(entry.loop_born, 3);
    assert!((entry.success_rate - 0.9).abs() < 0.01);
}

#[test]
fn test_search_strategy_serialization() {
    let strategy = SearchStrategy::EliteExploitation;
    let json = serde_json::to_string(&strategy).unwrap();
    let decoded: SearchStrategy = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, strategy);
}
