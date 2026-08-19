use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

use super::{AgentRole, Blackboard, RoleOutput, EvidenceItem, FileContext,
            call_llm_with_tools, load_todo, append_event, parse_llm_json};

/// Opponent (反方) — challenges hypotheses, finds counter-evidence.
/// Simulates the most skeptical peer reviewer in the debate triad.
pub struct OpponentRole {
    provider: Box<dyn LlmProvider>,
}

impl OpponentRole {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self { Self { provider } }
}

#[async_trait]
impl AgentRole for OpponentRole {
    fn name(&self) -> &str { "opponent" }
    fn description(&self) -> &str {
        "Scientific hypothesis opponent (反方). Rigorously challenges claims, finds counter-evidence, \
         identifies logical flaws, proposes alternatives. Persists to opponent/ directory."
    }

    async fn execute(
        &self, task: &str, blackboard: &mut Blackboard, cancel: CancellationToken,
    ) -> Result<RoleOutput, AgentError> {
        let todo = load_todo(&blackboard.work_dir);
        let hypothesis = blackboard.get("proposer/hypothesis.json")
            .unwrap_or_else(|| "No hypothesis found".to_string());
        let judge_verdict = blackboard.get("judge/verdict.json");

        let verdict_section = judge_verdict.map(|v| {
            format!("\n## Previous Judge Verdict (consider in your critique)\n{v}")
        }).unwrap_or_default();

        let prompt = format!(
            r#"You are a scientific hypothesis opponent (反方). Your role is adversarial critique.

**Research Topic:** {task}

## Todo
{todo}

**Proposer's Hypothesis:**
{hypothesis}{verdict_section}

## Your Role
Find EVERY weakness:
1. **Evidence Quality**: Rate each piece of evidence. Is the source credible?
2. **Methodological Flaws**: What could invalidate the claims?
3. **Counter-Evidence**: What contradicts this hypothesis?
4. **Alternative Explanations**: Propose 2-3 alternatives
5. **Logical Gaps**: What hidden assumptions are made?
6. **Falsifiability**: Can this actually be disproven?

## Output Format (JSON)
{{
  "overall_score": 0.0-1.0,
  "evidence_quality": 0.0-1.0,
  "methodological_rigor": 0.0-1.0,
  "novelty": 0.0-1.0,
  "falsifiability": 0.0-1.0,
  "critical_flaws": ["flaw 1", "flaw 2"],
  "counter_evidence": [{{"claim": "...", "source": "PMID:...", "strength": 0.8}}],
  "alternative_explanations": ["alt 1", "alt 2"],
  "recommendation": "accept|revise|reject",
  "revision_suggestions": ["suggestion 1"]
}}
"#
        );

        let system = "You are the most rigorous scientific skeptic. Find every flaw. \
                       Be specific. Output only valid JSON.".to_string();

        append_event(&blackboard.work_dir, "opponent: challenging hypothesis");

        if blackboard.budget_exhausted() {
            return Ok(RoleOutput::failed(self.name(), "Budget exhausted"));
        }

        let response = call_llm_with_tools(
            blackboard.agent(), &*self.provider, &[],
            &system, &prompt, &blackboard.work_dir_str(), cancel,
        ).await?;
        let parsed = self.parse_response(&response);

        // Persist critique
        let output_json = serde_json::to_string_pretty(&serde_json::json!({
            "overall_score": parsed.confidence,
            "critical_flaws": parsed.content,
            "counter_evidence": parsed.evidence.iter().map(|e| serde_json::json!({
                "claim": e.claim, "source": e.source, "strength": e.strength,
            })).collect::<Vec<_>>(),
            "scores": {
                "evidence_quality": parsed.metadata.get("evidence_quality"),
                "methodological_rigor": parsed.metadata.get("methodological_rigor"),
                "novelty": parsed.metadata.get("novelty"),
                "falsifiability": parsed.metadata.get("falsifiability"),
            },
            "recommendation": parsed.metadata.get("recommendation"),
        })).unwrap_or_default();
        blackboard.put("opponent/critique.json", &output_json)?;

        let scores = serde_json::json!({
            "overall": parsed.confidence,
            "evidence_quality": parsed.metadata.get("evidence_quality"),
            "methodological_rigor": parsed.metadata.get("methodological_rigor"),
            "novelty": parsed.metadata.get("novelty"),
            "falsifiability": parsed.metadata.get("falsifiability"),
            "recommendation": parsed.metadata.get("recommendation"),
        });
        blackboard.put("opponent/scores.json",
            &serde_json::to_string_pretty(&scores).unwrap_or_default())?;

        append_event(&blackboard.work_dir, "opponent: critique persisted");

        Ok(parsed)
    }
}

#[async_trait]
impl FileContext for OpponentRole {
    fn workspace_name(&self) -> &str { "opponent" }
}

impl OpponentRole {
    fn parse_response(&self, text: &str) -> RoleOutput {
        let parsed = match parse_llm_json(text) {
            Ok(v) => v,
            Err(e) => return RoleOutput::failed(self.name(), &e),
        };

        let mut metadata = std::collections::HashMap::new();
        for key in &["evidence_quality", "methodological_rigor", "novelty", "falsifiability", "recommendation"] {
            if let Some(v) = parsed[key].as_f64() { metadata.insert(key.to_string(), v.to_string()); }
            if let Some(v) = parsed[key].as_str() { metadata.insert(key.to_string(), v.to_string()); }
        }
        if let Some(arr) = parsed["revision_suggestions"].as_array() {
            metadata.insert("revision_suggestions".into(),
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "));
        }

        RoleOutput {
            content: parsed["critical_flaws"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default(),
            evidence: parsed["counter_evidence"].as_array().map(|a| a.iter().map(|e| EvidenceItem {
                claim: e["claim"].as_str().unwrap_or("").to_string(),
                source: e["source"].as_str().unwrap_or("").to_string(),
                strength: e["strength"].as_f64().unwrap_or(0.5),
                counter_evidence: vec![],
            }).collect()).unwrap_or_default(),
            confidence: parsed["overall_score"].as_f64().unwrap_or(0.5),
            metadata,
            output_files: vec!["opponent/critique.json".into(), "opponent/scores.json".into()],
            status: "success".into(),
        }
    }
}
