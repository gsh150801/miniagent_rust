use serde::{Deserialize, Serialize};

use crate::message::Message;
use crate::types::{CheckpointId, ProjectId, RunId, StepId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub run_id: RunId,
    pub project_id: Option<ProjectId>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub step_id: StepId,
    pub iteration: usize,
    pub history: Vec<Message>,
    pub progress: Option<serde_json::Value>,
}

impl Checkpoint {
    pub fn new(
        run_id: RunId,
        step_id: StepId,
        iteration: usize,
        history: Vec<Message>,
    ) -> Self {
        Self {
            id: CheckpointId::new(),
            run_id,
            project_id: None,
            timestamp: chrono::Utc::now(),
            step_id,
            iteration,
            history,
            progress: None,
        }
    }

    pub fn with_project(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self
    }

    pub fn with_progress(mut self, progress: serde_json::Value) -> Self {
        self.progress = Some(progress);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummary {
    pub id: CheckpointId,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub step_id: StepId,
    pub iteration: usize,
    pub message_count: usize,
}
