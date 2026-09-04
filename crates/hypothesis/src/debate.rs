//! Hypothesis debate · cross-comparison · refinement.
//!
//! This module closes the gap between the (already implemented) KG-driven
//! hypothesis generator + ranker and the generic `DebateRunner`. It specializes
//! the Proposer / Opponent / Judge pattern to operate on concrete
//! [`Hypothesis`] values — each carrying KG evidence paths and literature
//! supporting / counter evidence — so that, for a given disease, the several
//! competing hypotheses can be:
//!
//! 1. **Debated** one-by-one on evidence vs. contradiction (Phase A),
//! 2. **Cross-compared** to surface inter-hypothesis conflicts and rank by
//!    overall evidence strength (Phase B),
//! 3. **Refined** where the debate exposed weaknesses (Phase C).
//!
//! This directly implements goal 2 of the project brief: *"对于提出的（每种
//! 疾病）若干假说，分别基于海量文献，进行证据-矛盾之处的辩论和比较，然后
//! 进一步思考假说是否有可以完善的地方"*.

use miniagent_core::error::AgentError;
use miniagent_core::json_util;
use miniagent_core::message::Message;
use miniagent_kg::KnowledgeGraph;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio_util::sync::CancellationToken;

use crate::generator::Hypothesis;

// ───────────────────────────── public types ─────────────────────────────

/// Judge's verdict for a single hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The evidence holds up; the hypothesis stands as-is.
    Accept,
    /// Plausible but has weaknesses worth addressing before validation.
    Revise,
    /// The contradictions / lack of evidence are decisive.
    Reject,
}

impl Verdict {
    fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "accept" | "accepted" | "accepts" => Verdict::Accept,
            "reject" | "rejected" | "rejects" => Verdict::Reject,
            _ => Verdict::Revise,
        }
    }
}

/// Debate outcome for a single hypothesis (Phase A).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypothesisVerdict {
    pub hypothesis_id: uuid::Uuid,
    pub verdict: Verdict,
    /// Strongest literature / graph-grounded points in favour.
    pub supporting_points: Vec<String>,
    /// Strongest contradictions, counter-evidence, alternative explanations.
    pub contradicting_points: Vec<String>,
    /// Confidence after weighing both sides, in `[0,1]`.
    pub confidence_after: f64,
    /// Free-text notes on how the hypothesis could be improved.
    pub refinement_notes: String,
    /// What the Opponent independently recommended (audit: disagreement with
    /// the Judge's verdict is visible in the report). Absent on legacy data.
    #[serde(default)]
    pub opponent_recommendation: Option<Verdict>,
    /// The Proposer's second-round answers to the Opponent's strongest
    /// objections. Empty when the Opponent raised nothing or the rebuttal
    /// call failed (degrade, don't fail). Absent on legacy data.
    #[serde(default)]
    pub rebuttal_points: Vec<String>,
}

/// Cross-comparison across all hypotheses (Phase B).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrossComparison {
    /// Pairs of hypothesis ids that are in tension, with the reason.
    pub contradictions_between: Vec<ContradictionPair>,
    /// Why the hypotheses are ordered the way they are.
    pub ranking_rationale: String,
    /// The single strongest hypothesis id, if any.
    pub strongest_id: Option<uuid::Uuid>,
    /// Suggestions to merge / drop / combine hypotheses.
    pub merge_suggestions: Vec<String>,
}

/// A pair of mutually contradicting hypotheses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionPair {
    pub a: uuid::Uuid,
    pub b: uuid::Uuid,
    pub reason: String,
}

/// Full result of [`HypothesisDebater::debate_and_refine`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateOutcome {
    /// Per-hypothesis debate verdicts (full audit, includes rejected ones).
    pub per_hypothesis: Vec<HypothesisVerdict>,
    pub comparison: CrossComparison,
    /// Refined & re-ranked hypotheses (excludes rejected). Drives downstream
    /// validation planning.
    pub refined: Vec<Hypothesis>,
    /// Number of refinement rounds applied.
    pub rounds: usize,
}

// ───────────────────────────── debater ─────────────────────────────

pub struct HypothesisDebater {
    /// Proposer role: argues FOR the hypothesis (its own prompt + model).
    proposer: Box<dyn LlmProvider>,
    /// Opponent role: adversarial critique (its own prompt + model). A
    /// separate prompt/model is what makes the critique adversarial instead
    /// of the same call politely listing both sides.
    opponent: Box<dyn LlmProvider>,
    /// Judge role: per-hypothesis adjudication (Phase A), cross-comparison
    /// (Phase B), and refinement (Phase C).
    judge: Box<dyn LlmProvider>,
    /// Cap on how many hypotheses to send into refinement at once.
    max_refine: usize,
}

impl HypothesisDebater {
    /// Three debate roles, each with its own provider. Use the same provider
    /// for all three when no role split is configured (defaults to the main
    /// model).
    pub fn new(
        proposer: Box<dyn LlmProvider>,
        opponent: Box<dyn LlmProvider>,
        judge: Box<dyn LlmProvider>,
    ) -> Self {
        Self {
            proposer,
            opponent,
            judge,
            max_refine: 6,
        }
    }

    /// Run the three-phase debate → compare → refine pipeline.
    pub async fn debate_and_refine(
        &self,
        hypotheses: &[Hypothesis],
        kg: &KnowledgeGraph,
        cancel: CancellationToken,
    ) -> Result<DebateOutcome, AgentError> {
        self.debate_and_refine_with_evidence(hypotheses, kg, &Default::default(), cancel)
            .await
    }

    /// Debate with externally retrieved evidence (e.g. web-search results)
    /// injected per hypothesis. Grounding the debate in retrieved literature
    /// instead of parametric memory alone is what makes the verdicts auditable:
    /// every supporting/contradicting point can be traced back to a source.
    pub async fn debate_and_refine_with_evidence(
        &self,
        hypotheses: &[Hypothesis],
        kg: &KnowledgeGraph,
        evidence: &std::collections::HashMap<uuid::Uuid, String>,
        cancel: CancellationToken,
    ) -> Result<DebateOutcome, AgentError> {
        // Trivial cases: nothing to debate.
        if hypotheses.is_empty() {
            return Ok(DebateOutcome {
                per_hypothesis: vec![],
                comparison: CrossComparison::default(),
                refined: vec![],
                rounds: 0,
            });
        }
        if hypotheses.len() == 1 {
            // Still worth a single debate pass so the lone hypothesis is
            // stress-tested, but skip cross-comparison.
            let v = self
                .debate_one(&hypotheses[0], kg, evidence.get(&hypotheses[0].id).map(|s| s.as_str()), cancel.clone())
                .await?;
            let mut refined = hypotheses.to_vec();
            if v.verdict == Verdict::Revise {
                refined = self
                    .refine_batch(&[(refined.into_iter().next().unwrap(), v.clone())], kg, cancel.clone())
                    .await?;
            } else if v.verdict == Verdict::Reject {
                refined.clear();
            } else if let Some(h) = refined.get_mut(0) {
                h.confidence = v.confidence_after;
            }
            return Ok(DebateOutcome {
                per_hypothesis: vec![v],
                comparison: CrossComparison::default(),
                refined,
                rounds: 1,
            });
        }

        // Phase A: per-hypothesis debate. Hypotheses are independent — run
        // them concurrently via `buffered` (same-task concurrency, no
        // spawn/'static needed); each internally stays Proposer → Opponent →
        // Judge sequential. Bounded at 3 so provider rate limits hold.
        let mut verdicts = Vec::with_capacity(hypotheses.len());
        {
            use futures_util::stream::{self, StreamExt};
            // Owned captures per job (cloned hypothesis + evidence text):
            // borrows with caller-derived lifetimes flowing through the
            // generic stream combinator break the Send auto-trait once this
            // whole future is spawned (server research mode).
            let results: Vec<Result<HypothesisVerdict, AgentError>> = stream::iter(
                hypotheses.iter().map(|h| (h.clone(), evidence.get(&h.id).cloned())),
            )
                .map(|(h, evidence_text)| {
                    let cancel = cancel.clone();
                    async move {
                        self.debate_one(&h, kg, evidence_text.as_deref(), cancel).await
                    }
                })
                .buffered(3)
                .collect()
                .await;
            for (idx, r) in results.into_iter().enumerate() {
                match r {
                    Ok(v) => verdicts.push(v),
                    Err(e) => {
                        // One failed debate must not kill the rest — degrade
                        // to a revise verdict with the error as the refinement
                        // note so the audit shows why. `buffered` preserves
                        // input order, so index alignment with `hypotheses`
                        // holds.
                        tracing::warn!(error = %e, "phase A debate failed for one hypothesis");
                        verdicts.push(HypothesisVerdict {
                            hypothesis_id: hypotheses[idx].id,
                            verdict: Verdict::Revise,
                            supporting_points: vec![],
                            contradicting_points: vec![],
                            confidence_after: 0.0,
                            refinement_notes: format!("debate failed: {e}"),
                            opponent_recommendation: None,
                            rebuttal_points: vec![],
                        });
                    }
                }
            }
        }

        // Phase B: cross-comparison (judge = pro). A failed comparison must
        // not discard the per-hypothesis debate work — degrade to an empty
        // comparison so the verdicts (and the refined set below) survive.
        let comparison = match self.compare(hypotheses, &verdicts, kg, cancel.clone()).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "phase B cross-comparison failed — continuing with per-hypothesis verdicts only"
                );
                CrossComparison::default()
            }
        };

        // Phase C: refine the Revise candidates (bounded by max_refine).
        let to_refine: Vec<(Hypothesis, HypothesisVerdict)> = hypotheses
            .iter()
            .zip(verdicts.iter())
            .filter(|(_, v)| v.verdict == Verdict::Revise)
            .map(|(h, v)| (h.clone(), v.clone()))
            .take(self.max_refine)
            .collect();

        let rounds = {
            let any_rebuttal = verdicts.iter().any(|v| !v.rebuttal_points.is_empty());
            1 + usize::from(any_rebuttal) + usize::from(!to_refine.is_empty())
        };
        // Phase C: refinement failure degrades to "keep the debated originals
        // with updated confidence" (the assembly loop already handles missing
        // refined entries) instead of discarding the whole debate outcome.
        let mut refined_map = match self.refine_batch(&to_refine, kg, cancel.clone()).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "phase C refinement failed — keeping debated originals with post-debate confidence"
                );
                Vec::new()
            }
        };

        // Assemble the final refined set, ordered by post-debate confidence.
        // Accept hypotheses pass through (confidence bumped to verdict value).
        // Reject hypotheses are dropped from `refined` but stay in the audit.
        let mut refined: Vec<Hypothesis> = Vec::with_capacity(hypotheses.len());
        for (h, v) in hypotheses.iter().zip(verdicts.iter()) {
            match v.verdict {
                Verdict::Reject => continue,
                Verdict::Revise => {
                    if let Some(pos) = refined_map.iter().position(|r| r.id == h.id) {
                        // refined version carries updated statement/confidence.
                        let mut r = refined_map.remove(pos);
                        if r.confidence <= 0.0 {
                            r.confidence = v.confidence_after;
                        }
                        refined.push(r);
                    } else {
                        // refinement failed for this one — keep original, bump confidence.
                        let mut kept = h.clone();
                        kept.confidence = v.confidence_after;
                        refined.push(kept);
                    }
                }
                Verdict::Accept => {
                    let mut kept = h.clone();
                    kept.confidence = v.confidence_after;
                    refined.push(kept);
                }
            }
        }
        refined.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(DebateOutcome {
            per_hypothesis: verdicts,
            comparison,
            refined,
            rounds,
        })
    }

    // ── Phase A ───────────────────────────────────────────────────
    /// Three adversarial calls per hypothesis: Proposer argues FOR,
    /// Opponent attacks, Judge weighs both sides and rules. Each role has
    /// its own prompt — and its own model when configured — so the critique
    /// is genuinely independent of the advocacy.
    async fn debate_one(
        &self,
        h: &Hypothesis,
        kg: &KnowledgeGraph,
        external_evidence: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<HypothesisVerdict, AgentError> {
        let ctx = render_hypothesis_context(h, kg);
        let evidence_block = external_evidence
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                format!(
                    "\n**Retrieved literature evidence (web search — cite these where applicable):**\n{s}\n"
                )
            })
            .unwrap_or_default();

        // 1. Proposer — advocate.
        let proposer_prompt = format!(
            r#"You are the PROPOSER (正方) in a scientific debate. Argue IN FAVOUR of the hypothesis below using the published literature and the graph evidence. Be a strong but honest advocate: only claim support the evidence can carry.

{ctx}
{evidence_block}

**Your task:** list the 2-4 strongest pieces of evidence or reasoning that SUPPORT the hypothesis (cite the kind of literature, e.g. "GEO expression studies", "GWAS", "prior reviews"), and your confidence as the proposer. When a point draws on the retrieved evidence above, cite its URL or PMID in parentheses at the end of the point.

Output ONLY valid JSON (no markdown fences):
{{
  "supporting_points": ["..."],
  "proposer_confidence": 0.0
}}"#
        );
        let prop = complete_json_with_retry(
            self.proposer.as_ref(),
            "You are a rigorous scientific proposer. Output ONLY valid JSON.",
            &proposer_prompt,
            "proposer",
            cancel.clone(),
        )
        .await?;
        let supporting_points = str_array(&prop, "supporting_points");
        let proposer_confidence = prop
            .get("proposer_confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(h.confidence)
            .clamp(0.0, 1.0);
        let proposer_case = if supporting_points.is_empty() {
            "(proposer offered no points)".to_string()
        } else {
            supporting_points
                .iter()
                .map(|s| format!("  - {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // 2. Opponent — adversarial critique with its own prompt.
        let opponent_prompt = format!(
            r#"You are the OPPONENT (反方) in a scientific debate. Your role is adversarial critique. Find every flaw in the hypothesis below: missing causal links, over-generalization, confounders, contradicting literature, alternative explanations that fit the same evidence, and weaknesses in the graph evidence itself.

{ctx}
{evidence_block}

**The proposer has already argued:**
{proposer_case}

**Your task:** attack. List the 2-4 strongest CONTRADICTIONS or alternative explanations, and state what you would recommend the panel do with this hypothesis. When a point draws on the retrieved evidence above, cite its URL or PMID in parentheses at the end of the point.

Output ONLY valid JSON (no markdown fences):
{{
  "contradicting_points": ["..."],
  "opponent_recommendation": "accept|revise|reject"
}}"#
        );
        let opp = complete_json_with_retry(
            self.opponent.as_ref(),
            "You are the most rigorous scientific skeptic. Find every flaw. Output ONLY valid JSON.",
            &opponent_prompt,
            "opponent",
            cancel.clone(),
        )
        .await?;
        let contradicting_points = str_array(&opp, "contradicting_points");
        let opponent_recommendation = opp
            .get("opponent_recommendation")
            .and_then(|v| v.as_str())
            .map(Verdict::parse);
        let opponent_case = if contradicting_points.is_empty() {
            "(opponent offered no points)".to_string()
        } else {
            contradicting_points
                .iter()
                .map(|s| format!("  - {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // 3. Rebuttal — the Proposer answers the Opponent's strongest
        // objections so the Judge sees a genuine two-round exchange instead
        // of two unchallenged monologues. Degrade to no rebuttal on failure:
        // a missing second round beats losing the whole debate.
        let mut rebuttal_points: Vec<String> = Vec::new();
        if !contradicting_points.is_empty() {
            let rebuttal_prompt = format!(
                r#"You are the PROPOSER (正方) in the rebuttal round of a scientific debate. The opponent has attacked your hypothesis. Answer each attack head-on: concede what must be conceded, refute what the literature contradicts, and say honestly what additional evidence would settle the point.

{ctx}
{evidence_block}

**The opponent attacked:**
{opponent_case}

**Your task:** for the 2-4 strongest attacks, give your rebuttal. When a rebuttal draws on the retrieved evidence above, cite its URL or PMID in parentheses at the end of the point.

Output ONLY valid JSON (no markdown fences):
{{
  "rebuttal_points": ["..."]
}}"#
            );
            if let Ok(rep) = complete_json_with_retry(
                self.proposer.as_ref(),
                "You are a rigorous scientific proposer. Output ONLY valid JSON.",
                &rebuttal_prompt,
                "proposer rebuttal",
                cancel.clone(),
            )
            .await
            {
                rebuttal_points = str_array(&rep, "rebuttal_points");
            }
        }
        let rebuttal_case = if rebuttal_points.is_empty() {
            "(no rebuttal offered)".to_string()
        } else {
            rebuttal_points
                .iter()
                .map(|s| format!("  - {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // 4. Judge — weigh both sides (and the rebuttal), rule, leave notes.
        let judge_prompt = format!(
            r#"You are the JUDGE (裁判) of a scientific debate. Weigh the proposer's case and rebuttal against the opponent's critique and rule on the hypothesis.

{ctx}
{evidence_block}

**Proposer's case:**
{proposer_case}

**Opponent's critique:**
{opponent_case}

**Proposer's rebuttal:**
{rebuttal_case}

**Your task:**
1. `verdict` — `accept` (evidence holds), `revise` (plausible but has weaknesses to address), or `reject` (contradictions / no evidence are decisive).
2. `confidence_after` — your confidence in `[0,1]` after weighing both sides.
3. `refinement_notes` — concrete, specific suggestions for how to improve or qualify the hypothesis (empty string if `accept`).

Output ONLY valid JSON (no markdown fences):
{{
  "verdict": "accept|revise|reject",
  "confidence_after": 0.0,
  "refinement_notes": "..."
}}"#
        );
        let verdict_json = complete_json_with_retry(
            self.judge.as_ref(),
            "You are an impartial scientific judge. Output ONLY valid JSON.",
            &judge_prompt,
            "judge verdict",
            cancel,
        )
        .await?;

        Ok(HypothesisVerdict {
            hypothesis_id: h.id,
            verdict: Verdict::parse(
                verdict_json
                    .get("verdict")
                    .and_then(|v| v.as_str())
                    .unwrap_or("revise"),
            ),
            supporting_points,
            contradicting_points,
            confidence_after: verdict_json
                .get("confidence_after")
                .and_then(|v| v.as_f64())
                .unwrap_or(proposer_confidence)
                .clamp(0.0, 1.0),
            refinement_notes: verdict_json
                .get("refinement_notes")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            opponent_recommendation,
            rebuttal_points,
        })
    }

    // ── Phase B ───────────────────────────────────────────────────
    async fn compare(
        &self,
        hypotheses: &[Hypothesis],
        verdicts: &[HypothesisVerdict],
        kg: &KnowledgeGraph,
        cancel: CancellationToken,
    ) -> Result<CrossComparison, AgentError> {
        let roster: Vec<String> = hypotheses
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let v = verdicts.get(i);
                format!(
                    "- H{} [id={}]: {}\n   verdict={:?}, confidence_after={:.2}\n   contradictions: {}",
                    i + 1,
                    h.id,
                    h.statement,
                    v.map(|x| x.verdict).unwrap_or(Verdict::Revise),
                    v.map(|x| x.confidence_after).unwrap_or(h.confidence),
                    v.and_then(|x| x.contradicting_points.first().cloned())
                        .unwrap_or_else(|| "(none)".into()),
                )
            })
            .collect();
        let _ = kg; // KG available for future grounding; roster already summarizes.
        let roster = roster.join("\n");

        let prompt = format!(
            r#"You are a scientific judge comparing competing hypotheses about the same disease. Below are the hypotheses with their individual debate verdicts.

{roster}

**Your task — compare them against each other:**
1. `contradictions_between` — pairs of hypotheses that are in tension or mutually exclusive; give `{{"a": "<id>", "b": "<id>", "reason": "..."}}` for each.
2. `ranking_rationale` — a short justification for ordering them by overall evidence strength.
3. `strongest_id` — the id of the single strongest hypothesis (or null).
4. `merge_suggestions` — concrete ideas to merge / drop / combine hypotheses into a tighter set.

Output ONLY valid JSON (no markdown fences):
{{
  "contradictions_between": [{{"a":"<uuid>","b":"<uuid>","reason":"..."}}],
  "ranking_rationale": "...",
  "strongest_id": "<uuid> or null",
  "merge_suggestions": ["..."]
}}"#
        );

        let root = complete_json_with_retry(
            self.judge.as_ref(),
            "You are a precise scientific judge. Output ONLY valid JSON.",
            &prompt,
            "comparison",
            cancel,
        )
        .await?;

        let contradictions = root
            .get("contradictions_between")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        Some(ContradictionPair {
                            a: uuid_str(v.get("a")?).or_else(|| parse_index_uuid(v.get("a")?, hypotheses))?,
                            b: uuid_str(v.get("b")?).or_else(|| parse_index_uuid(v.get("b")?, hypotheses))?,
                            reason: v.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let strongest_id = root
            .get("strongest_id")
            .and_then(|v| match v {
                serde_json::Value::Null => None,
                _ => uuid_str(v).or_else(|| parse_index_uuid(v, hypotheses)),
            });

        Ok(CrossComparison {
            contradictions_between: contradictions,
            ranking_rationale: root
                .get("ranking_rationale")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            strongest_id,
            merge_suggestions: str_array(&root, "merge_suggestions"),
        })
    }

    // ── Phase C ───────────────────────────────────────────────────
    async fn refine_batch(
        &self,
        to_refine: &[(Hypothesis, HypothesisVerdict)],
        kg: &KnowledgeGraph,
        cancel: CancellationToken,
    ) -> Result<Vec<Hypothesis>, AgentError> {
        if to_refine.is_empty() {
            return Ok(vec![]);
        }
        let roster: Vec<String> = to_refine
            .iter()
            .map(|(h, v)| {
                format!(
                    "- id={}: {}\n   mechanism: {}\n   refinement_notes: {}",
                    h.id,
                    h.statement,
                    h.mechanism.as_deref().unwrap_or("(none)"),
                    v.refinement_notes,
                )
            })
            .collect();
        let roster = roster.join("\n");
        let _ = kg;

        let prompt = format!(
            r#"You are a senior researcher refining hypotheses in light of a debate. For each hypothesis below, incorporate the refinement notes to produce an improved version.

{roster}

For EACH hypothesis (keyed by its `id`), output a refined object with:
- `statement` — the improved, more precise / better-qualified hypothesis.
- `mechanism` — an updated mechanism.
- `supporting_evidence` — 1-3 strengthened supporting points.
- `counter_evidence` — 1-3 acknowledged caveats.
- `confidence` — refined confidence in `[0,1]`.

Output ONLY valid JSON (no markdown fences):
{{
  "refined": [
    {{"id":"<uuid>","statement":"...","mechanism":"...","supporting_evidence":["..."],"counter_evidence":["..."],"confidence":0.0}}
  ]
}}"#
        );

        let root = complete_json_with_retry(
            self.judge.as_ref(),
            "You are a precise scientific reasoning engine. Output ONLY valid JSON.",
            &prompt,
            "refinement",
            cancel,
        )
        .await?;

        let arr = root
            .get("refined")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AgentError::invalid_config("debate refinement: missing 'refined' array"))?;

        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let Some(id) = item.get("id").and_then(uuid_str) else { continue };
            // Locate the original to preserve source_candidate / id.
            let Some((orig, _)) = to_refine.iter().find(|(h, _)| h.id == id) else {
                continue;
            };
            let mut r = orig.clone();
            if let Some(s) = item.get("statement").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    r.statement = s.to_string();
                }
            }
            if let Some(m) = item.get("mechanism").and_then(|v| v.as_str()) {
                if !m.is_empty() {
                    r.mechanism = Some(m.to_string());
                }
            }
            let sup = str_array(item, "supporting_evidence");
            if !sup.is_empty() {
                r.supporting_evidence = sup;
            }
            let con = str_array(item, "counter_evidence");
            if !con.is_empty() {
                r.counter_evidence = con;
            }
            if let Some(c) = item.get("confidence").and_then(|v| v.as_f64()) {
                r.confidence = c.clamp(0.0, 1.0);
            }
            out.push(r);
        }
        Ok(out)
    }
}

// ───────────────────────────── helpers ─────────────────────────────

/// Call a provider and return the concatenated text content.
async fn complete_json(
    provider: &dyn LlmProvider,
    system: &str,
    prompt: &str,
    cancel: CancellationToken,
) -> Result<String, AgentError> {
    let request = CompletionRequest {
        system: system.into(),
        messages: vec![Message::user(prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.2),
            // 3500 truncated multi-hypothesis refinements mid-object (parse
            // failures at ~line 63); refinement payloads are long but the
            // per-model cap is far higher.
            max_tokens: Some(8000),
            ..Default::default()
        },
    };
    let response = provider.complete(&request, cancel).await?;
    Ok(response
        .content
        .iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(""))
}

/// Complete + parse JSON with one corrective retry: reasoning models
/// occasionally emit slightly malformed JSON (stray quotes), and one bad
/// field must not kill a whole debate phase.
async fn complete_json_with_retry(
    provider: &dyn LlmProvider,
    system: &str,
    prompt: &str,
    what: &str,
    cancel: CancellationToken,
) -> Result<serde_json::Value, AgentError> {
    let text = complete_json(provider, system, prompt, cancel.clone()).await?;
    let repaired = json_util::extract_and_repair(&text);
    match serde_json::from_str(&repaired) {
        Ok(v) => Ok(v),
        Err(first_err) => {
            tracing::warn!("debate {what} parse failed ({first_err}); retrying");
            let retry_prompt = format!(
                "{prompt}\n\nYour previous output was NOT valid JSON ({first_err}). \
                 Re-output the same content as STRICTLY valid JSON. Escape all quotes \
                 inside strings; output ONLY the JSON object."
            );
            let text = complete_json(provider, system, &retry_prompt, cancel).await?;
            let repaired = json_util::extract_and_repair(&text);
            match serde_json::from_str(&repaired) {
                Ok(v) => Ok(v),
                Err(e) => {
                    // Truncation salvage: repeatedly drop the trailing
                    // incomplete fragment and re-close. Refinement payloads
                    // are per-item arrays, so a partial object beats losing
                    // the whole debate phase.
                    if let Some(v) = salvage_truncated(&text) {
                        tracing::warn!("debate {what}: salvaged partial JSON after retry parse failed ({e})");
                        Ok(v)
                    } else {
                        Err(AgentError::invalid_config(format!("debate {what} parse failed: {e}")))
                    }
                }
            }
        }
    }
}

/// Last index (byte) of a `,` or `}` that sits outside any string literal.
/// Cutting there leaves a structurally prefix-complete JSON fragment.
fn last_cut_index(s: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escape_next = false;
    let mut last: Option<usize> = None;
    for (idx, ch) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if !in_string && (ch == ',' || ch == '}') {
            last = Some(idx);
        }
    }
    last
}

/// Salvage a parseable JSON value from truncated model output: iteratively
/// cut at the last structural `,`/`}`, re-close open brackets/strings
/// (`fix_truncated_json`), and retry the parse. Returns the first prefix
/// that parses, or `None` when even the empty prefix fails.
fn salvage_truncated(text: &str) -> Option<serde_json::Value> {
    let mut cur = json_util::extract_and_repair(text);
    for _ in 0..16 {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cur) {
            return Some(v);
        }
        let cut = last_cut_index(&cur)?;
        // Guarantee strict progress: the cut must shrink the fragment.
        let mut next = cur[..cut].to_string();
        while next.ends_with(',') {
            next.pop();
        }
        if next.len() >= cur.len() {
            return None;
        }
        cur = json_util::fix_truncated_json(&next);
    }
    serde_json::from_str::<serde_json::Value>(&cur).ok()
}

/// Render a readable, self-contained context block for a hypothesis (mirrors
/// the generator's rendering so the debate sees the same KG evidence).
fn render_hypothesis_context(h: &Hypothesis, kg: &KnowledgeGraph) -> String {
    let head_name = kg
        .get_entity(&h.source_candidate.head)
        .map(|e| e.name.clone())
        .unwrap_or_else(|| "unknown".into());
    let tail_name = kg
        .get_entity(&h.source_candidate.tail)
        .map(|e| e.name.clone())
        .unwrap_or_else(|| "unknown".into());
    let rel_name = format!("{:?}", h.source_candidate.relation).to_lowercase();

    let paths_text = if h.source_candidate.evidence.supporting_paths.is_empty() {
        "(no explicit graph paths)".to_string()
    } else {
        h.source_candidate
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
                format!("  Path {}: {}", i + 1, steps.join(" → "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let supporting = if h.supporting_evidence.is_empty() {
        "(none enumerated)".to_string()
    } else {
        h.supporting_evidence
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let counter = if h.counter_evidence.is_empty() {
        "(none enumerated)".to_string()
    } else {
        h.counter_evidence
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "**Hypothesis:** {stmt}\n\
         **Proposed mechanism:** {mech}\n\
         **Graph relationship:** {head} --[{rel}]--> {tail}  (algorithm confidence {score:.3}, novelty {novelty:?})\n\
         **Graph evidence paths:**\n{paths}\n\
         **Previously enumerated supporting evidence:**\n{sup}\n\
         **Previously enumerated counter-evidence:**\n{con}",
        stmt = h.statement,
        mech = h.mechanism.as_deref().unwrap_or("(not specified)"),
        head = head_name,
        rel = rel_name,
        tail = tail_name,
        score = h.source_candidate.score,
        novelty = h.novelty,
        paths = paths_text,
        sup = supporting,
        con = counter,
    )
}

fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a UUID from a JSON string value.
fn uuid_str(v: &serde_json::Value) -> Option<uuid::Uuid> {
    v.as_str().and_then(|s| uuid::Uuid::parse_str(s).ok())
}

/// Tolerate models that answer with a 1-based index ("1", "H1") instead of a
/// full UUID by mapping it back to the corresponding hypothesis id.
fn parse_index_uuid(v: &serde_json::Value, hypotheses: &[Hypothesis]) -> Option<uuid::Uuid> {
    let s = v.as_str()?;
    let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).filter(|c| c.is_ascii_digit()).collect();
    let idx: usize = digits.parse().ok()?;
    if idx >= 1 && idx <= hypotheses.len() {
        Some(hypotheses[idx - 1].id)
    } else {
        None
    }
}

// ─────────────────────── audit persistence ───────────────────────

/// Write a human-readable `debate_report.json` for auditability.
///
/// Entity names are resolved from the KG so the report is self-contained.
pub fn persist_debate_report(
    outcome: &DebateOutcome,
    kg: &KnowledgeGraph,
    path: &Path,
) -> Result<(), AgentError> {
    let per: Vec<serde_json::Value> = outcome
        .per_hypothesis
        .iter()
        .map(|v| {
            serde_json::json!({
                "hypothesis_id": v.hypothesis_id,
                "verdict": v.verdict,
                "confidence_after": v.confidence_after,
                "supporting_points": v.supporting_points,
                "contradicting_points": v.contradicting_points,
                "rebuttal_points": v.rebuttal_points,
                "refinement_notes": v.refinement_notes,
                "opponent_recommendation": v.opponent_recommendation,
            })
        })
        .collect();

    let refined: Vec<serde_json::Value> = outcome
        .refined
        .iter()
        .map(|h| {
            let head = kg.get_entity(&h.source_candidate.head).map(|e| e.name.clone()).unwrap_or_default();
            let tail = kg.get_entity(&h.source_candidate.tail).map(|e| e.name.clone()).unwrap_or_default();
            serde_json::json!({
                "id": h.id,
                "statement": h.statement,
                "mechanism": h.mechanism,
                "confidence": h.confidence,
                "head": head,
                "tail": tail,
            })
        })
        .collect();

    let mut comparison = serde_json::to_value(&outcome.comparison)
        .unwrap_or(serde_json::Value::Null);
    // Alias consumed by the report generator (`strongest_hypothesis`);
    // the canonical field remains `strongest_id`.
    if let Some(id) = outcome.comparison.strongest_id {
        comparison["strongest_hypothesis"] = serde_json::json!(id.to_string());
    }

    let report = serde_json::json!({
        "rounds": outcome.rounds,
        "per_hypothesis": per,
        "comparison": comparison,
        "refined": refined,
    });

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AgentError::Checkpoint(format!("create debate report dir: {e}")))?;
    }
    let pretty = serde_json::to_string_pretty(&report)
        .map_err(|e| AgentError::Checkpoint(format!("serialize debate report: {e}")))?;
    std::fs::write(path, pretty)
        .map_err(|e| AgentError::Checkpoint(format!("write debate report: {e}")))?;
    Ok(())
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::HypothesisNovelty;
    use miniagent_core::event::{ContentBlock, StopReason};
    use miniagent_core::event::{Usage};
    use miniagent_kg::link_prediction::{
        HypothesisCandidate, HypothesisEvidence, HypothesisNovelty as KgNovelty,
    };
    use miniagent_kg::schema::{Entity, EntityId, EntityType, RelationType};
    use miniagent_provider::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A stub provider that returns successive canned responses per call.
    struct SeqProvider {
        responses: Vec<String>,
        call: Arc<AtomicUsize>,
    }
    impl SeqProvider {
        fn new(responses: Vec<String>) -> Self {
            Self { responses, call: Arc::new(AtomicUsize::new(0)) }
        }
    }
    #[async_trait]
    impl LlmProvider for SeqProvider {
        async fn complete(
            &self,
            _req: &CompletionRequest,
            _cancel: CancellationToken,
        ) -> Result<CompletionResponse, AgentError> {
            let i = self.call.fetch_add(1, Ordering::SeqCst);
            let text = self.responses.get(i).cloned().unwrap_or_else(|| "{}".into());
            Ok(CompletionResponse {
                content: vec![ContentBlock::Text { text }],
                usage: Usage { input_tokens: 1, output_tokens: 1, cache_creation_input_tokens: None, cache_read_input_tokens: None },
                stop_reason: StopReason::EndTurn,
            })
        }
        async fn stream(&self, _req: &CompletionRequest, _cancel: CancellationToken) -> Result<StreamResponse, AgentError> {
            Err(AgentError::internal("stub"))
        }
    }

    fn kg_with(disease: &str, gene: &str) -> (KnowledgeGraph, EntityId, EntityId) {
        let mut kg = KnowledgeGraph::new();
        let d = EntityId::new();
        let g = EntityId::new();
        kg.add_entity(Entity { id: d, name: disease.into(), entity_type: EntityType::Disease, aliases: vec![], metadata: serde_json::json!({}) });
        kg.add_entity(Entity { id: g, name: gene.into(), entity_type: EntityType::Gene, aliases: vec![], metadata: serde_json::json!({}) });
        (kg, d, g)
    }

    fn hyp(id: uuid::Uuid, stmt: &str, score: f64, head: EntityId, tail: EntityId) -> Hypothesis {
        Hypothesis {
            id,
            statement: stmt.into(),
            mechanism: Some("m".into()),
            novelty: HypothesisNovelty::Novel,
            confidence: 0.5,
            supporting_evidence: vec!["s1".into()],
            counter_evidence: vec![],
            experimental_design: None,
            source_candidate: HypothesisCandidate {
                head,
                relation: RelationType::AssociatedWith,
                tail,
                score,
                evidence: HypothesisEvidence {
                    kge_score: score,
                    path_score: 0.0,
                    give_score: 0.0,
                    supporting_paths: vec![],
                    novelty: KgNovelty::Novel,
                },
            },
        }
    }

    #[test]
    fn verdict_parse_handles_variants() {
        assert_eq!(Verdict::parse("Accept"), Verdict::Accept);
        assert_eq!(Verdict::parse("REJECTED"), Verdict::Reject);
        assert_eq!(Verdict::parse("revise"), Verdict::Revise);
        assert_eq!(Verdict::parse("nonsense"), Verdict::Revise);
    }

    #[tokio::test]
    async fn empty_input_is_noop() {
        let debater = HypothesisDebater::new(
            Box::new(SeqProvider::new(vec![])),
            Box::new(SeqProvider::new(vec![])),
            Box::new(SeqProvider::new(vec![])),
        );
        let kg = KnowledgeGraph::new();
        let out = debater.debate_and_refine(&[], &kg, CancellationToken::new()).await.unwrap();
        assert!(out.refined.is_empty());
        assert!(out.per_hypothesis.is_empty());
        assert_eq!(out.rounds, 0);
    }

    #[tokio::test]
    async fn rejects_dropped_from_refined_but_kept_in_audit() {
        let (kg, d, g) = kg_with("Alzheimer", "APOE");
        let h = hyp(uuid::Uuid::new_v4(), "APOE causes AD", 0.8, d, g);
        let prop = r#"{"supporting_points":[],"proposer_confidence":0.4}"#;
        let opp = r#"{"contradicting_points":["no causality"],"opponent_recommendation":"reject"}"#;
        let judge = r#"{"verdict":"reject","confidence_after":0.1,"refinement_notes":""}"#;
        let debater = HypothesisDebater::new(
            Box::new(SeqProvider::new(vec![prop.into()])),
            Box::new(SeqProvider::new(vec![opp.into()])),
            Box::new(SeqProvider::new(vec![judge.into()])),
        );
        let out = debater.debate_and_refine(&[h], &kg, CancellationToken::new()).await.unwrap();
        assert_eq!(out.per_hypothesis.len(), 1);
        assert_eq!(out.per_hypothesis[0].verdict, Verdict::Reject);
        // Opponent recommendation is recorded for audit even when the judge
        // agrees.
        assert_eq!(out.per_hypothesis[0].opponent_recommendation, Some(Verdict::Reject));
        assert_eq!(out.per_hypothesis[0].contradicting_points, vec!["no causality".to_string()]);
        assert!(out.refined.is_empty(), "rejected hypothesis excluded from refined");
    }

    #[tokio::test]
    async fn accept_passes_through_with_bumped_confidence() {
        let (kg, d, g) = kg_with("Alzheimer", "APOE");
        let id = uuid::Uuid::new_v4();
        let h = hyp(id, "APOE causes AD", 0.8, d, g);
        let prop = r#"{"supporting_points":["gwas"],"proposer_confidence":0.9}"#;
        let opp = r#"{"contradicting_points":[],"opponent_recommendation":"accept"}"#;
        let judge = r#"{"verdict":"accept","confidence_after":0.92,"refinement_notes":""}"#;
        let debater = HypothesisDebater::new(
            Box::new(SeqProvider::new(vec![prop.into()])),
            Box::new(SeqProvider::new(vec![opp.into()])),
            Box::new(SeqProvider::new(vec![judge.into()])),
        );
        let out = debater.debate_and_refine(&[h], &kg, CancellationToken::new()).await.unwrap();
        assert_eq!(out.refined.len(), 1);
        assert!((out.refined[0].confidence - 0.92).abs() < 1e-9);
        assert_eq!(out.per_hypothesis[0].verdict, Verdict::Accept);
        assert_eq!(out.per_hypothesis[0].supporting_points, vec!["gwas".to_string()]);
    }

    #[tokio::test]
    async fn multi_hypothesis_debate_compare_refine() {
        let (kg, d, g) = kg_with("Alzheimer", "APOE");
        let h1 = hyp(uuid::Uuid::new_v4(), "APOE4 drives amyloid", 0.7, d, g);
        let h2 = hyp(uuid::Uuid::new_v4(), "Tau phosphorylation is primary", 0.6, d, g);
        let id1 = h1.id;
        let id2 = h2.id;

        // Proposer / Opponent / Judge each answer per hypothesis (2×), then
        // the judge additionally handles comparison (Phase B) and refinement
        // (Phase C).
        let prop = SeqProvider::new(vec![
            r#"{"supporting_points":["gwas"],"proposer_confidence":0.8}"#.into(),
            r#"{"supporting_points":["pathology"],"proposer_confidence":0.7}"#.into(),
        ]);
        let opp = SeqProvider::new(vec![
            r#"{"contradicting_points":["tau"],"opponent_recommendation":"revise"}"#.into(),
            r#"{"contradicting_points":[],"opponent_recommendation":"accept"}"#.into(),
        ]);
        let b = format!(
            r#"{{"contradictions_between":[{{"a":"{id1}","b":"{id2}","reason":"amyloid vs tau cascade"}}],"ranking_rationale":"both plausible","strongest_id":"{id1}","merge_suggestions":["test interaction"]}}"#,
        );
        let c = format!(
            r#"{{"refined":[{{"id":"{id1}","statement":"APOE4 drives early amyloid","mechanism":"lipid","supporting_evidence":["gwas2"],"counter_evidence":["tau"],"confidence":0.78}}]}}"#,
        );
        let judge = SeqProvider::new(vec![
            r#"{"verdict":"revise","confidence_after":0.65,"refinement_notes":"qualify to early stage"}"#.into(),
            r#"{"verdict":"accept","confidence_after":0.7,"refinement_notes":""}"#.into(),
            b,
            c,
        ]);

        let debater = HypothesisDebater::new(Box::new(prop), Box::new(opp), Box::new(judge));
        let out = debater.debate_and_refine(&[h1, h2], &kg, CancellationToken::new()).await.unwrap();

        assert_eq!(out.per_hypothesis.len(), 2);
        assert_eq!(out.comparison.contradictions_between.len(), 1);
        assert_eq!(out.comparison.strongest_id, Some(id1));
        // Both hypotheses survive (revise + accept), refine applied to h1.
        assert_eq!(out.refined.len(), 2);
        let refined_h1 = out.refined.iter().find(|h| h.id == id1).unwrap();
        assert!(refined_h1.statement.contains("early amyloid"));
        assert!((refined_h1.confidence - 0.78).abs() < 1e-9);
        // Re-ranked by confidence descending: h1 (0.78) before h2 (0.7).
        assert_eq!(out.refined[0].id, id1);
    }

    #[tokio::test]
    async fn comparison_tolerates_index_strongest_id() {
        let (kg, d, g) = kg_with("Flu", "GeneX");
        let h1 = hyp(uuid::Uuid::new_v4(), "A", 0.7, d, g);
        let h2 = hyp(uuid::Uuid::new_v4(), "B", 0.6, d, g);
        let id1 = h1.id;
        let prop = SeqProvider::new(vec![
            r#"{"supporting_points":[],"proposer_confidence":0.7}"#.into(),
            r#"{"supporting_points":[],"proposer_confidence":0.6}"#.into(),
        ]);
        let opp = SeqProvider::new(vec![
            r#"{"contradicting_points":[],"opponent_recommendation":"accept"}"#.into(),
            r#"{"contradicting_points":[],"opponent_recommendation":"accept"}"#.into(),
        ]);
        let judge = SeqProvider::new(vec![
            r#"{"verdict":"accept","confidence_after":0.7,"refinement_notes":""}"#.into(),
            r#"{"verdict":"accept","confidence_after":0.6,"refinement_notes":""}"#.into(),
            // Model says strongest_id = "H1" instead of a uuid.
            r#"{"contradictions_between":[],"ranking_rationale":"x","strongest_id":"H1","merge_suggestions":[]}"#.into(),
        ]);
        let debater = HypothesisDebater::new(Box::new(prop), Box::new(opp), Box::new(judge));
        let out = debater.debate_and_refine(&[h1, h2], &kg, CancellationToken::new()).await.unwrap();
        assert_eq!(out.comparison.strongest_id, Some(id1));
    }

    #[test]
    fn persist_report_writes_valid_json() {
        let (kg, d, g) = kg_with("AD", "APOE");
        let id = uuid::Uuid::new_v4();
        let out = DebateOutcome {
            per_hypothesis: vec![HypothesisVerdict {
                hypothesis_id: id,
                verdict: Verdict::Accept,
                supporting_points: vec!["x".into()],
                contradicting_points: vec![],
                confidence_after: 0.9,
                refinement_notes: "".into(),
                opponent_recommendation: None,
                rebuttal_points: vec![],
            }],
            comparison: CrossComparison { strongest_id: Some(id), ..Default::default() },
            refined: vec![hyp(id, "APOE drives AD", 0.8, d, g)],
            rounds: 1,
        };
        let dir = std::env::temp_dir().join("miniagent_debate_report_test");
        let path = dir.join("debate_report.json");
        persist_debate_report(&out, &kg, &path).unwrap();
        let txt = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["rounds"], 1);
        assert_eq!(v["per_hypothesis"][0]["verdict"], "accept");
        assert_eq!(v["refined"][0]["head"], "AD");
        assert_eq!(v["refined"][0]["tail"], "APOE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn salvage_truncated_recovers_prefix_items() {
        // Truncated mid-string in the 2nd item: the 1st item must survive
        // (stack-ordered closers keep the 2nd object structurally valid too,
        // just with fewer fields — per-item parsing skips bad ids anyway).
        let truncated = r#"{"refined": [
            {"id":"aaa-bbb","statement":"hypothesis one","mechanism":"m1","supporting_evidence":["e1"],"counter_evidence":[],"confidence":0.7},
            {"id":"ccc-ddd","statement":"hyp"#;
        let v = salvage_truncated(truncated).expect("should salvage");
        let arr = v["refined"].as_array().unwrap();
        assert!(!arr.is_empty());
        assert_eq!(arr[0]["id"], "aaa-bbb");
        assert_eq!(arr[0]["statement"], "hypothesis one");
    }

    #[test]
    fn salvage_truncated_gives_up_on_garbage() {
        assert!(salvage_truncated("no json here at all").is_none());
    }
}
