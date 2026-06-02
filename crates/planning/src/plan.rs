use std::collections::{HashMap, VecDeque};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

// ── Plan Types ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: uuid::Uuid,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub estimated_steps: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: uuid::Uuid,
    pub index: usize,
    pub description: String,
    pub tool_hint: Option<String>,
    pub depends_on: Vec<uuid::Uuid>,
    pub status: StepStatus,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl Plan {
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            goal: goal.into(),
            steps: Vec::new(),
            created_at: chrono::Utc::now(),
            estimated_steps: 0,
        }
    }

    pub fn progress(&self) -> (usize, usize) {
        let done = self.steps.iter().filter(|s| s.status == StepStatus::Completed).count();
        (done, self.steps.len())
    }

    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
    }

    /// Topological order for execution (respects dependencies)
    pub fn execution_order(&self) -> Vec<Vec<usize>> {
        let mut in_degree: HashMap<usize, usize> = HashMap::new();
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();

        for (i, step) in self.steps.iter().enumerate() {
            in_degree.entry(i).or_insert(step.depends_on.len());
            for dep_id in &step.depends_on {
                if let Some(dep_idx) = self.steps.iter().position(|s| s.id == *dep_id) {
                    children.entry(dep_idx).or_default().push(i);
                }
            }
        }

        let mut waves = Vec::new();
        let mut queue: VecDeque<usize> = in_degree.iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&i, _)| i)
            .collect();

        while !queue.is_empty() {
            let wave: Vec<usize> = queue.drain(..).collect();
            let mut next = VecDeque::new();
            for &idx in &wave {
                if let Some(kids) = children.get(&idx) {
                    for &kid in kids {
                        if let Some(deg) = in_degree.get_mut(&kid) {
                            *deg -= 1;
                            if *deg == 0 { next.push_back(kid); }
                        }
                    }
                }
            }
            waves.push(wave);
            queue = next;
        }

        waves
    }

    /// Generate a status display
    pub fn status_display(&self) -> String {
        let mut out = format!("Plan: {}\n", self.goal);
        let (done, total) = self.progress();
        out.push_str(&format!("Progress: {done}/{total}\n\n"));

        let order = self.execution_order();
        for (wave_idx, wave) in order.iter().enumerate() {
            for &step_idx in wave {
                let step = &self.steps[step_idx];
                let icon = match step.status {
                    StepStatus::Completed => "✅",
                    StepStatus::Running => "🔄",
                    StepStatus::Failed => "❌",
                    StepStatus::Skipped => "⏭️",
                    StepStatus::Pending => "⏳",
                };
                let deps: Vec<String> = step.depends_on.iter()
                    .filter_map(|did| self.steps.iter().find(|s| s.id == *did).map(|s| s.index.to_string()))
                    .collect();
                let dep_str = if deps.is_empty() { String::new() } else { format!(" (after: {})", deps.join(",")) };
                out.push_str(&format!("  {icon} [{}/{}] {}{dep_str}\n", step.index + 1, wave_idx + 1, step.description));
            }
        }
        out
    }
}

// ── Planner — LLM-based task decomposition ───────────────────

pub struct Planner {
    provider: Box<dyn miniagent_provider::traits::LlmProvider>,
}

impl Planner {
    pub fn new(provider: Box<dyn miniagent_provider::traits::LlmProvider>) -> Self {
        Self { provider }
    }

    /// Decompose a complex task into a Plan with steps
    pub async fn decompose(
        &self,
        goal: &str,
        cancel: CancellationToken,
    ) -> Result<Plan, miniagent_core::error::AgentError> {
        let prompt = format!(
            r#"Decompose this task into sequential steps. Output a JSON plan.

Task: {goal}

Rules:
- Break into 3-10 concrete, executable steps
- Each step should produce a clear deliverable
- Add tool_hint for the tool best suited for each step (one of: pubmed_search, web_search, web_fetch, read, write, edit, bash, glob, grep)
- Add depends_on array (step indices) for steps that need prior results
- Steps are 0-indexed

Output ONLY valid JSON:
{{
  "steps": [
    {{
      "index": 0,
      "description": "Search PubMed for relevant papers",
      "tool_hint": "pubmed_search",
      "depends_on": []
    }},
    {{
      "index": 1,
      "description": "Fetch and read the top papers",
      "tool_hint": "web_fetch",
      "depends_on": [0]
    }}
  ]
}}"#
        );

        let request = miniagent_provider::traits::CompletionRequest {
            system: "You are a task planner. Output ONLY valid JSON, no commentary.".into(),
            messages: vec![miniagent_core::message::Message::user(&prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0), max_tokens: Some(2000), ..Default::default()
            },
        };

        let response = self.provider.complete(&request, cancel).await?;
        let text = response.content.iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("");

        let json_str = text.trim()
            .trim_start_matches("```json").trim_start_matches("```")
            .trim_end_matches("```");
        let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap_or_default();

        let mut plan = Plan::new(goal);
        if let Some(steps_arr) = parsed["steps"].as_array() {
            for step_val in steps_arr {
                let index = step_val["index"].as_u64().unwrap_or(plan.steps.len() as u64) as usize;
                let description = step_val["description"].as_str().unwrap_or("").to_string();
                let tool_hint = step_val["tool_hint"].as_str().map(|s| s.to_string());
                let depends_on: Vec<uuid::Uuid> = step_val["depends_on"].as_array()
                    .map(|a| a.iter()
                        .filter_map(|v| v.as_u64())
                        .filter_map(|i| plan.steps.get(i as usize).map(|s| s.id))
                        .collect())
                    .unwrap_or_default();

                plan.steps.push(PlanStep {
                    id: uuid::Uuid::new_v4(),
                    index,
                    description,
                    tool_hint,
                    depends_on,
                    status: StepStatus::Pending,
                    output: None,
                    error: None,
                });
            }
        }

        plan.estimated_steps = plan.steps.len();
        Ok(plan)
    }
}

// ── PlanExecutor — executes a Plan via the Agent ──────────────

pub struct PlanExecutor {
    agent: std::sync::Arc<miniagent_agent::Agent>,
}

impl PlanExecutor {
    pub fn new(agent: std::sync::Arc<miniagent_agent::Agent>) -> Self {
        Self { agent }
    }

    /// Execute the plan and return updated plan with outputs filled in
    pub async fn execute(
        &self,
        plan: &mut Plan,
        cancel: CancellationToken,
    ) -> Result<(), miniagent_core::error::AgentError> {
        let waves = plan.execution_order();

        for (wave_idx, wave) in waves.iter().enumerate() {
            // Steps within a wave can run in parallel
            let mut tasks = Vec::new();

            for &step_idx in wave {
                let step = &plan.steps[step_idx];

                // Gather outputs from dependencies
                let dep_outputs: Vec<String> = step.depends_on.iter()
                    .filter_map(|did| {
                        plan.steps.iter()
                            .find(|s| s.id == *did)
                            .and_then(|s| s.output.clone())
                    })
                    .collect();

                let context = if dep_outputs.is_empty() {
                    step.description.clone()
                } else {
                    format!("{}\n\nContext from previous steps:\n{}",
                        step.description,
                        dep_outputs.join("\n\n"))
                };

                let agent = self.agent.clone();
                let cancel = cancel.child_token();

                tasks.push(tokio::spawn(async move {
                    let mut history = Vec::new();
                    history.push(miniagent_core::message::Message::user(&context));

                    let ctx = miniagent_agent::context::RunContext::new(
                        "Execute this step precisely. Use available tools."
                    ).with_complexity(miniagent_core::config::TaskComplexity::Moderate);

                    let result = agent.run_with_loop(&mut history, &ctx, cancel).await;

                    (step_idx, result)
                }));
            }

            for task in tasks {
                let (step_idx, result) = match task.await {
                    Ok(res) => res,
                    Err(e) => {
                        tracing::error!("Parallel step task panicked: {e}");
                        continue;
                    }
                };
                let step = &mut plan.steps[step_idx];
                step.status = StepStatus::Running;

                match result {
                    Ok(delta) => {
                        let text = delta.new_messages.iter()
                            .map(|m| m.text_content()).collect::<Vec<_>>().join("\n");
                        step.output = Some(text);
                        step.status = StepStatus::Completed;
                    }
                    Err(e) => {
                        step.error = Some(format!("{e}"));
                        step.status = StepStatus::Failed;
                    }
                }
            }

            eprintln!("   Wave {}/{} complete", wave_idx + 1, waves.len());

            if cancel.is_cancelled() { return Err(miniagent_core::error::AgentError::Cancelled); }
        }

        Ok(())
    }
}
