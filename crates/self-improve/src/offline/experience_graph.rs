use serde::{Deserialize, Serialize};

/// Experience Graph: structured representation of success/failure patterns.
/// Inspired by EXG (Self-Evolving Agents with Experience Graphs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceGraph {
    nodes: Vec<ExperienceNode>,
    edges: Vec<ExperienceEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceNode {
    pub id: uuid::Uuid,
    pub node_type: NodeType,
    pub task_signature: Vec<f64>,  // feature vector for similarity matching
    pub description: String,
    pub lessons: Vec<String>,
    pub confidence: f64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    SuccessPattern,
    FailurePattern,
    EdgeCase,
    Insight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceEdge {
    pub from_id: uuid::Uuid,
    pub to_id: uuid::Uuid,
    pub edge_type: EdgeType,
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeType {
    CausedBy,
    SimilarTo,
    GeneralizesTo,
    PreventsFrom,
}

impl ExperienceGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_experience(
        &mut self,
        node_type: NodeType,
        description: &str,
        lessons: &[String],
        signature: &[f64],
    ) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        self.nodes.push(ExperienceNode {
            id,
            node_type,
            task_signature: signature.to_vec(),
            description: description.to_string(),
            lessons: lessons.to_vec(),
            confidence: 1.0,
            created_at: chrono::Utc::now(),
        });
        id
    }

    pub fn link(&mut self, from: uuid::Uuid, to: uuid::Uuid, edge_type: EdgeType, weight: f64) {
        self.edges.push(ExperienceEdge {
            from_id: from,
            to_id: to,
            edge_type,
            weight,
        });
    }

    /// Find similar experiences by task signature (cosine similarity)
    pub fn find_similar(&self, signature: &[f64], threshold: f64, max_results: usize) -> Vec<&ExperienceNode> {
        let mut scored: Vec<(f64, &ExperienceNode)> = self
            .nodes
            .iter()
            .map(|node| {
                let sim = cosine_similarity(signature, &node.task_signature);
                (sim, node)
            })
            .filter(|(sim, _)| *sim >= threshold)
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_results);
        scored.into_iter().map(|(_, node)| node).collect()
    }

    /// Query for patterns: find success patterns similar to this task
    pub fn query_success_patterns(&self, signature: &[f64]) -> Vec<&ExperienceNode> {
        self.find_similar(signature, 0.5, 5)
            .into_iter()
            .filter(|n| n.node_type == NodeType::SuccessPattern)
            .collect()
    }

    /// Query for pitfalls: find failure patterns similar to this task
    pub fn query_pitfalls(&self, signature: &[f64]) -> Vec<&ExperienceNode> {
        self.find_similar(signature, 0.5, 5)
            .into_iter()
            .filter(|n| n.node_type == NodeType::FailurePattern)
            .collect()
    }

    pub fn node_count(&self) -> usize { self.nodes.len() }
    pub fn edge_count(&self) -> usize { self.edges.len() }
}

impl Default for ExperienceGraph {
    fn default() -> Self { Self::new() }
}

fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
    dot / (norm_a * norm_b)
}
