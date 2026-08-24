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

    pub fn node_count(&self) -> usize { self.nodes.len() }
    pub fn edge_count(&self) -> usize { self.edges.len() }

}

impl Default for ExperienceGraph {
    fn default() -> Self { Self::new() }
}

