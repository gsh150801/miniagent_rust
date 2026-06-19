use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_tool::traits::{Tool, ToolClass, ToolContext, ToolOutput};
use tokio_util::sync::CancellationToken;
use crate::bundle::SkillBundle;
use crate::registry::SkillRegistry;
use std::sync::Arc;

/// Makes a skill executable as a Tool, allowing the Agent to invoke it
/// via tool-call just like any other tool.
pub struct SkillAsTool {
    #[allow(dead_code)]
    bundle: SkillBundle,
    registry: Arc<SkillRegistry>,
}

impl SkillAsTool {
    pub fn new(bundle: SkillBundle, registry: Arc<SkillRegistry>) -> Self {
        Self { bundle, registry }
    }
}

#[async_trait]
impl Tool for SkillAsTool {
    fn name(&self) -> &str { "skill_execute" }
    fn description(&self) -> &str { "Execute a registered skill by name" }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        let skill_names: Vec<&str> = self.registry.all().iter()
            .map(|s| s.metadata.name.as_str())
            .collect();

        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill to execute",
                    "enum": skill_names
                },
                "input": {
                    "type": "string",
                    "description": "Input to pass to the skill"
                }
            },
            "required": ["skill", "input"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let skill_name = input["skill"].as_str()
            .ok_or_else(|| AgentError::tool("skill_execute", "missing 'skill' name"))?;
        let user_input = input["input"].as_str().unwrap_or("");

        let bundle = self.registry.get_by_name(skill_name)
            .ok_or_else(|| AgentError::tool("skill_execute", format!("Skill '{skill_name}' not found")))?;

        // Return the skill prompt body — the actual LLM execution happens
        // when the Agent feeds this into its context in the next turn.
        let output = format!(
            "## Skill: {name}\n\
             {body}\n\n\
             ---\n\
             **Task**: {user_input}\n\
             **Instructions**: Execute the above skill on the provided input. \
             Follow the skill's methodology precisely.",
            name = bundle.metadata.name,
            body = bundle.body,
        );

        Ok(ToolOutput {
            content: output,
            metadata: Some(miniagent_tool::traits::ToolMetadata {
                duration_ms: 0,
                is_error: false,
            }),
        })
    }
}

/// Chain multiple skills together into a pipeline.
/// Skills execute sequentially, each receiving the previous skill's output.
pub struct SkillChain {
    skill_names: Vec<String>,
    registry: Arc<SkillRegistry>,
}

impl SkillChain {
    pub fn new(skill_names: Vec<String>, registry: Arc<SkillRegistry>) -> Self {
        Self { skill_names, registry }
    }

    /// Build the combined prompt for the full skill chain.
    pub fn build_prompt(&self, initial_input: &str) -> Result<String, AgentError> {
        let mut prompt = String::from("# Skill Chain Execution\n\n");
        prompt.push_str("Execute the following skills in sequence. \
                         Each skill's output feeds into the next.\n\n");

        for (i, name) in self.skill_names.iter().enumerate() {
            let bundle = self.registry.get_by_name(name)
                .ok_or_else(|| AgentError::tool("skill_chain", format!("Skill '{name}' not found")))?;

            prompt.push_str(&format!(
                "## Step {}: {}\n{}\n\n",
                i + 1,
                bundle.metadata.name,
                bundle.body,
            ));

            if i == 0 {
                prompt.push_str(&format!("**Initial Input**: {initial_input}\n\n"));
            } else {
                prompt.push_str("**Input**: Output from previous step\n\n");
            }
        }

        prompt.push_str("---\n");
        prompt.push_str("Execute all steps sequentially. Output the final result after the last step.\n");

        Ok(prompt)
    }
}
