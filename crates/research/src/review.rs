//! Final-report review & verification (the "safety net" stage).
//!
//! After the user-facing report (`{brief}.md`) is written, this module runs
//! a two-layer audit of the report against the run's underlying artifacts:
//!
//! 1. **Mechanical cross-check** (always runs, no LLM): re-derives the key
//!    counts and file existence claims from `ProjectManifest` / the artifact
//!    files on disk and compares them to what the report states. This layer
//!    is deterministic and survives total provider outage.
//! 2. **LLM structured review** (best-effort): asks a provider (with the
//!    cross-family fallback list) to verify the report's section coverage
//!    and internal consistency, outputting a strict JSON verdict
//!    (pass / pass_with_warnings / fail) plus evidence-backed issues.
//!
//! Everything lands in three auditable places:
//! - `report_review.json` — the structured review record (checks + issues),
//! - the report itself — an appended "报告审核" section with the verdict,
//! - `project.json` — a `report_review` event + a `review` stage record.
//!
//! Design principles (from biomni / biomedical-agent practice): every issue
//! carries evidence and a suggestion; a review failure NEVER deletes or
//! rewrites the report (append-only); the mechanical layer is the floor.

use miniagent_core::json_util;
use miniagent_provider::traits::{CompletionRequest, LlmProvider};
use serde::Serialize;
use std::path::Path;

/// Status of one review check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Mismatch,
    Missing,
}

/// One mechanical or LLM-reported check.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewCheck {
    pub item: String,
    pub status: CheckStatus,
    /// What the report claims vs. what the artifacts say.
    pub detail: String,
    /// Where the ground truth came from (manifest field / file path).
    pub evidence: String,
}

/// One actionable review issue.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewIssue {
    pub severity: String, // high | medium | low
    pub description: String,
    pub suggestion: String,
}

/// Structured review verdict for the whole report.
#[derive(Debug, Clone, Serialize)]
pub struct ReportReview {
    pub verdict: String, // pass | pass_with_warnings | fail
    pub summary: String,
    pub checks: Vec<ReviewCheck>,
    pub issues: Vec<ReviewIssue>,
    pub reviewer: String, // "mechanical" | "mechanical+llm"
}

/// Inputs the review needs: the report text plus the ground-truth facts
/// extracted from the manifest (already serialized by the caller).
pub struct ReviewContext {
    /// The user's original research request, for topic-coherence review.
    pub question: String,
    pub report_markdown: String,
    /// `"hypotheses": 5, "validation_plans": 2, ...` style key facts.
    pub facts: Vec<(String, String)>,
}

/// Extract the ground-truth facts a review should check against.
pub fn collect_facts(manifest: &crate::ProjectManifest) -> Vec<(String, String)> {
    let mut facts = Vec::new();
    facts.push((
        "hypotheses".into(),
        manifest.hypotheses.len().to_string(),
    ));
    facts.push((
        "validation_plans".into(),
        manifest.validation_plans.len().to_string(),
    ));
    facts.push(("analyses".into(), manifest.analyses.len().to_string()));
    // Per-outcome counts are NOT free-text-audited: words like "failed in
    // Phase 3" describe trial outcomes, not task counts, and keyword-window
    // matching misfires on them (live: Riluzole "failed in Phase 3" ⇒ fake
    // mismatch). Per-task delivery status is verified structurally instead.
    let ok = manifest.analyses.iter().filter(|a| a.success).count();
    facts.push(("analyses_succeeded".into(), ok.to_string()));
    facts.push((
        "debate_report_present".into(),
        manifest
            .debate_report
            .as_ref()
            .map(|_| "yes".to_string())
            .unwrap_or_else(|| "no".to_string()),
    ));
    facts
}

/// Layer 1 — deterministic cross-checks of report text against facts.
///
/// Number claims are verified only where the report states a count next to a
/// recognizable keyword; missing mentions are `Missing` (informational), not
/// failures — the LLM layer judges whether the omission matters.
pub fn mechanical_checks(ctx: &ReviewContext) -> Vec<ReviewCheck> {
    let mut checks = Vec::new();
    let report = &ctx.report_markdown;

    // Windowed number extraction: a count claim puts the number right next
    // to its noun ("5 条假说" / "5 hypotheses" / "假说: 5"), so only numbers
    // within `WINDOW` chars of a keyword occurrence count as candidates.
    // Grabbing the first number of the whole line matched PMIDs/years and
    // produced false mismatches (caught in the first live review run).
    const WINDOW: usize = 14;
    let candidates_for = |keywords: &[&str]| -> Vec<(String, Vec<String>)> {
        let mut hits: Vec<(String, Vec<String>)> = Vec::new();
        for line in report.lines() {
            let lower = line.to_lowercase();
            for kw in keywords {
                let mut from = 0;
                while let Some(pos) = lower[from..].find(kw) {
                    let start = from + pos;
                    let w_start = start.saturating_sub(WINDOW);
                    let w_end = (start + kw.len() + WINDOW).min(line.len());
                    // Char-boundary-safe window slice.
                    let mut ws = w_start;
                    while ws < w_end && !line.is_char_boundary(ws) {
                        ws += 1;
                    }
                    let mut we = w_end;
                    while we < line.len() && !line.is_char_boundary(we) {
                        we += 1;
                    }
                    let window = &line[ws..we];
                    let nums: Vec<String> = digit_count_candidates(window);
                    if !nums.is_empty() {
                        hits.push((window.trim().to_string(), nums));
                    }
                    from = start + kw.len();
                }
            }
        }
        hits
    };

    for (key, actual) in &ctx.facts {
        let keywords: &[&str] = match key.as_str() {
            "hypotheses" => &["hypotheses", "假说"],
            "validation_plans" => &["validation plan", "验证计划"],
            "analyses" => &["analyses", "数据分析任务", "数据分析"],
            "analyses_succeeded" => &["succeeded", "成功"],
            "debate_report_present" => &["debate", "辩论"],
            _ => &[key.as_str()],
        };
        let actual_num = actual.parse::<u64>().ok();
        match actual_num {
            None => {
                // Non-numeric fact (yes/no): presence check only.
                let found = report.to_lowercase().contains(keywords[0])
                    || keywords[1..].iter().any(|kw| report.contains(kw));
                checks.push(ReviewCheck {
                    item: format!("report mentions '{key}'"),
                    status: if found { CheckStatus::Ok } else { CheckStatus::Missing },
                    detail: format!("expected value = {actual}"),
                    evidence: "project.json manifest".into(),
                });
            }
            Some(actual_val) => {
                let hits = candidates_for(keywords);
                if hits
                    .iter()
                    .any(|(_, nums)| nums.iter().any(|n| n == &actual_val.to_string()))
                {
                    checks.push(ReviewCheck {
                        item: format!("report count for '{key}'"),
                        status: CheckStatus::Ok,
                        detail: format!("report matches manifest value {actual_val}"),
                        evidence: "project.json manifest".into(),
                    });
                } else if let Some((window, nums)) = hits.first() {
                    checks.push(ReviewCheck {
                        item: format!("report count for '{key}'"),
                        status: CheckStatus::Mismatch,
                        detail: format!(
                            "near '{window}' report shows {:?}, manifest says {actual_val}",
                            nums
                        ),
                        evidence: "project.json manifest".into(),
                    });
                } else {
                    checks.push(ReviewCheck {
                        item: format!("report mentions '{key}'"),
                        status: CheckStatus::Missing,
                        detail: format!(
                            "no count near '{keywords:?}' in report; manifest value = {actual_val}"
                        ),
                        evidence: "project.json manifest".into(),
                    });
                }
            }
        }
    }
    checks
}

/// Count-like numbers in a window: digit runs of ≤3 digits (excludes years
/// like 2026), not immediately followed by `)` (excludes list markers like
/// "(6) Failed"), with leading zeros normalized.
fn digit_count_candidates(window: &str) -> Vec<String> {
    let chars: Vec<char> = window.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            if run.len() <= 3 && chars.get(i) != Some(&')') {
                let normalized = run.trim_start_matches('0');
                if !normalized.is_empty() {
                    out.push(normalized.to_string());
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Layer 2 — LLM structured review (best-effort). Walks the fallback list;
/// returns None when no provider can answer, in which case the mechanical
/// review alone stands (recorded with reviewer="mechanical").
pub async fn llm_review(
    ctx: &ReviewContext,
    providers: &[std::sync::Arc<dyn LlmProvider>],
    cancel: tokio_util::sync::CancellationToken,
) -> Option<ReportReview> {
    if providers.is_empty() {
        return None;
    }
    // Cap the report body to keep the review prompt focused and cheap.
    let report_head: String = ctx.report_markdown.chars().take(18_000).collect();
    let facts_text = ctx
        .facts
        .iter()
        .map(|(k, v)| format!("- {k} = {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        r#"You are a strict scientific report auditor. Cross-check the draft research report below against the ground-truth run facts, then output a structured review.

Ground-truth facts (from the audit manifest — authoritative):
{facts_text}

Report draft (may be truncated):
```markdown
{report_head}
```

Check:
0. TOPIC COHERENCE: the report must actually answer the research question above. If the report's hypotheses/datasets/conclusions belong to a different topic, that is a high-severity issue.
1. Every number in the report that matches a fact above must agree.
2. Required sections: 文献综述, 知识图谱, 假说, 辩论, 验证计划, 数据分析交付, 引用索引. Note any missing section.
3. The 数据分析交付 section must state per-task outcomes honestly (failures reported as failures).
4. Flag any unverifiable strong claim ("首次证明", "证实了因果") as an issue.

Output ONLY valid JSON:
{{"verdict": "pass"|"pass_with_warnings"|"fail",
  "summary": "one paragraph",
  "issues": [{{"severity": "high"|"medium"|"low", "description": "...", "suggestion": "..."}}]}}"#
    );
    let request = CompletionRequest {
        system: "You are a rigorous scientific report auditor. Output ONLY valid JSON."
            .into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.0),
            max_tokens: Some(2_000),
            ..Default::default()
        },
    };
    for provider in providers {
        let Ok(resp) = provider.complete(&request, cancel.child_token()).await else {
            continue;
        };
        let text: String = resp
            .content
            .iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let repaired = json_util::extract_and_repair(&text);
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&repaired) else {
            continue;
        };
        let verdict = v["verdict"].as_str().unwrap_or("pass_with_warnings").to_string();
        let summary = v["summary"].as_str().unwrap_or("").to_string();
        let issues = v["issues"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|i| ReviewIssue {
                        severity: i["severity"].as_str().unwrap_or("low").to_string(),
                        description: i["description"].as_str().unwrap_or("").to_string(),
                        suggestion: i["suggestion"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Some(ReportReview {
            verdict,
            summary,
            checks: Vec::new(),
            issues,
            reviewer: "mechanical+llm".into(),
        });
    }
    None
}

/// Combine both layers into the final review record. Verdicts compose
/// conservatively: any high-severity issue or a mechanical mismatch ⇒ at
/// most `pass_with_warnings`; a failed mechanical layer ⇒ `fail`.
pub fn combine(
    mut checks: Vec<ReviewCheck>,
    llm: Option<ReportReview>,
) -> ReportReview {
    let mechanical_bad = checks
        .iter()
        .any(|c| c.status == CheckStatus::Mismatch);
    let mut issues = Vec::new();
    for c in &checks {
        if c.status == CheckStatus::Mismatch {
            issues.push(ReviewIssue {
                severity: "high".into(),
                description: format!("{}: {}", c.item, c.detail),
                suggestion: "以 project.json 的数字为准修正报告相关表述".into(),
            });
        }
    }
    let mut reviewer = "mechanical".to_string();
    let mut llm_summary = String::new();
    if let Some(l) = llm {
        reviewer = "mechanical+llm".into();
        issues.extend(l.issues);
        llm_summary = l.summary;
    }
    let verdict = if mechanical_bad {
        "fail"
    } else if issues.iter().any(|i| i.severity == "high") {
        // LLM high-severity issues (e.g. topic mismatch) mean the report
        // would mislead a reader — same escalation class as a mechanical
        // mismatch. (First live review run flagged a fully off-topic report
        // yet still returned pass_with_warnings; this closes that gap.)
        "fail"
    } else if !issues.is_empty() {
        "pass_with_warnings"
    } else {
        "pass"
    }
    .to_string();
    let summary = if !llm_summary.is_empty() {
        llm_summary
    } else if mechanical_bad {
        "机械校验发现报告数字与审计 manifest 不一致。".to_string()
    } else {
        "机械校验全部通过。".to_string()
    };
    checks.sort_by_key(|c| match c.status {
        CheckStatus::Mismatch | CheckStatus::Missing => 0,
        CheckStatus::Ok => 1,
    });
    ReportReview {
        verdict,
        summary,
        checks,
        issues,
        reviewer,
    }
}

/// Render the review as a markdown section appended to the user report.
pub fn review_markdown_section(review: &ReportReview) -> String {
    use std::fmt::Write;
    let mut md = String::new();
    let icon = match review.verdict.as_str() {
        "pass" => "✅",
        "pass_with_warnings" => "⚠️",
        _ => "❌",
    };
    let _ = writeln!(md, "\n## 报告审核（自动）\n");
    let _ = writeln!(
        md,
        "{icon} **审核结论：`{}`**（审核者：{}）\n",
        review.verdict, review.reviewer
    );
    let _ = writeln!(md, "{}\n", review.summary);
    if !review.checks.is_empty() {
        let _ = writeln!(md, "| 检查项 | 状态 | 说明 |");
        let _ = writeln!(md, "|---|---|---|");
        for c in &review.checks {
            let s = match c.status {
                CheckStatus::Ok => "✅",
                CheckStatus::Mismatch => "❌",
                CheckStatus::Missing => "➖",
            };
            let _ = writeln!(md, "| {} | {} | {} |", c.item, s, c.detail);
        }
        writeln!(md).ok();
    }
    if !review.issues.is_empty() {
        let _ = writeln!(md, "**发现的问题**：\n");
        for i in &review.issues {
            let _ = writeln!(md, "- 【{}】{} 建议：{}", i.severity, i.description, i.suggestion);
        }
    } else {
        let _ = writeln!(md, "未发现需要人工介入的问题。");
    }
    md
}

/// Persist the structured review record next to the report.
pub fn persist_review(dir: &Path, review: &ReportReview) -> std::io::Result<std::path::PathBuf> {
    let path = dir.join("report_review.json");
    let json = serde_json::to_string_pretty(review)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, json)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(report: &str, facts: Vec<(&str, &str)>) -> ReviewContext {
        ReviewContext {
            question: "test question".into(),
            report_markdown: report.into(),
            facts: facts
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn mechanical_checks_catch_count_mismatch() {
        // Chinese report wording, English manifest key (the real shape).
        let ctx = ctx_with(
            "共 **3** 项数据分析任务。",
            vec![("analyses", "8")],
        );
        let checks = mechanical_checks(&ctx);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Mismatch);
        assert!(checks[0].detail.contains("8"));
    }

    #[test]
    fn mechanical_checks_pass_on_match_and_flag_missing() {
        let ctx = ctx_with(
            "共 5 条假说。\n未提任务数。",
            vec![("hypotheses", "5"), ("analyses", "2")],
        );
        let checks = mechanical_checks(&ctx);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert_eq!(checks[1].status, CheckStatus::Missing);
    }

    #[test]
    fn mechanical_checks_ignore_faraway_numbers() {
        // A PMID/year far from the keyword must NOT become a candidate
        // (first live review run produced false mismatches from this).
        let ctx = ctx_with(
            "Phase 6b: 假说辩论 (2026)。2026年5条假说无关。\n关键行：共 5 条假说。",
            vec![("hypotheses", "5")],
        );
        let checks = mechanical_checks(&ctx);
        assert_eq!(checks[0].status, CheckStatus::Ok, "any-line window match wins");
    }

    #[test]
    fn mechanical_checks_nonnumeric_fact_is_presence_only() {
        let ctx = ctx_with(
            "辩论完成，报告见 debate_report.json。",
            vec![("debate_report_present", "yes")],
        );
        let checks = mechanical_checks(&ctx);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn trial_failure_prose_is_not_a_count_mismatch() {
        // Live false positive: "Riluzole (which failed in Phase 3 …)" matched
        // the analyses_failed keyword window. Free-text outcome words must
        // never be audited as task counts.
        let ctx = ctx_with(
            "Existing drugs: Riluzole (which failed in Phase 3 development) shows marginal benefit.",
            vec![("analyses_failed", "0")],
        );
        let checks = mechanical_checks(&ctx);
        assert!(
            checks.iter().all(|c| c.status != CheckStatus::Mismatch),
            "prose about drug-trial failure must not create a count mismatch"
        );
    }

    #[test]
    fn digit_candidates_exclude_list_markers_and_years() {
        // "(6) Failed anti-amyloid" — live false positive from run12.
        assert!(digit_count_candidates("(6) Failed anti-amyloid").is_empty());
        // Years are excluded.
        assert!(digit_count_candidates("假说 (2026)").is_empty());
        // Real counts survive.
        assert_eq!(digit_count_candidates("共 5 条假说"), vec!["5"]);
        assert_eq!(digit_count_candidates("analyses: 12"), vec!["12"]);
    }

    #[test]
    fn combine_escalates_mismatch_to_fail() {
        let checks = vec![ReviewCheck {
            item: "x".into(),
            status: CheckStatus::Mismatch,
            detail: "3 vs 8".into(),
            evidence: "manifest".into(),
        }];
        let review = combine(checks, None);
        assert_eq!(review.verdict, "fail");
        assert_eq!(review.reviewer, "mechanical");
        assert!(review.issues.iter().any(|i| i.severity == "high"));
    }

    #[test]
    fn combine_pass_when_all_ok() {
        let checks = vec![ReviewCheck {
            item: "x".into(),
            status: CheckStatus::Ok,
            detail: "match".into(),
            evidence: "manifest".into(),
        }];
        let review = combine(checks, None);
        assert_eq!(review.verdict, "pass");
        assert!(review.issues.is_empty());
    }

    #[test]
    fn review_markdown_section_renders_verdict_and_table() {
        let review = ReportReview {
            verdict: "pass_with_warnings".into(),
            summary: "一个低危问题。".into(),
            checks: vec![ReviewCheck {
                item: "hypotheses".into(),
                status: CheckStatus::Ok,
                detail: "5 = 5".into(),
                evidence: "manifest".into(),
            }],
            issues: vec![ReviewIssue {
                severity: "low".into(),
                description: "缺少引用年份".into(),
                suggestion: "补全引用".into(),
            }],
            reviewer: "mechanical+llm".into(),
        };
        let md = review_markdown_section(&review);
        assert!(md.contains("pass_with_warnings"));
        assert!(md.contains("审核"));
        assert!(md.contains("缺少引用年份"));
        assert!(md.contains("mechanical+llm"));
    }

    #[test]
    fn persist_review_roundtrips() {
        let dir = std::env::temp_dir().join("miniagent_review_test");
        std::fs::create_dir_all(&dir).unwrap();
        let review = combine(
            vec![ReviewCheck {
                item: "x".into(),
                status: CheckStatus::Ok,
                detail: "d".into(),
                evidence: "e".into(),
            }],
            None,
        );
        let path = persist_review(&dir, &review).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["verdict"], "pass");
        assert!(v["checks"].as_array().unwrap().len() == 1);
        std::fs::remove_file(path).ok();
    }
}
