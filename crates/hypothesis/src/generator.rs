use miniagent_core::error::AgentError;
use miniagent_core::json_util;
use miniagent_kg::link_prediction::HypothesisCandidate;
use miniagent_kg::KnowledgeGraph;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::validation::{AnalysisVariables, DataAnalysisTask, DatasetSource, ValidationPlan, WetLabProtocol};

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

    /// Generate a hypothesis from a KG candidate.
    ///
    /// Returns `Ok(None)` when the evaluator marks the candidate as
    /// `plausible: false` — an implausible candidate must not become a
    /// hypothesis just because the JSON parsed.
    pub async fn generate(
        &self,
        candidate: &HypothesisCandidate,
        kg: &KnowledgeGraph,
        cancel: CancellationToken,
    ) -> Result<Option<Hypothesis>, AgentError> {
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
    ) -> Result<Option<Hypothesis>, AgentError> {
        // extract_and_repair strips <think> blocks + fences and repairs
        // truncated JSON; parse failures propagate as errors instead of
        // silently producing a hypothesis with an empty statement.
        let repaired = miniagent_core::json_util::extract_and_repair(text);
        let parsed: serde_json::Value = serde_json::from_str(&repaired).map_err(|e| {
            AgentError::invalid_config(format!(
                "hypothesis JSON parse failed: {e}; output head: {:?}",
                repaired.chars().take(160).collect::<String>()
            ))
        })?;

        // The evaluator's own plausibility gate: an explicit `false` drops
        // the candidate. Missing field means "not assessed" — keep it.
        if parsed["plausible"].as_bool() == Some(false) {
            tracing::info!(
                candidate = %candidate.head.0,
                "candidate marked implausible by evaluator; skipping"
            );
            return Ok(None);
        }

        Ok(Some(Hypothesis {
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
        }))
    }

    /// Generate a structured validation plan for a hypothesis.
    ///
    /// The plan deliberately separates **computational data-analysis tasks**
    /// (executable against public datasets or local files) from **wet-lab
    /// protocols** (bench work). The prompt instructs the model to ground each
    /// task in the hypothesis mechanism and to recommend concrete public
    /// datasets (GEO/TCGA/ArrayExpress) where possible.
    pub async fn generate_validation_plan(
        &self,
        hypothesis: &Hypothesis,
        kg: &KnowledgeGraph,
        cancel: CancellationToken,
    ) -> Result<ValidationPlan, AgentError> {
        let provider = self.pro_provider.as_ref().ok_or_else(|| {
            AgentError::invalid_config(
                "HypothesisGenerator requires a Pro provider for validation plan generation. \
                 Call with_provider() before use."
                    .to_string(),
            )
        })?;

        let head_name = kg
            .get_entity(&hypothesis.source_candidate.head)
            .map(|e| e.name.as_str())
            .unwrap_or("unknown");
        let tail_name = kg
            .get_entity(&hypothesis.source_candidate.tail)
            .map(|e| e.name.as_str())
            .unwrap_or("unknown");
        let rel_name = format!("{:?}", hypothesis.source_candidate.relation).to_lowercase();

        let evidence_text = if hypothesis.supporting_evidence.is_empty() {
            "No prior supporting evidence enumerated.".to_string()
        } else {
            hypothesis
                .supporting_evidence
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let prompt = format!(
            r#"You are a senior biomedical researcher designing a validation plan.

**Hypothesis under validation:**
{statement}

**Proposed mechanism:**
{mechanism}

**Graph relationship:** {head} --[{rel}]--> {tail} (algorithm confidence {score:.3})

**Supporting evidence from literature:**
{evidence}

**Task:** Design a concrete, executable validation plan that separates two tracks:

1. **data_analysis_tasks** — computational analyses over *existing public datasets*
   (GEO / TCGA / ArrayExpress) or a local data file. Each must specify a concrete
   dataset (accession when known), cohort definition, variables (independent /
   dependent / covariates), statistical method, expected outcome, and a concrete
   deliverable (e.g. "volcano plot + DE gene table CSV"). These will be executed
   automatically, so be precise.

2. **wet_lab_protocols** — bench procedures that cannot be automated. Specify
   reagents, ordered steps, controls, expected outcome, and timeline.

Recommend at least one data-analysis task and at least one wet-lab protocol when
applicable. Prefer datasets you can name by accession.

Output ONLY valid JSON (no markdown fences, no commentary) with this schema:
{{
  "rationale": "why these validations test the hypothesis",
  "data_analysis_tasks": [
    {{
      "id": "DA-1",
      "objective": "...",
      "dataset_source": {{"kind": "geo"}},
      "dataset_accession": "GSE00000",
      "cohort_definition": "...",
      "variables": {{"independent": ["..."], "dependent": ["..."], "covariates": ["..."]}},
      "statistical_method": "...",
      "expected_outcome": "...",
      "deliverable": "...",
      "priority": 0.9
    }}
  ],
  "wet_lab_protocols": [
    {{
      "id": "WL-1",
      "objective": "...",
      "reagents": ["..."],
      "steps": ["..."],
      "controls": ["..."],
      "expected_outcome": "...",
      "timeline_days": 14,
      "feasibility": 0.7
    }}
  ]
}}

`dataset_source.kind` ∈ {{"geo", "tcga", "arrayexpress", "local", "custom_url"}}.
For local/custom_url, also provide `value` (a path or URL)."#,
            statement = hypothesis.statement,
            mechanism = hypothesis.mechanism.as_deref().unwrap_or("(not specified)"),
            head = head_name,
            tail = tail_name,
            rel = rel_name,
            score = hypothesis.source_candidate.score,
            evidence = evidence_text,
        );

        let request = CompletionRequest {
            system: "You are a precise scientific planning engine. Output ONLY valid JSON.".into(),
            messages: vec![miniagent_core::message::Message::user(&prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.2),
                // Reasoning models burn part of the budget on CoT before the
                // JSON answer; 4000 left empty content on deepseek-reasoner.
                max_tokens: Some(8192),
                ..Default::default()
            },
        };

        let response = provider.complete(&request, cancel.clone()).await?;
        let mut text = response
            .content
            .iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        // Reasoning models can exhaust the token budget on chain-of-thought and
        // return empty text. One retry with a doubled budget recovers most
        // cases; without it the whole validation/analysis tail of the pipeline
        // silently produces nothing.
        if text.trim().is_empty() {
            tracing::warn!("validation plan: empty response ({}), retrying with larger budget", hypothesis.id);
            let retry = CompletionRequest {
                system: request.system.clone(),
                messages: request.messages.clone(),
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.2),
                    max_tokens: Some(16_384),
                    ..Default::default()
                },
            };
            let response = provider.complete(&retry, cancel).await?;
            text = response
                .content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
        }

        parse_validation_plan(&text, hypothesis.id)
    }
}

/// Tolerantly parse an LLM-produced validation plan JSON into typed structs.
fn parse_validation_plan(text: &str, hypothesis_id: uuid::Uuid) -> Result<ValidationPlan, AgentError> {
    // strip fences, fix truncation, and extract the JSON object in one step.
    let repaired = json_util::extract_and_repair(text);
    let root: serde_json::Value =
        serde_json::from_str(&repaired).map_err(|e| AgentError::invalid_config(
            format!("validation plan JSON parse failed: {e}"),
        ))?;

    let rationale = root
        .get("rationale")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let data_analysis_tasks = root
        .get("data_analysis_tasks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, v)| parse_data_analysis_task(i, v))
                .collect()
        })
        .unwrap_or_default();

    let wet_lab_protocols = root
        .get("wet_lab_protocols")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, v)| parse_wet_lab_protocol(i, v))
                .collect()
        })
        .unwrap_or_default();

    Ok(ValidationPlan {
        hypothesis_id,
        rationale,
        data_analysis_tasks,
        wet_lab_protocols,
    })
}

fn parse_data_analysis_task(idx: usize, v: &serde_json::Value) -> DataAnalysisTask {
    DataAnalysisTask {
        id: v
            .get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("DA-{}", idx + 1)),
        objective: as_string(v, "objective"),
        dataset_source: parse_dataset_source(v.get("dataset_source")),
        dataset_accession: v
            .get("dataset_accession")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        cohort_definition: as_string(v, "cohort_definition"),
        variables: parse_variables(v.get("variables")),
        statistical_method: as_string(v, "statistical_method"),
        expected_outcome: as_string(v, "expected_outcome"),
        deliverable: as_string(v, "deliverable"),
        priority: v
            .get("priority")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.5)
            .clamp(0.0, 1.0),
    }
}

fn parse_wet_lab_protocol(idx: usize, v: &serde_json::Value) -> WetLabProtocol {
    WetLabProtocol {
        id: v
            .get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("WL-{}", idx + 1)),
        objective: as_string(v, "objective"),
        reagents: as_string_array(v, "reagents"),
        steps: as_string_array(v, "steps"),
        controls: as_string_array(v, "controls"),
        expected_outcome: as_string(v, "expected_outcome"),
        timeline_days: v
            .get("timeline_days")
            .and_then(|x| x.as_u64())
            .map(|n| n as u32),
        feasibility: v
            .get("feasibility")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.5)
            .clamp(0.0, 1.0),
    }
}

/// Tolerate both `{"kind":"geo"}` and a bare `"geo"` string.
fn parse_dataset_source(v: Option<&serde_json::Value>) -> DatasetSource {
    let Some(v) = v else {
        return DatasetSource::Geo;
    };
    if let Some(s) = v.as_str() {
        return parse_source_kind(s, None);
    }
    let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("geo");
    let value = v.get("value").and_then(|x| x.as_str()).unwrap_or("");
    parse_source_kind(kind, Some(value))
}

fn parse_source_kind(kind: &str, value: Option<&str>) -> DatasetSource {
    let value = value.unwrap_or("").to_string();
    match kind.to_lowercase().as_str() {
        "tcga" => DatasetSource::Tcga,
        "arrayexpress" | "array_express" => DatasetSource::ArrayExpress,
        "local" => DatasetSource::Local(if value.is_empty() {
            "data.csv".to_string()
        } else {
            value
        }),
        "custom_url" | "customurl" | "url" => DatasetSource::CustomUrl(value),
        _ => DatasetSource::Geo,
    }
}

fn parse_variables(v: Option<&serde_json::Value>) -> AnalysisVariables {
    let Some(v) = v else {
        return AnalysisVariables::default();
    };
    AnalysisVariables {
        independent: as_string_array_val(v, "independent"),
        dependent: as_string_array_val(v, "dependent"),
        covariates: as_string_array_val(v, "covariates"),
    }
}

fn as_string(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn as_string_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    as_string_array_val(v, key)
}

fn as_string_array_val(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

impl Default for HypothesisGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod plausible_tests {
    use super::*;
    use miniagent_kg::link_prediction::{HypothesisCandidate, HypothesisEvidence, HypothesisNovelty as KgNovelty};
    use miniagent_kg::schema::{EntityId, RelationType};

    fn candidate() -> HypothesisCandidate {
        HypothesisCandidate {
            head: EntityId::new(),
            relation: RelationType::Regulates,
            tail: EntityId::new(),
            score: 0.7,
            evidence: HypothesisEvidence {
                kge_score: 0.7,
                path_score: 0.5,
                give_score: 0.4,
                supporting_paths: vec![],
                novelty: KgNovelty::Novel,
            },
        }
    }

    fn parse(text: &str) -> Result<Option<Hypothesis>, AgentError> {
        HypothesisGenerator::new().parse_hypothesis_response(text, &candidate())
    }

    #[test]
    fn explicit_implausible_is_filtered() {
        let h = parse(r#"{"plausible": false, "statement": "X causes Y"}"#).unwrap();
        assert!(h.is_none(), "plausible:false must not become a hypothesis");
    }

    #[test]
    fn explicit_plausible_passes() {
        let h = parse(r#"{"plausible": true, "statement": "X causes Y", "confidence": 0.8}""#)
            .unwrap()
            .expect("plausible:true → Some");
        assert_eq!(h.statement, "X causes Y");
        assert!((h.confidence - 0.8).abs() < 1e-9);
    }

    #[test]
    fn missing_plausible_field_keeps_hypothesis() {
        let h = parse(r#"{"statement": "X causes Y"}"#).unwrap();
        assert!(h.is_some(), "missing plausible field means not assessed");
    }
}

#[cfg(test)]
mod validation_plan_tests {
    use super::parse_validation_plan;

    #[test]
    fn parses_full_plan_with_tagged_dataset_source() {
        let json = r#"```json
{
  "rationale": "BRCA1 loss should reduce DNA-repair capacity in tumor cells.",
  "data_analysis_tasks": [
    {
      "id": "DA-1",
      "objective": "Measure BRCA1 differential expression",
      "dataset_source": {"kind": "geo"},
      "dataset_accession": "GSE12345",
      "cohort_definition": "tumor vs normal",
      "variables": {"independent": ["BRCA1"], "dependent": ["status"], "covariates": ["age"]},
      "statistical_method": "limma DE",
      "expected_outcome": "BRCA1 downregulated in tumor",
      "deliverable": "volcano + CSV",
      "priority": 0.9
    }
  ],
  "wet_lab_protocols": [
    {
      "id": "WL-1",
      "objective": "Western blot",
      "reagents": ["anti-BRCA1"],
      "steps": ["lyse", "run gel"],
      "controls": ["GAPDH"],
      "expected_outcome": "reduced band",
      "timeline_days": 3,
      "feasibility": 0.8
    }
  ]
}
```"#;
        let plan = parse_validation_plan(json, uuid::Uuid::new_v4()).unwrap();
        assert_eq!(plan.data_analysis_tasks.len(), 1);
        assert_eq!(plan.wet_lab_protocols.len(), 1);
        let t = &plan.data_analysis_tasks[0];
        assert_eq!(t.id, "DA-1");
        assert_eq!(t.dataset_source, crate::validation::DatasetSource::Geo);
        assert_eq!(t.dataset_accession.as_deref(), Some("GSE12345"));
        assert_eq!(t.variables.independent, vec!["BRCA1".to_string()]);
        assert!((t.priority - 0.9).abs() < 1e-9);
        assert_eq!(plan.wet_lab_protocols[0].timeline_days, Some(3));
    }

    #[test]
    fn tolerates_bare_string_dataset_source_and_local() {
        let json = r#"{"rationale":"x","data_analysis_tasks":[
            {"id":"DA-1","objective":"o","dataset_source":"local","dataset_accession":"data.csv","cohort_definition":"c","variables":{},"statistical_method":"t-test","expected_outcome":"e","deliverable":"d","priority":1.5}
        ],"wet_lab_protocols":[]}"#;
        let plan = parse_validation_plan(json, uuid::Uuid::new_v4()).unwrap();
        let t = &plan.data_analysis_tasks[0];
        // bare "local" string + default value path.
        assert_eq!(t.dataset_source, crate::validation::DatasetSource::Local("data.csv".into()));
        // priority clamped to 1.0.
        assert!((t.priority - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fills_default_ids_when_missing() {
        let json = r#"{"rationale":"x","data_analysis_tasks":[
            {"objective":"o","statistical_method":"t","expected_outcome":"e","deliverable":"d"}
        ],"wet_lab_protocols":[
            {"objective":"p","expected_outcome":"e"}
        ]}"#;
        let plan = parse_validation_plan(json, uuid::Uuid::new_v4()).unwrap();
        assert_eq!(plan.data_analysis_tasks[0].id, "DA-1");
        assert_eq!(plan.wet_lab_protocols[0].id, "WL-1");
    }

    #[test]
    fn rejects_garbage() {
        let res = parse_validation_plan("not json at all", uuid::Uuid::new_v4());
        assert!(res.is_err());
    }

    #[test]
    fn empty_arrays_when_keys_absent() {
        let json = r#"{"rationale":"x"}"#;
        let plan = parse_validation_plan(json, uuid::Uuid::new_v4()).unwrap();
        assert_eq!(plan.task_count(), 0);
        assert_eq!(plan.rationale, "x");
    }
}
