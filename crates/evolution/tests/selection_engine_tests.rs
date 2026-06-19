use miniagent_evolution::selection_engine::{CandidateSource, MutationOp, SelectionEngine, TaskUnit};
use miniagent_core::TaskPlan;

// ── Helpers ────────────────────────────────────────────────────

fn sample_plan() -> TaskPlan {
    TaskPlan {
        overall_goal: "Build a web scraper and generate a report".into(),
        tasks: vec![
            TaskUnit {
                id: "task_1".into(),
                description: "Research target website structure".into(),
                assigned_role: "researcher".into(),
                depends_on: vec![],
                expected_output: "Website structure analysis".into(),
                difficulty: "medium".into(),
                failed: false,
                error: None,
                output: None,
            },
            TaskUnit {
                id: "task_2".into(),
                description: "Implement Python scraper".into(),
                assigned_role: "executor".into(),
                depends_on: vec!["task_1".into()],
                expected_output: "scraper.py".into(),
                difficulty: "hard".into(),
                failed: false,
                error: None,
                output: None,
            },
            TaskUnit {
                id: "task_3".into(),
                description: "Write analysis report".into(),
                assigned_role: "writer".into(),
                depends_on: vec!["task_2".into()],
                expected_output: "report.md".into(),
                difficulty: "medium".into(),
                failed: false,
                error: None,
                output: None,
            },
        ],
        max_loops: 5,
    }
}

fn simple_plan() -> TaskPlan {
    TaskPlan {
        overall_goal: "Simple task".into(),
        tasks: vec![
            TaskUnit {
                id: "t1".into(),
                description: "Do something".into(),
                assigned_role: "executor".into(),
                depends_on: vec![],
                expected_output: "result".into(),
                difficulty: "easy".into(),
                failed: false,
                error: None,
                output: None,
            },
        ],
        max_loops: 3,
    }
}

// ── SelectionEngine: disabled ──────────────────────────────────

#[test]
fn test_disabled_returns_original() {
    let mut engine = SelectionEngine::new(3).with_enabled(false);
    let plan = sample_plan();
    let result = engine.select(&plan);
    assert_eq!(result.tasks.len(), plan.tasks.len());
    assert_eq!(result.overall_goal, plan.overall_goal);
}

#[test]
fn test_population_size_1_returns_original() {
    let mut engine = SelectionEngine::new(1);
    let plan = sample_plan();
    let result = engine.select(&plan);
    assert_eq!(result.tasks.len(), 3);
}

// ── SelectionEngine: generate_candidates ──────────────────────

#[test]
fn test_generate_candidates_has_original() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let candidates = engine.generate_candidates(&plan);
    assert_eq!(candidates.len(), 3, "Should generate 3 candidates (1 original + 2 variants)");
    assert_eq!(candidates[0].source, CandidateSource::Original);
}

#[test]
fn test_generate_candidates_population_size_5() {
    let engine = SelectionEngine::new(5);
    let plan = sample_plan(); // 3 tasks
    let candidates = engine.generate_candidates(&plan);
    // Variants capped by task count: 1 original + min(4, 3) = 4
    assert_eq!(candidates.len(), 4);
}

#[test]
fn test_generate_candidates_single_task() {
    let engine = SelectionEngine::new(3);
    let plan = simple_plan();
    let candidates = engine.generate_candidates(&plan);
    // With 1 task and population=3, should still generate at least 1 variant
    assert!(!candidates.is_empty());
}

// ── SelectionEngine: select ───────────────────────────────────

#[test]
fn test_select_returns_valid_plan() {
    let mut engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let result = engine.select(&plan);
    assert_eq!(result.overall_goal, plan.overall_goal);
    assert!(!result.tasks.is_empty(), "Result should have tasks");
}

#[test]
fn test_select_preserves_task_count_or_increases() {
    let mut engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let original_count = plan.tasks.len();
    let result = engine.select(&plan);
    // Mutation may add tasks (injection), but never remove them
    assert!(result.tasks.len() >= original_count,
        "Result tasks ({}) should be >= original ({})", result.tasks.len(), original_count);
}

#[test]
fn test_select_updates_elo_ratings() {
    let mut engine = SelectionEngine::new(3);
    let plan = sample_plan();
    engine.select(&plan);
    assert!(!engine.elo_ratings.is_empty(), "Elo ratings should be updated after selection");
}

#[test]
fn test_select_with_experiences() {
    let experiences = vec![
        miniagent_evolution::ExperienceSummary {
            description: "Use pubmed_search for scientific queries".into(),
            lessons: vec!["pubmed_search gives better results".into()],
            node_type: "successpattern".into(),
            confidence: 0.9,
        },
    ];
    let mut engine = SelectionEngine::new(3)
        .with_experiences(experiences);
    let plan = sample_plan();
    let result = engine.select(&plan);
    assert!(!result.tasks.is_empty());
}

// ── SelectionEngine: quick_fitness ────────────────────────────

#[test]
fn test_fitness_optimal_task_count() {
    let engine = SelectionEngine::new(3);
    // 5 tasks is optimal (3-8 range)
    let plan = sample_plan();
    let fitness = engine.quick_fitness(&plan);
    assert!(fitness > 0.0, "Fitness should be positive for optimal count");
    assert!(fitness <= 1.0, "Fitness should not exceed 1.0");
}

#[test]
fn test_fitness_too_few_tasks() {
    let engine = SelectionEngine::new(3);
    let plan = simple_plan(); // 1 task
    let fitness = engine.quick_fitness(&plan);
    assert!(fitness < 1.0, "Single-task plan should have lower fitness");
}

#[test]
fn test_fitness_deterministic() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let f1 = engine.quick_fitness(&plan);
    let f2 = engine.quick_fitness(&plan);
    assert_eq!(f1, f2, "Fitness should be deterministic for same plan");
}

// ── SelectionEngine: mutation operators ───────────────────────

#[test]
fn test_mutate_plan_preserves_overall_goal() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let (mutated, ops) = engine.mutate_plan(&plan, 0);
    assert_eq!(mutated.overall_goal, plan.overall_goal, "Overall goal should not change");
}

#[test]
fn test_mutate_plan_preserves_or_adds_tasks() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let original_count = plan.tasks.len();
    let (mutated, _ops) = engine.mutate_plan(&plan, 0);
    assert!(mutated.tasks.len() >= original_count,
        "Mutation should not remove tasks: {} -> {}", original_count, mutated.tasks.len());
}

#[test]
fn test_mutate_plan_records_ops() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let (_mutated, ops) = engine.mutate_plan(&plan, 0);
    // With 3 tasks and 20% mutation rate, at least some ops may be generated
    // (depends on random seed, but the function should not panic)
    // Just verify it returns without error
    assert!(ops.len() <= plan.tasks.len(), "Ops should not exceed task count");
}

#[test]
fn test_mutate_empty_plan() {
    let engine = SelectionEngine::new(3);
    let empty_plan = TaskPlan {
        overall_goal: "Empty".into(),
        tasks: vec![],
        max_loops: 1,
    };
    let (mutated, ops) = engine.mutate_plan(&empty_plan, 0);
    assert!(mutated.tasks.is_empty());
    assert!(ops.is_empty());
}

// ── SelectionEngine: Elo ──────────────────────────────────────

#[test]
fn test_elo_update_basic() {
    let engine = SelectionEngine::new(3);
    let (w, l) = engine.update_elo(1200.0, 1200.0, 1.0);
    // Equal ratings, winner should gain, loser should lose
    assert!(w > 1200.0, "Winner rating should increase: got {}", w);
    assert!(l < 1200.0, "Loser rating should decrease: got {}", l);
}

#[test]
fn test_elo_update_conservation() {
    let engine = SelectionEngine::new(3);
    let (w, l) = engine.update_elo(1200.0, 1200.0, 1.0);
    // Total rating should be approximately conserved
    let total_before = 1200.0 + 1200.0;
    let total_after = w + l;
    assert!((total_after - total_before).abs() < 0.01,
        "Total Elo should be conserved: {} -> {}", total_before, total_after);
}

#[test]
fn test_elo_upset() {
    // Lower-rated winner beats higher-rated loser
    let engine = SelectionEngine::new(3);
    let (w, l) = engine.update_elo(1100.0, 1300.0, 1.0);
    assert!(w > 1100.0, "Underdog winner should gain more: got {}", w);
    assert!(l < 1300.0, "Favorite loser should lose more: got {}", l);
}

// ── SelectionEngine: plan_signature ───────────────────────────

#[test]
fn test_plan_signature_deterministic() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let sig1 = engine.plan_signature(&plan);
    let sig2 = engine.plan_signature(&plan);
    assert_eq!(sig1, sig2);
}

#[test]
fn test_plan_signature_different_roles() {
    let engine = SelectionEngine::new(3);
    let mut plan1 = sample_plan();
    let mut plan2 = sample_plan();
    plan2.tasks[0].assigned_role = "writer".into();
    let sig1 = engine.plan_signature(&plan1);
    let sig2 = engine.plan_signature(&plan2);
    assert_ne!(sig1, sig2, "Different role assignments should produce different signatures");
}

// ── SelectionEngine: edge cases ───────────────────────────────

#[test]
fn test_select_with_experience_pool_injection() {
    let experiences = vec![
        miniagent_evolution::ExperienceSummary {
            description: "Always verify API endpoints before coding".into(),
            lessons: vec!["Check docs first".into()],
            node_type: "successpattern".into(),
            confidence: 0.85,
        },
    ];
    let mut engine = SelectionEngine::new(3)
        .with_experiences(experiences);
    let plan = sample_plan();
    let result = engine.select(&plan);
    // With experience pool, some variants may have injected tasks
    assert!(!result.tasks.is_empty());
}

#[test]
fn test_fitness_score_range() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let fitness = engine.quick_fitness(&plan);
    assert!(fitness >= 0.0, "Fitness should be >= 0: {}", fitness);
    assert!(fitness <= 1.0, "Fitness should be <= 1: {}", fitness);
}

#[test]
fn test_candidate_source_original() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let candidates = engine.generate_candidates(&plan);
    assert_eq!(candidates[0].source, CandidateSource::Original);
}

#[test]
fn test_candidate_source_mutated() {
    let engine = SelectionEngine::new(3);
    let plan = sample_plan();
    let candidates = engine.generate_candidates(&plan);
    for c in &candidates[1..] {
        assert_eq!(c.source, CandidateSource::Mutated);
    }
}
