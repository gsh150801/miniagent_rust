use miniagent_core::error::AgentError;
use miniagent_kg::link_prediction::HypothesisCandidate;
use miniagent_kg::KnowledgeGraph;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: uuid::Uuid,
    pub statement: String,
    pub mechanism: Option<String>,
    pub novelty: HypothesisNovelty,
    pub confidence: f64,
    pub supporting_evidence: Vec<String>,
    pub counter_evidence: Vec<String>,
    pub experimental_design: Option<ExperimentDesign>,
    pub source_candidate: HypothesisCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HypothesisNovelty {
    Novel,
    Incremental,
    Trivial,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentDesign {
    pub approach: String,
    pub methods: Vec<String>,
    pub expected_outcomes: Vec<String>,
    pub controls: Vec<String>,
    pub feasibility: f64,
}

pub struct HypothesisGenerator {
    pro_provider: Option<Box<dyn LlmProvider>>,
}

impl HypothesisGenerator {
    pub fn new() -> Self {
        Self { pro_provider: None }
    }

    pub fn with_provider(mut self, provider: Box<dyn LlmProvider>) -> Self {
        self.pro_provider = Some(provider);
        self
    }

    /// Generate a hypothesis from a KG candidate
    pub async fn generate(
        &self,
        candidate: &HypothesisCandidate,
        kg: &KnowledgeGraph,
        cancel: CancellationToken,
    ) -> Result<Hypothesis, AgentError> {
        let head = kg.get_entity(&candidate.head);
        let tail = kg.get_entity(&candidate.tail);

        let head_name = head.map(|e| e.name.as_str()).unwrap_or("unknown");
        let tail_name = tail.map(|e| e.name.as_str()).unwrap_or("unknown");
        let head_type = head.map(|e| format!("{:?}", e.entity_type)).unwrap_or_default();
        let tail_type = tail.map(|e| format!("{:?}", e.entity_type)).unwrap_or_default();
        let rel_name = format!("{:?}", candidate.relation).to_lowercase();

        let paths_text = candidate
            .evidence
            .supporting_paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let steps: Vec<String> = path
                    .iter()
                    .map(|(from, rt, to)| {
                        let from_name = kg.get_entity(from).map(|e| e.name.as_str()).unwrap_or("?");
                        let to_name = kg.get_entity(to).map(|e| e.name.as_str()).unwrap_or("?");
                        format!("{from_name} --[{:?}]--> {to_name}", rt)
                    })
                    .collect();
                format!("Path {}: {}", i + 1, steps.join(" → "))
            })
            .collect::<Vec<_>>()
            .join("\n");

        // If we have a provider, use it for validation
        if let Some(ref provider) = self.pro_provider {
            let prompt = format!(
                r#"You are a scientific hypothesis evaluator. A knowledge graph link prediction algorithm has identified a potential novel relationship:

**Candidate Relationship:**
- {head_name} ({head_type}) --[{rel_name}]--> {tail_name} ({tail_type})
- Algorithm Confidence: {score:.3}

**Graph Evidence Paths:**
{paths_text}

**Task:**
1. Evaluate the biological/scientific plausibility of this relationship
2. If plausible, formulate it as a complete, testable scientific hypothesis
3. Propose a mechanism explaining the relationship
4. Assess novelty: Novel (previously unknown), Incremental (refinement), or Trivial (already known)
5. List supporting evidence (from existing literature reasoning)
6. List potential counter-evidence or alternative explanations
7. Design a validation experiment with:
   - Experimental approach
   - Specific methods
   - Expected outcomes (if hypothesis is correct)
   - Appropriate controls
   - Feasibility (0-1)

Output as JSON:
{{
  "plausible": true/false,
  "statement": "...",
  "mechanism": "...",
  "novelty": "Novel|Incremental|Trivial",
  "confidence": 0.0-1.0,
  "supporting_evidence": ["..."],
  "counter_evidence": ["..."],
  "experiment": {{
    "approach": "...",
    "methods": ["..."],
    "expected_outcomes": ["..."],
    "controls": ["..."],
    "feasibility": 0.0-1.0
  }}
}}"#,
                head_name = head_name,
                head_type = head_type,
                tail_name = tail_name,
                tail_type = tail_type,
                rel_name = rel_name,
                score = candidate.score,
                paths_text = paths_text,
            );

            let request = CompletionRequest {
                system: "You are a precise scientific reasoning engine. Output ONLY valid JSON, no commentary.".into(),
                messages: vec![miniagent_core::message::Message::user(&prompt)],
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.1),
                    max_tokens: Some(4000),
                    ..Default::default()
                },
            };

            let response = provider.complete(&request, cancel).await?;
            let text = response.content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            // Parse JSON from response
            self.parse_hypothesis_response(&text, candidate)
        } else {
            Err(AgentError::invalid_config(
                "HypothesisGenerator requires a Pro provider for LLM validation. \
                 Call with_provider() before use.".to_string()
            ))
        }
    }

    fn parse_hypothesis_response(
        &self,
        text: &str,
        candidate: &HypothesisCandidate,
    ) -> Result<Hypothesis, AgentError> {
        let json_str = text
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap_or_default();

        Ok(Hypothesis {
            id: uuid::Uuid::new_v4(),
            statement: parsed["statement"].as_str().unwrap_or("").to_string(),
            mechanism: parsed["mechanism"].as_str().map(|s| s.to_string()),
            novelty: match parsed["novelty"].as_str().unwrap_or("Unknown") {
                "Novel" => HypothesisNovelty::Novel,
                "Incremental" => HypothesisNovelty::Incremental,
                "Trivial" => HypothesisNovelty::Trivial,
                _ => HypothesisNovelty::Unknown,
            },
            confidence: parsed["confidence"].as_f64().unwrap_or(candidate.score),
            supporting_evidence: parsed["supporting_evidence"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
            counter_evidence: parsed["counter_evidence"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
            experimental_design: parsed["experiment"].as_object().map(|exp| ExperimentDesign {
                approach: exp.get("approach").and_then(|v| v.as_str()).unwrap_or("").into(),
                methods: exp.get("methods")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                expected_outcomes: exp.get("expected_outcomes")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                controls: exp.get("controls")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                feasibility: exp.get("feasibility").and_then(|v| v.as_f64()).unwrap_or(0.5),
            }),
            source_candidate: candidate.clone(),
        })
    }
}

impl Default for HypothesisGenerator {
    fn default() -> Self {
        Self::new()
    }
}
