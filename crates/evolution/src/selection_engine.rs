use std::collections::HashMap;

use miniagent_core::TaskPlan;
use crate::cold_start_kb::DomainTemplate;
use crate::memory_router::RetrievalContext;
use crate::{ExperienceSummary, MemoryRetriever};
use serde::{Deserialize, Serialize};

// ── Re-export core types for downstream consumers ──────────────

pub use miniagent_core::TaskUnit;

// ── Candidate Plan ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CandidatePlan {
    pub plan: TaskPlan,
    pub fitness: f64,
    pub elo_rating: f64,
    pub source: CandidateSource,
    pub mutations_applied: Vec<MutationOp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    Original,
    Mutated,
    Injected,
}

// ── Mutation Operators ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOp {
    SwapRole { task_id: String, new_role: String },
    AddDependency { task_id: String, depends_on: Vec<String> },
    RemoveDependency { task_id: String, remove_dep: String },
    SplitTask { task_id: String, new_task_a: String, new_task_b: String },
    InjectFromExperience { experience_description: String, suggested_role: String },
}

// ── Selection Engine ───────────────────────────────────────────

pub struct SelectionEngine {
    pub population_size: usize,
    pub mutation_rate: f64,
    pub elo_k_factor: f64,
    pub experience_pool: Vec<ExperienceSummary>,
    pub elo_ratings: HashMap<String, f64>,
    pub enabled: bool,
}

impl Default for SelectionEngine {
    fn default() -> Self {
        Self {
            population_size: 3,
            mutation_rate: 0.2,
            elo_k_factor: 32.0,
            experience_pool: Vec::new(),
            elo_ratings: HashMap::new(),
            enabled: true,
        }
    }
}

impl SelectionEngine {
    pub fn new(population_size: usize) -> Self {
        Self {
            population_size,
            ..Default::default()
        }
    }

    pub fn with_experiences(mut self, experiences: Vec<ExperienceSummary>) -> Self {
        self.experience_pool = experiences;
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Select the best plan from candidates.
    ///
    /// If enabled, generates mutated variants, evaluates fitness,
    /// and runs a mini tournament (Elo update). Returns the winner.
    /// If disabled, returns the original plan unchanged.
    pub fn select(&mut self, original: &TaskPlan) -> TaskPlan {
        if !self.enabled || self.population_size <= 1 {
            return original.clone();
        }

        let candidates = self.generate_candidates(original);

        // Score candidates: blend quick_fitness (70%) with Elo prior (30%)
        // This makes Elo a real selection signal, not just bookkeeping.
        let mut scored: Vec<_> = candidates.into_iter().map(|c| {
            let heuristic_fitness = self.quick_fitness(&c.plan);
            let elo = self.elo_ratings.get(&self.plan_signature(&c.plan)).copied().unwrap_or(1200.0);
            // Normalize Elo to [0,1] range: 1200 = 0.5, ±400 = ±0.1
            let elo_normalized = ((elo - 800.0) / 800.0).clamp(0.0, 1.0);
            let blended = heuristic_fitness * 0.7 + elo_normalized * 0.3;
            CandidatePlan {
                fitness: blended,
                elo_rating: elo,
                ..c
            }
        }).collect();

        scored.sort_by(|a, b| b.fitness.partial_cmp(&a.fitness).unwrap_or(std::cmp::Ordering::Equal));

        // Update Elo AFTER selection (for future loops)
        if scored.len() >= 2 {
            let (winner, loser) = if scored[0].fitness >= scored[1].fitness {
                (&scored[0], &scored[1])
            } else {
                (&scored[1], &scored[0])
            };

            let w_key = self.plan_signature(&winner.plan);
            let l_key = self.plan_signature(&loser.plan);

            let (wr, lr) = self.update_elo(
                self.elo_ratings.get(&w_key).copied().unwrap_or(1200.0),
                self.elo_ratings.get(&l_key).copied().unwrap_or(1200.0),
                1.0,
            );
            self.elo_ratings.insert(w_key, wr);
            self.elo_ratings.insert(l_key, lr);
        }

        scored[0].plan.clone()
    }

    pub fn generate_candidates(&self, original: &TaskPlan) -> Vec<CandidatePlan> {
        let mut candidates = vec![CandidatePlan {
            plan: original.clone(),
            fitness: 0.0,
            elo_rating: self.elo_ratings.get(&self.plan_signature(original)).copied().unwrap_or(1200.0),
            source: CandidateSource::Original,
            mutations_applied: Vec::new(),
        }];

        let num_variants = (self.population_size - 1).min(original.tasks.len().max(1));
        for i in 0..num_variants {
            let (mutated_plan, ops) = self.mutate_plan(original, i);
            candidates.push(CandidatePlan {
                plan: mutated_plan,
                fitness: 0.0,
                elo_rating: 1200.0,
                source: CandidateSource::Mutated,
                mutations_applied: ops,
            });
        }

        candidates
    }

    // ── Mutation ───────────────────────────────────────────────

    pub fn mutate_plan(&self, plan: &TaskPlan, seed: usize) -> (TaskPlan, Vec<MutationOp>) {
        use rand::seq::SliceRandom;
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut ops = Vec::new();

        if plan.tasks.is_empty() {
            return (plan.clone(), ops);
        }

        // Guarantee at least 1 mutation index — fix for the "zero-mutation variant" bug
        let num_mutations = (plan.tasks.len() as f64 * self.mutation_rate).ceil().max(1.0) as usize;
        let all_indices: Vec<usize> = (0..plan.tasks.len()).collect();
        let indices: Vec<usize> = all_indices.choose_multiple(&mut rng, num_mutations.min(all_indices.len())).copied().collect();

        let mut mutated = plan.clone();

        for (i, &idx) in indices.iter().enumerate() {
            let task_id = mutated.tasks[idx].id.clone();
            let mtype = (seed + i) % 5; // 5 mutation types now (was 3)

            match mtype {
                0 => {
                    // SwapRole: only swap to roles that have tools relevant to the task description
                    let desc_lower = mutated.tasks[idx].description.to_lowercase();
                    let candidate_roles: Vec<&str> = if desc_lower.contains("search") || desc_lower.contains("research") || desc_lower.contains("find") {
                        vec!["researcher", "analyst"]
                    } else if desc_lower.contains("write") || desc_lower.contains("report") || desc_lower.contains("document") {
                        vec!["writer", "synthesizer"]
                    } else if desc_lower.contains("code") || desc_lower.contains("implement") || desc_lower.contains("script") {
                        vec!["executor", "analyst"]
                    } else {
                        vec!["executor", "researcher", "writer"]
                    };
                    let current = &mutated.tasks[idx].assigned_role;
                    let candidates: Vec<String> = candidate_roles.iter()
                        .filter(|r| *r != current)
                        .map(|s| s.to_string())
                        .collect();
                    let new_role = candidates.choose(&mut rng)
                        .cloned()
                        .unwrap_or_else(|| "executor".into());
                    mutated.tasks[idx].assigned_role = new_role.clone();
                    ops.push(MutationOp::SwapRole { task_id, new_role });
                }
                1 => {
                    if !self.experience_pool.is_empty() {
                        let exp = &self.experience_pool[rng.r#gen::<usize>() % self.experience_pool.len()];
                        // Unique ID: task_id + loop seed + index — prevents dispatch HashMap collision
                        let new_id = format!("{}_injected_{}_{}", task_id, seed, i);
                        let new_task = TaskUnit {
                            id: new_id,
                            description: format!("[Injected] {}", exp.description),
                            assigned_role: "researcher".into(),
                            depends_on: vec![task_id.clone()],
                            expected_output: exp.lessons.first().cloned().unwrap_or_else(|| "Injected subtask output".into()),
                            difficulty: "medium".into(),
                            failed: false,
                            error: None,
                            output: None,
                        };
                        mutated.tasks.push(new_task);
                        ops.push(MutationOp::InjectFromExperience {
                            experience_description: exp.description.clone(),
                            suggested_role: "researcher".into(),
                        });
                    }
                }
                2 => {
                    // AddDependency: pick a RANDOM earlier task (not just idx-1) to avoid self-defeating chains
                    if idx > 0 {
                        let candidates: Vec<usize> = (0..idx).collect();
                        if let Some(&dep_idx) = candidates.choose(&mut rng) {
                            let dep_id = mutated.tasks[dep_idx].id.clone();
                            if !mutated.tasks[idx].depends_on.contains(&dep_id) {
                                mutated.tasks[idx].depends_on.push(dep_id.clone());
                                ops.push(MutationOp::AddDependency {
                                    task_id: task_id.clone(),
                                    depends_on: vec![dep_id],
                                });
                            }
                        }
                    }
                }
                3 => {
                    // RemoveDependency: reduce serial bottlenecks. If the task
                    // has dependencies, remove one to increase parallelism.
                    // P2 #1: previously a dead variant, now implemented.
                    if !mutated.tasks[idx].depends_on.is_empty() {
                        let dep_pos = rng.r#gen::<usize>() % mutated.tasks[idx].depends_on.len();
                        let removed = mutated.tasks[idx].depends_on.remove(dep_pos);
                        ops.push(MutationOp::RemoveDependency {
                            task_id: task_id.clone(),
                            remove_dep: removed,
                        });
                    } else {
                        // Fallback: SwapRole if no deps to remove
                        mutated.tasks[idx].assigned_role = "researcher".into();
                        ops.push(MutationOp::SwapRole { task_id, new_role: "researcher".into() });
                    }
                }
                4 | _ => {
                    // SplitTask: decompose a complex task into two sequential
                    // sub-tasks. This increases granularity for tasks whose
                    // description suggests multi-step work.
                    // P2 #1: previously a dead variant, now implemented.
                    let desc = mutated.tasks[idx].description.clone();
                    if desc.len() > 40 {
                        let mid = desc.len() / 2;
                        // Find nearest word boundary
                        let split_at = desc[mid..].find(' ').map(|p| mid + p).unwrap_or(mid);
                        let (part_a, part_b) = desc.split_at(split_at);
                        let part_a = part_a.trim().to_string();
                        let part_b = part_b.trim().to_string();
                        let new_b_id = format!("{}_split_{}_{}", task_id, seed, i);
                        let original_role = mutated.tasks[idx].assigned_role.clone();
                        // Modify the original task to be part A
                        mutated.tasks[idx].description = part_a.clone();
                        mutated.tasks[idx].expected_output = format!("Intermediate result for: {}", part_b);
                        // Insert part B after the original, depending on it
                        let new_task_b = TaskUnit {
                            id: new_b_id.clone(),
                            description: part_b.clone(),
                            assigned_role: original_role.clone(),
                            depends_on: vec![task_id.clone()],
                            expected_output: "Final result of split task".into(),
                            difficulty: mutated.tasks[idx].difficulty.clone(),
                            failed: false,
                            error: None,
                            output: None,
                        };
                        mutated.tasks.push(new_task_b);
                        ops.push(MutationOp::SplitTask {
                            task_id,
                            new_task_a: part_a,
                            new_task_b: part_b,
                        });
                    } else {
                        // Task too short to split — fall back to SwapRole
                        mutated.tasks[idx].assigned_role = "executor".into();
                        ops.push(MutationOp::SwapRole { task_id, new_role: "executor".into() });
                    }
                }
            }
        }

        (mutated, ops)
    }

    // ── Fitness ────────────────────────────────────────────────

    pub fn quick_fitness(&self, plan: &TaskPlan) -> f64 {
        let n = plan.tasks.len() as f64;
        if n == 0.0 {
            return 0.0;
        }

        // Count score: 3-8 tasks optimal
        let count_score = if (3.0..=8.0).contains(&n) {
            1.0
        } else if n < 3.0 {
            0.5
        } else {
            (8.0 / n).min(1.0)
        };

        // Structural efficiency: ratio of parallelism to total work.
        let max_depth = self.critical_path_depth(plan);
        let para_score = if n > 0.0 {
            1.0 - ((max_depth - 1) as f64 / n).max(0.0) * 0.7
        } else {
            0.5
        };

        // Role diversity
        let mut roles = std::collections::HashSet::new();
        for t in &plan.tasks {
            roles.insert(t.assigned_role.clone());
        }
        let div_score = (roles.len() as f64 / 6.0).min(1.0);

        // P1 #3 fix: outcome-aware component. The old fitness was purely
        // structural — a plan with beautiful structure but consistently
        // failing tasks scored the same as a proven winner. Now we look up
        // this plan signature's past Elo rating (which is updated by real
        // match results) and fold it in as a 4th term. Plans that have won
        // tournaments get a structural bonus; unproven plans stay neutral.
        let sig = self.plan_signature(plan);
        let elo = self.elo_ratings.get(&sig).copied().unwrap_or(1200.0);
        // Normalize: 1200 (baseline) → 0.5, 2000 (strong) → 1.0, 800 (weak) → 0.0
        let outcome_score = ((elo - 800.0) / 1200.0).clamp(0.0, 1.0);

        // Rebalanced: structural 75% (count 20% + para 30% + diversity 25%),
        // outcome 25%. Structural signals still dominate for novel plans
        // (outcome_score=0.5 for unproven), but proven plans rise above
        // structurally-similar unproven competitors.
        count_score * 0.20 + para_score * 0.30 + div_score * 0.25 + outcome_score * 0.25
    }

    /// Compute the critical path depth (longest dependency chain) via DFS.
    /// A plan with all-independent tasks has depth 1.
    /// A fully sequential chain of n tasks has depth n.
    fn critical_path_depth(&self, plan: &TaskPlan) -> usize {
        use std::collections::HashMap;
        let task_map: HashMap<&str, &TaskUnit> = plan.tasks.iter()
            .map(|t| (t.id.as_str(), t))
            .collect();

        let mut memo: HashMap<String, usize> = HashMap::new();

        fn dfs(
            id: &str,
            task_map: &HashMap<&str, &TaskUnit>,
            memo: &mut HashMap<String, usize>,
        ) -> usize {
            if let Some(&d) = memo.get(id) {
                return d;
            }
            let task = match task_map.get(id) {
                Some(t) => *t,
                None => return 1,
            };
            if task.depends_on.is_empty() {
                memo.insert(id.to_string(), 1);
                return 1;
            }
            let max_dep = task.depends_on.iter()
                .map(|dep| dfs(dep, task_map, memo))
                .max()
                .unwrap_or(0);
            let depth = max_dep + 1;
            memo.insert(id.to_string(), depth);
            depth
        }

        plan.tasks.iter()
            .map(|t| dfs(&t.id, &task_map, &mut memo))
            .max()
            .unwrap_or(1)
    }

    // ── Elo ────────────────────────────────────────────────────

    pub fn update_elo(&self, w: f64, l: f64, w_score: f64) -> (f64, f64) {
        let k = self.elo_k_factor;
        let ew = 1.0 / (1.0 + 10.0_f64.powf((l - w) / 400.0));
        let el = 1.0 - ew;
        (w + k * (w_score - ew), l + k * ((1.0 - w_score) - el))
    }

    /// Produce a signature that uniquely identifies a plan's *structure*,
    /// not just its role multiset.
    ///
    /// P1 #2 fix: the old signature was just `sorted(roles)`, so two plans with
    /// identical roles but completely different task counts, dependency graphs,
    /// and descriptions shared the same Elo rating and elite entry. The new
    /// signature incorporates task count, per-task dependency depth, and the
    /// role→role dependency edges so structurally distinct plans are
    /// distinguished.
    pub fn plan_signature(&self, plan: &TaskPlan) -> String {
        let n = plan.tasks.len();
        // Depth of each task in the DAG (0 = no deps, 1 = depends on wave-0, ...)
        let id_to_role: std::collections::HashMap<&str, &str> = plan.tasks.iter()
            .map(|t| (t.id.as_str(), t.assigned_role.as_str()))
            .collect();
        let mut depth_parts: Vec<String> = Vec::with_capacity(n);
        for t in &plan.tasks {
            let depth = dag_depth(&plan.tasks, &t.id, &id_to_role);
            depth_parts.push(format!("{}@{}", t.assigned_role, depth));
        }
        depth_parts.sort();
        format!("n={}|{}", n, depth_parts.join(","))
    }
}

/// Recursively compute the depth of a task in the dependency DAG.
/// Tasks with no `depends_on` have depth 0; depth = 1 + max(depth of deps).
/// Cycles are clamped to avoid infinite recursion (returns 0).
fn dag_depth(
    tasks: &[miniagent_core::TaskUnit],
    task_id: &str,
    _id_to_role: &std::collections::HashMap<&str, &str>,
) -> usize {
    fn helper(
        tasks: &[miniagent_core::TaskUnit],
        id: &str,
        cache: &mut std::collections::HashMap<String, usize>,
    ) -> usize {
        if let Some(&d) = cache.get(id) {
            return d;
        }
        // Guard against cycles by tentatively marking depth 0
        cache.insert(id.to_string(), 0);
        let Some(task) = tasks.iter().find(|t| t.id == id) else {
            return 0;
        };
        let max_dep = task.depends_on.iter()
            .map(|dep| helper(tasks, dep, cache))
            .max()
            .unwrap_or(0);
        let depth = max_dep + 1;
        cache.insert(id.to_string(), depth);
        depth
    }
    let mut cache = std::collections::HashMap::new();
    helper(tasks, task_id, &mut cache).saturating_sub(1)
}
