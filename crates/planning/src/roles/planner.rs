use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, FileContext,
            call_llm_with_tools, load_todo, append_event, parse_llm_json};

/// Planner (规划者) — generates and refines execution plans.
///
/// Inspired by Anthropic's Evaluator-Optimizer pattern:
/// the planner creates initial plans and refines them based on
/// evaluator feedback, producing concrete step-by-step instructions.
pub struct PlannerRole {
    provider: Box<dyn LlmProvider>,
}

impl PlannerRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for PlannerRole {
    fn name(&self) -> &str { "planner" }
    fn description(&self) -> &str {
        "Execution planner. Creates detailed step-by-step plans from high-level goals, \
         refines plans based on feedback. All plans persisted to planner/ directory."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let supervisor_plan = blackboard.get("supervisor/plan.json");
        let evaluator_feedback = blackboard.get("evaluator/evaluation.json");
        let prior_plan = blackboard.get("planner/current_plan.json");

        let feedback_section = evaluator_feedback.map(|f| {
            format!("\n## Evaluator Feedback (refine plan accordingly)\n{f}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are the **Planner** in a multi-agent system.

**Task:** {task}

## Todo (attention anchor)
{todo}
{supervisor_section}
{prior_section}
{feedback_section}

## Your Role
1. Break the task into concrete, executable steps
2. For each step, specify: action, tools needed, expected output, success criteria
3. Define retry/fallback strategies for risky steps
4. Estimate token/time budgets per step

## Output Format (JSON)
{{
  "plan_version": 1,
  "steps": [
    {{
      "id": "s1",
      "action": "concrete action description",
      "agent_hint": "researcher|executor|writer|...",
      "tools_needed": ["web_search", "file_read"],
      "input_files": ["path"],
      "output_file": "planned output path",
      "success_criteria": "how to verify this step succeeded",
      "fallback": "what to do if this step fails",
      "estimated_tokens": 500,
      "depends_on": []
    }}
  ],
  "total_estimated_tokens": 5000,
  "critical_path": ["s1", "s3", "s5"]
}}
"#,
            supervisor_section = supervisor_plan.map(|p| format!("\n## Supervisor's Plan\n{p}")).unwrap_or_default(),
            prior_section = prior_plan.map(|p| format!("\n## Prior Plan (refine if exists)\n{p}")).unwrap_or_default(),
        );

        let system = "You are an expert task planner. Create detailed, executable plans with \
                       clear success criteria and fallback strategies. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "planner: generating execution plan");

        // Check budget before LLM call
        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = call_llm_with_tools(
            blackboard.agent(), &*self.provider, &[],
            &system, &prompt, &blackboard.work_dir_str(), cancel,
        ).await?;
        let parsed = self.parse_response(&response);

        // Persist plan
        let plan_json = serde_json::to_string_pretty(&serde_json::json!({
            "plan_version": parsed.metadata.get("plan_version"),
            "steps": parsed.metadata.get("steps"),
            "critical_path": parsed.metadata.get("critical_path"),
        })).unwrap_or_default();
        blackboard.put("planner/current_plan.json", &plan_json)?;

        let version = blackboard.iteration;
        blackboard.put(&format!("planner/plan_v{version}.md"), &parsed.content)?;

        append_event(&blackboard.work_dir, "planner: plan persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for PlannerRole {
    fn workspace_name(&self) -> &str { "planner" }
}

impl PlannerRole {
    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let version = parsed["plan_version"].as_u64().unwrap_or(1);
        let steps = parsed["steps"].as_array().cloned().unwrap_or_default();
        let steps_text = steps.iter().enumerate().map(|(i, s)| {
            format!("{}. {} [{}] → {}",
                i + 1,
                s["action"].as_str().unwrap_or(""),
                s["agent_hint"].as_str().unwrap_or(""),
                s["output_file"].as_str().unwrap_or(""),
            )
        }).collect::<Vec<_>>().join("\n");

        let mut metadata = std::collections::HashMap::new();
        if let Some(v) = parsed["plan_version"].as_u64() {
            metadata.insert("plan_version".into(), v.to_string());
        }
        metadata.insert("steps".into(), serde_json::to_string(&steps).unwrap_or_default());
        if let Some(cp) = parsed["critical_path"].as_array() {
            metadata.insert("critical_path".into(), serde_json::to_string(cp).unwrap_or_default());
        }
        if let Some(tokens) = parsed["total_estimated_tokens"].as_u64() {
            metadata.insert("total_estimated_tokens".into(), tokens.to_string());
        }

        RoleOutput {
            content: format!("## Execution Plan\n\n{steps_text}"),
            evidence: vec![],
            confidence: 0.8,
            metadata,
            output_files: vec![format!("planner/current_plan.json"), format!("planner/plan_v{version}.md")],
            status: "success".into(),
        }
    }
}
