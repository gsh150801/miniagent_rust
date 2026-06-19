use std::sync::{Arc, Mutex};

use miniagent_self_improve::offline::experience_graph::ExperienceGraph;
use miniagent_self_improve::online::q_router::{QLearningRouter, RouterState, TaskType};
use miniagent_provider::router::ProviderChoice;
use serde::{Deserialize, Serialize};

use crate::cold_start_kb::{ColdStartKnowledgeBase, DomainTemplate};
use std::future::Future;
use std::pin::Pin;

use crate::MemoryRetriever;

// ── Public Types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceSummary {
    pub description: String,
    pub lessons: Vec<String>,
    pub node_type: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrievalContext {
    pub relevant_successes: Vec<ExperienceSummary>,
    pub pitfalls: Vec<ExperienceSummary>,
    pub confidence: f64,
}

// ── Memory Router ──────────────────────────────────────────────

pub struct MemoryRouter {
    /// Shared mutable experience graph — same instance across retrieve() and record().
    experience_graph: Arc<Mutex<ExperienceGraph>>,
    q_router: Arc<Mutex<QLearningRouter>>,
    cold_start_kb: Arc<ColdStartKnowledgeBase>,
    rrf_alpha: f64,
    top_k: usize,
}

impl MemoryRouter {
    pub fn new(
        experience_graph: Arc<Mutex<ExperienceGraph>>,
        q_router: Arc<Mutex<QLearningRouter>>,
        cold_start_kb: Arc<ColdStartKnowledgeBase>,
    ) -> Self {
        Self {
            experience_graph,
            q_router,
            cold_start_kb,
            rrf_alpha: 0.5,
            top_k: 5,
        }
    }

    /// Create with default components. The ExperienceGraph is shared
    /// between retrieve() and record(), so experiences accumulate.
    pub fn defaults() -> Self {
        Self::new(
            Arc::new(Mutex::new(ExperienceGraph::new())),
            Arc::new(Mutex::new(QLearningRouter::new())),
            Arc::new(ColdStartKnowledgeBase::with_defaults()),
        )
    }

    /// Expose the shared graph for external construction (e.g., SelfImprover).
    pub fn shared_graph(&self) -> Arc<Mutex<ExperienceGraph>> {
        Arc::clone(&self.experience_graph)
    }

    // ── retrieve() ─────────────────────────────────────────────

    pub fn retrieve(&self, task_description: &str) -> RetrievalContext {
        let domain_template = self.cold_start_kb.match_task(task_description).cloned();
        let signature = self.text_signature(task_description);

        // Vector retrieval — lock graph, clone results inside lock
        let vector_results: Vec<ExperienceSummary> = {
            let graph = self.experience_graph.lock().unwrap_or_else(|e| e.into_inner());
            graph.find_similar(&signature, 0.3, self.top_k)
                .into_iter()
                .map(|n| ExperienceSummary {
                    description: n.description.clone(),
                    lessons: n.lessons.clone(),
                    node_type: format!("{:?}", n.node_type).to_lowercase(),
                    confidence: n.confidence,
                })
                .collect()
        };

        // Lexical retrieval — scan graph descriptions for keyword overlap
        let lexical_results = self.lexical_search(task_description);

        // RRF fusion
        let fused = self.reciprocal_rank_fusion(vector_results, lexical_results);

        let (successes, pitfalls): (Vec<ExperienceSummary>, Vec<ExperienceSummary>) = fused
            .into_iter()
            .partition(|s| s.node_type == "successpattern" || s.node_type == "success");

        let confidence = self.compute_confidence(&successes, &pitfalls, &domain_template);

        RetrievalContext {
            relevant_successes: successes.into_iter().take(3).collect(),
            pitfalls: pitfalls.into_iter().take(3).collect(),
            confidence,
        }
    }

    // ── record() — NOW ACTUALLY WRITES TO THE GRAPH ────────────

    pub fn record(&self, task_description: &str, success: bool, quality_score: f64) {
        let signature = self.text_signature(task_description);
        let node_type = if success && quality_score > 0.7 {
            miniagent_self_improve::offline::experience_graph::NodeType::SuccessPattern
        } else if !success {
            miniagent_self_improve::offline::experience_graph::NodeType::FailurePattern
        } else {
            miniagent_self_improve::offline::experience_graph::NodeType::EdgeCase
        };

        let description = if success {
            format!("Completed: {} (quality={:.2})", task_description, quality_score)
        } else {
            format!("Failed: {} (quality={:.2})", task_description, quality_score)
        };

        let lessons = if success && quality_score > 0.8 {
            vec!["High quality outcome".to_string(), "Maintain approach".to_string()]
        } else if !success {
            vec!["Review error patterns".to_string(), "Consider alternative approach".to_string()]
        } else {
            vec!["Partial success".to_string()]
        };

        // ACTUALLY WRITE to the shared ExperienceGraph
        {
            let mut graph = self.experience_graph.lock().unwrap_or_else(|e| e.into_inner());
            let node_id = graph.add_experience(
                node_type,
                &description,
                &lessons,
                &signature,
            );
            tracing::info!(
                node_id = %node_id,
                desc = %description,
                graph_size = graph.node_count(),
                "MemoryRouter: recorded experience to graph"
            );
        }

        // Update Q-table with reward signal.
        // P1 #6 fix: the old code used a hardcoded RouterState (complexity:128,
        // memory:true, budget:50) for every task, so all Q-updates collapsed
        // onto a single state cell — the router learned nothing. Now the state
        // is derived from the task signature so different task families land on
        // different cells.
        {
            let mut router = self.q_router.lock().unwrap_or_else(|e| e.into_inner());
            let task_type = self.classify_task_type(task_description);
            // Derive a coarse complexity bucket from the signature so the Q-table
            // can learn different strategies for short vs long, code vs research tasks.
            let sig = &signature;  // already computed at the top of record()
            let word_density = if sig.len() >= 2 { sig[0] } else { 0.5 };     // normalized word count
            let has_code = sig.get(2).copied().unwrap_or(0.0) > 0.5;
            // Map to discrete levels RouterState expects: 64 (simple), 128 (moderate), 255 (hard)
            let complexity_level = if word_density < 0.3 { 64 }
                else if word_density < 0.7 || has_code { 128 }
                else { 255 };
            // Budget bucket from quality: failed tasks get a lower budget tier
            // so the router can learn that cheap models suffice for simple tasks.
            let budget_percent = if quality_score >= 0.8 { 75 }
                else if quality_score >= 0.4 { 50 }
                else { 25 };

            let state = RouterState {
                task_type,
                complexity_level,
                memory_available: !self.cold_start_kb.entries.is_empty(),
                budget_percent,
            };
            let reward = quality_score * 0.7 + if success { 0.3 } else { 0.0 };

            // Decide which action was taken (the model tier we would have used)
            let decision = router.decide(&state);
            // Update Q-value: state → action → reward → next_state (same state, single-step)
            router.update(&state, decision.model, reward, &state);
            router.decay_exploration();

            tracing::debug!(
                reward = reward,
                complexity = complexity_level,
                budget = budget_percent,
                total_steps = router.total_steps(),
                epsilon = router.current_epsilon(),
                "MemoryRouter: Q-table updated"
            );
        }
    }

    // ── text_signature (5-D, case-insensitive) ─────────────────

    pub fn text_signature(&self, text: &str) -> Vec<f64> {
        let text_lower = text.to_lowercase();
        let words: Vec<&str> = text_lower.split_whitespace().collect();
        let word_count = words.len() as f64;
        let avg_word_len = if word_count > 0.0 {
            words.iter().map(|w| w.len() as f64).sum::<f64>() / word_count
        } else {
            0.0
        };

        // P2 #16 fix: convert binary flags to continuous density scores.
        // Old: `has_code = text.contains("code")` → binary 0/1, so "write
        // one function" and "refactor 50 code files" mapped to the same point.
        // New: count keyword hits and normalize by word count → density.
        let count_hits = |keywords: &[&str]| -> f64 {
            let hits = keywords.iter()
                .filter(|kw| text_lower.contains(*kw))
                .count() as f64;
            // Density: hits / keyword count, then boosted by word count ratio.
            // A 100-word task with 3 code keywords scores higher than a 5-word
            // task with the same 3 keywords — the signal is stronger.
            let base = hits / keywords.len().max(1) as f64;
            let boost = (word_count / 30.0).min(1.5);
            (base * boost).min(1.0)
        };

        let code_density = count_hits(&["code", "implement", "script", "function",
            "class", "debug", "compile", "refactor", "test", "algorithm"]);
        let research_density = count_hits(&["research", "find", "analyze", "search",
            "investigate", "study", "survey", "literature", "compare", "evaluate"]);
        let write_density = count_hits(&["write", "report", "summarize", "document",
            "draft", "article", "blog", "essay", "outline", "edit"]);

        // Language tokens — prevents cross-language signature collisions.
        // "Write Python function" vs "Implement Rust module" now differ.
        let lang_python = text_lower.contains("python") || text_lower.contains(".py")
            || text_lower.contains("pandas") || text_lower.contains("numpy")
            || text_lower.contains("django") || text_lower.contains("flask");
        let lang_rust = text_lower.contains("rust") || text_lower.contains(".rs")
            || text_lower.contains("cargo") || text_lower.contains("tokio")
            || text_lower.contains("serde") || text_lower.contains("trait");
        let lang_js = text_lower.contains("javascript") || text_lower.contains("typescript")
            || text_lower.contains(".js") || text_lower.contains(".ts")
            || text_lower.contains("node") || text_lower.contains("react");
        let lang_shell = text_lower.contains("bash") || text_lower.contains("shell")
            || text_lower.contains(".sh") || text_lower.contains("awk");

        // Domain tokens
        let domain_web = text_lower.contains("web") || text_lower.contains("api")
            || text_lower.contains("http") || text_lower.contains("url")
            || text_lower.contains("rest") || text_lower.contains("graphql");
        let domain_data = text_lower.contains("data") || text_lower.contains("csv")
            || text_lower.contains("json") || text_lower.contains("database")
            || text_lower.contains("sql") || text_lower.contains("chart");

        vec![
            (word_count / 50.0).min(1.0),
            (avg_word_len / 10.0).min(1.0),
            code_density,
            research_density,
            write_density,
            if lang_python { 1.0 } else { 0.0 },
            if lang_rust { 1.0 } else { 0.0 },
            if lang_js { 1.0 } else { 0.0 },
            if lang_shell { 1.0 } else { 0.0 },
            if domain_web { 1.0 } else { 0.0 },
            if domain_data { 1.0 } else { 0.0 },
        ]
    }

    // ── lexical_search — NOW IMPLEMENTED ───────────────────────

    fn lexical_search(&self, query: &str) -> Vec<ExperienceSummary> {
        let query_lower = query.to_lowercase();
        let keywords: Vec<&str> = query_lower
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();

        if keywords.is_empty() {
            return Vec::new();
        }

        let graph = self.experience_graph.lock().unwrap_or_else(|e| e.into_inner());
        let mut scored: Vec<(usize, ExperienceSummary)> = Vec::new();

        for node in graph.all_nodes() {
            let desc_lower = node.description.to_lowercase();
            let overlap = keywords.iter()
                .filter(|kw| desc_lower.contains(*kw))
                .count();

            if overlap > 0 {
                scored.push((overlap, ExperienceSummary {
                    description: node.description.clone(),
                    lessons: node.lessons.clone(),
                    node_type: format!("{:?}", node.node_type).to_lowercase(),
                    confidence: node.confidence,
                }));
            }
        }

        // Sort by keyword overlap descending, take top_k
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(self.top_k).map(|(_, s)| s).collect()
    }

    // ── RRF Fusion ─────────────────────────────────────────────

    fn reciprocal_rank_fusion(
        &self,
        vector_results: Vec<ExperienceSummary>,
        lexical_results: Vec<ExperienceSummary>,
    ) -> Vec<ExperienceSummary> {
        let k: f64 = 60.0;
        let alpha = self.rrf_alpha;

        let mut scores: std::collections::HashMap<String, (f64, ExperienceSummary)> =
            std::collections::HashMap::new();

        for (rank, item) in vector_results.iter().enumerate() {
            let key = format!("{}_{}", item.node_type, item.description);
            let score = (1.0 - alpha) * 1.0 / (k + (rank + 1) as f64);
            scores.entry(key).and_modify(|(s, _)| *s += score).or_insert((score, item.clone()));
        }

        for (rank, item) in lexical_results.iter().enumerate() {
            let key = format!("{}_{}", item.node_type, item.description);
            let score = alpha * 1.0 / (k + (rank + 1) as f64);
            scores.entry(key).and_modify(|(s, _)| *s += score).or_insert((score, item.clone()));
        }

        let mut scored: Vec<_> = scores.into_values().collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(_, item)| item).collect()
    }

    // ── Helpers ────────────────────────────────────────────────

    fn classify_task_type(&self, text: &str) -> TaskType {
        let lower = text.to_lowercase();
        if lower.contains("code") || lower.contains("implement") || lower.contains("function") {
            TaskType::CodeGeneration
        } else if lower.contains("research") || lower.contains("find") || lower.contains("search") {
            TaskType::Research
        } else if lower.contains("summarize") || lower.contains("report") {
            TaskType::Summarization
        } else if lower.contains("analyze") || lower.contains("data") {
            TaskType::Analysis
        } else {
            TaskType::Qa
        }
    }

    fn compute_confidence(
        &self,
        successes: &[ExperienceSummary],
        pitfalls: &[ExperienceSummary],
        template: &Option<DomainTemplate>,
    ) -> f64 {
        let mut score: f64 = 0.0;
        if !successes.is_empty() { score += 0.4; }
        if !pitfalls.is_empty() { score += 0.2; }
        if template.is_some() { score += 0.3; }
        if successes.len() >= 3 { score += 0.1; }
        score.min(1.0)
    }
}

// ── MemoryRetriever impl ───────────────────────────────────────

impl MemoryRetriever for MemoryRouter {
    fn retrieve<'a>(&'a self, task: &'a str) -> Pin<Box<dyn Future<Output = RetrievalContext> + Send + 'a>> {
        Box::pin(async move { self.retrieve(task) })
    }
    fn record(&self, task: &str, success: bool, quality_score: f64) {
        self.record(task, success, quality_score);
    }
}
