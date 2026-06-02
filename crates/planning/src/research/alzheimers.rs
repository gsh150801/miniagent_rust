use serde::{Deserialize, Serialize};

use crate::state_graph::{StateGraph, CompiledGraph, GraphState, ModelTier};

/// Configuration for the Alzheimer's disease hypothesis pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlzheimersConfig {
    pub max_tournament_rounds: usize,
    pub convergence_threshold: f64,
    pub convergence_window: usize,
    pub hypotheses_per_mechanism: usize,
    pub debate_rounds_per_match: usize,
    pub model_tier: ModelTier,
}

impl Default for AlzheimersConfig {
    fn default() -> Self {
        Self {
            max_tournament_rounds: 5,
            convergence_threshold: 15.0,
            convergence_window: 3,
            hypotheses_per_mechanism: 2,
            debate_rounds_per_match: 3,
            model_tier: ModelTier::Pro,
        }
    }
}

/// Build the Alzheimer's disease hypothesis tournament pipeline as a StateGraph.
///
/// Pipeline phases:
/// 1. Literature Search → Researcher + tools
/// 2. KG Construction → Evidence extraction + merge
/// 3. Hypothesis Seeding → Dynamic Proposer agents per mechanism (parallel)
/// 4. Tournament Debate → Pairwise Elo debates
/// 5. Hypothesis Evolution → Mutator agents revise REVISE'd hypotheses
/// 6. Gap Analysis → Evidence Accumulator identifies weak spots
/// 7. Synthesis → Synthesis Judge produces final integrated hypothesis
pub fn build_alzheimers_pipeline(config: &AlzheimersConfig) -> CompiledGraph {
    let profiles = super::profiles::default_profiles();

    let graph = StateGraph::new("literature_search");

    // Phase 1: Literature Search
    let graph = graph.add_agent(
        "literature_search",
        "You are a research scientist specializing in Alzheimer's disease. \
         Search PubMed and the web for the latest research on AD pathogenesis mechanisms. \
         Focus on: amyloid cascade, tau propagation, neuroinflammation, vascular dysfunction, \
         mitochondrial dysfunction, and synaptic failure. \
         Collect PMIDs, key findings, and methodological details.",
        ModelTier::Flash,
    );

    // Phase 2: KG Construction
    let graph = graph.add_agent(
        "kg_construction",
        "You are a knowledge graph specialist. Take the research findings from literature search \
         and extract structured entities (genes, proteins, pathways, diseases) and their relationships. \
         Output in JSON format suitable for knowledge graph ingestion.",
        ModelTier::Flash,
    );

    // Phase 3: Hypothesis Seeding — one node per mechanism profile
    let mut graph = graph;
    let mut seeding_node_names = Vec::new();
    for profile in &profiles {
        let node_name = format!("seed_{}", profile.id);
        let prompt = format!(
            "You are a hypothesis proposer for Alzheimer's disease research.\n\n\
             **Mechanism Focus:** {}\n\n\
             **Key Genes:** {:?}\n\
             **Key Pathways:** {:?}\n\n\
             **Supporting Evidence:**\n- {}\n\n\
             **Opposing Evidence:**\n- {}\n\n\
             **Task:** {}\n\n\
             Generate a specific, falsifiable hypothesis. Output JSON with: \
             hypothesis, mechanism, evidence (with sources), confidence, testable_prediction.",
            profile.mechanism,
            profile.key_genes,
            profile.key_pathways,
            profile.supporting_evidence.join("\n- "),
            profile.opposing_evidence.join("\n- "),
            profile.seed_prompt,
        );
        graph = graph.add_agent(&node_name, &prompt, config.model_tier);
        seeding_node_names.push(node_name);
    }

    // Phase 4: Tournament Debate
    let graph = graph.add_agent(
        "tournament_debate",
        "You are the Tournament Master for Alzheimer's disease hypothesis evaluation. \
         Manage pairwise debates between hypotheses. Score each on: \
         evidence_support (0.30), mechanistic_plausibility (0.25), falsifiability (0.20), \
         novelty (0.15), consistency (0.10). Update Elo ratings after each debate. \
         Check for convergence using Nash equilibrium detection.",
        ModelTier::Pro,
    );

    // Phase 5: Hypothesis Evolution (for REVISE'd hypotheses)
    let graph = graph.add_agent(
        "hypothesis_evolution",
        "You are a hypothesis evolution specialist for AD research. \
         Take hypotheses that received REVISE verdicts in the tournament \
         and the specific critiques. Produce improved variants that address \
         the weaknesses identified. Maintain the core insight while strengthening \
         evidence and falsifiability.",
        ModelTier::Pro,
    );

    // Phase 6: Gap Analysis
    let graph = graph.add_agent(
        "gap_analysis",
        "You are an evidence gap analyst for AD research. \
         Review all hypotheses and their tournament performance. \
         Identify: missing evidence, under-supported claims, contradictory findings, \
         and methodological gaps. Prioritize gaps by their impact on hypothesis strength.",
        ModelTier::Flash,
    );

    // Phase 7: Synthesis
    let graph = graph.add_agent(
        "synthesis",
        "You are the Synthesis Judge for an Alzheimer's disease hypothesis tournament. \
         Integrate the top-rated hypotheses into a unified multi-mechanism framework. \
         Identify synergies between mechanisms (e.g., how amyloid drives inflammation \
         which accelerates tau spread). Resolve contradictions with evidence-based reasoning. \
         Produce testable predictions that distinguish the integrated hypothesis from alternatives.",
        ModelTier::Pro,
    );

    // Wire edges: sequential phases with parallel seeding
    let mut graph = graph.add_edge("literature_search", "kg_construction");

    // kg_construction → all seed nodes (fan-out)
    for node_name in &seeding_node_names {
        graph = graph.add_edge("kg_construction", node_name);
    }

    // All seed nodes → tournament (fan-in)
    for node_name in &seeding_node_names {
        graph = graph.add_edge(node_name, "tournament_debate");
    }

    // Conditional: tournament → evolution (continue) or gap_analysis (converged)
    // Convention: "converged" key in step_outputs triggers gap_analysis route
    let graph = graph.add_conditional(
        "tournament_debate",
        vec![
            ("hypothesis_evolution".into(), Box::new(|state: &GraphState| {
                // Continue if tournament says "continue_tournament"
                state.step_outputs.get("tournament_debate")
                    .map(|s| s.contains("continue_tournament"))
                    .unwrap_or(false)
            }) as crate::state_graph::EdgePredicate),
            ("gap_analysis".into(), Box::new(|state: &GraphState| {
                // Go to gap analysis if converged or default
                state.step_outputs.get("tournament_debate")
                    .map(|s| s.contains("converged") || s.contains("synthesis"))
                    .unwrap_or(true)
            }) as crate::state_graph::EdgePredicate),
        ],
        "gap_analysis", // default route
    );

    // Evolution feeds back into tournament
    let graph = graph.add_edge("hypothesis_evolution", "tournament_debate");
    let graph = graph.add_edge("gap_analysis", "synthesis");

    // Add checkpoint at synthesis node for recovery
    let graph = graph.with_checkpoint("synthesis");

    graph.compile().expect("AD pipeline graph compilation failed")
}

/// Generate the research topic prompt for the AD pipeline.
pub fn alzheimers_research_prompt() -> String {
    "Investigate the pathogenic mechanisms underlying Alzheimer's disease, \
     with a focus on the relative contributions and interactions between: \
     (1) amyloid-β accumulation and oligomer toxicity, \
     (2) tau hyperphosphorylation and prion-like propagation, \
     (3) neuroinflammation and microglial activation, \
     (4) vascular dysfunction and BBB breakdown, \
     (5) mitochondrial dysfunction and oxidative stress, \
     (6) synaptic failure and complement-mediated pruning. \
     The goal is to determine which mechanisms are causal vs. consequential, \
     identify key interaction pathways, and produce an integrated multi-mechanism \
     hypothesis with testable predictions.".into()
}
