//! Loop-orchestrated research: every pipeline phase runs as a loop-semantics
//! subtask — explore → (clarify) → plan → dispatch → adjudicate → repair →
//! finalize — so phase completion is never a single model's opinion and the
//! user gets one chance to sharpen the requirements before compute starts.
//!
//! The dispatch step is the existing [`run_research`] invoked with
//! `stop_after=<phase>`; the manifest's resume logic makes each subsequent
//! subtask continue exactly where the previous one stopped. Phase artifacts
//! (papers.json / kg.json / hypotheses.json / …) are the evidence the
//! three-way adjudication (advocate → challenger → arbiter) rules on.
//!
//! Generic by construction: the phases below are the pipeline's own stage
//! structure (not domain content), and adjudication/clarification make their
//! decisions from the actual artifacts and the user's request — no hardcoded
//! terms, thresholds, or dictionaries.

use std::sync::Arc;

use miniagent_core::settings::AppConfig;
use miniagent_loop_pipeline::{adjudicate, ClarifyHook, SteerHook};
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

use crate::pipeline::{run_research, ResearchOptions, ResearchProgress};

/// One research phase as a loop subtask: the `key` matches
/// `ResearchOptions::stop_after`; the label is user-facing.
const PHASES: &[(&str, &str)] = &[
    ("literature", "文献检索与语料校验"),
    ("kg", "知识图谱构建"),
    ("prediction", "链路预测"),
    ("hypotheses", "假说生成与排序"),
    ("debate", "证据辩论与精炼"),
    ("validation", "验证计划"),
    ("analysis", "数据分析执行"),
    ("review", "报告审核"),
];

/// Manifest stage names each phase depends on — a phase is repaired by
/// resetting exactly these and re-dispatching (resume re-runs them).
const PHASE_STAGES: &[(&str, &[&str])] = &[
    ("literature", &["search", "abstracts", "corpus_coherence", "relevance_filter"]),
    ("kg", &["kg_extraction"]),
    ("prediction", &["link_prediction"]),
    ("hypotheses", &["hypothesis_generation", "ranking"]),
    ("debate", &["debate"]),
    ("validation", &["validation"]),
    ("analysis", &["analysis"]),
    ("review", &["review"]),
];

fn stages_of(phase: &str) -> &'static [&'static str] {
    PHASE_STAGES
        .iter()
        .find(|(k, _)| *k == phase)
        .map(|(_, stages)| *stages)
        .unwrap_or(&[])
}

/// Run the full research pipeline loop-orchestrated. Each phase is one
/// subtask with explore → clarify(first) → plan → dispatch → adjudicate →
/// (repair → re-adjudicate) semantics. Progress events use the phase name so
/// the server's plan pills stay aligned; loop-stage detail rides in the
/// summary payloads.
pub async fn run_research_in_loop(
    query: String,
    project_dir: std::path::PathBuf,
    opts: ResearchOptions,
    config: Arc<AppConfig>,
    on_progress: Option<ResearchProgress>,
    ask_hook: Option<ClarifyHook>,
    steer_hook: Option<SteerHook>,
    interactive_gate: bool,
) -> String {
    let emit = |phase: &str, status: &str, detail: Option<String>| {
        if let Some(d) = &detail {
            println!("      [{phase}/{status}] {d}");
        }
        if let Some(cb) = on_progress.as_ref() {
            cb(phase, status, detail.as_deref());
        }
    };

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Research × Loop: 8 subtasks, each explore→plan→dispatch→    ║");
    println!("║  adjudicate→repair (three-way ruling on phase artifacts)     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Working query — clarification answers are appended so translation,
    // requirement extraction, and anchoring all see the refined intent.
    let mut working_query = query.clone();
    let mut clarified_once = false;
    let mut summary_log: Vec<String> = Vec::new();
    let total_start = std::time::Instant::now();

    for (phase, label) in PHASES {
        let phase_start = std::time::Instant::now();
        emit(phase, "running", Some(format!("{label}: loop subtask start")));
        println!("\n┏━━ Loop subtask ▸ {label} [{phase}]");

        // ── P3 执行中转向：阶段边界拉取用户插入的指令，并入工作请求 ──
        if let Some(hook) = steer_hook.as_ref() {
            let steers = hook();
            for st in steers {
                working_query.push_str(&format!("\n[用户转向] {}", st));
                println!("   ➡️  steering: {st}");
                emit("steer", "completed", Some(format!("转向指令已并入后续阶段: {st}")));
            }
        }

        // ── Explore: what does the manifest say already exists? ─────
        let manifest_path = project_dir.join("project.json");
        let stage_status = std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(b.as_bytes()).ok())
            .map(|v| {
                v["stages"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| {
                                Some(format!(
                                    "{}={}",
                                    s["name"].as_str()?,
                                    s["status"].as_str()?
                                ))
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "fresh run".into());
        let explore_note = format!("explore: manifest stages → [{stage_status}]");
        println!("   {explore_note}");

        // ── Clarify: once, before the first subtask, when a channel exists.
        if !clarified_once {
            clarified_once = true;
            if let Some(hook) = ask_hook.as_ref() {
                match plan_clarification(&working_query, config.clone()).await {
                    Some(questions) if !questions.is_empty() => {
                        for (q, options) in questions.iter().take(3) {
                            let answer = (hook)(q.clone(), options.clone()).await;
                            if !answer.trim().is_empty() {
                                working_query.push_str(&format!("\n[已澄清] {q} → {answer}"));
                                println!("   clarify: {q} → {answer}");
                            } else {
                                println!("   clarify: {q} → (no answer; proceeding on stated assumptions)");
                            }
                        }
                        summary_log.push("clarified with user".into());
                    }
                    _ => {
                        println!("   clarify: task executable as stated — no questions");
                    }
                }
            } else {
                println!("   clarify: no interactive channel — skipped");
            }
        }

        // ── Plan: the phase's execution is the plan (deterministic). ──
        println!("   plan: dispatch run_research(stop_after={phase}) — resume skips completed stages");

        // ── 内层重跑循环：P4 审查门打回时，该阶段在同一轮内重新派发 ──
        // （此前打回只重置了 manifest 阶段却直接前进到下一阶段——重跑从未
        // 发生。每个阶段最多打回 2 次，避免用户反复打回造成死循环。）
        let mut rejections = 0usize;
        loop {
        // ── Dispatch (+ one bounded repair round) ────────────────────
        let mut opts_phase = opts.clone();
        opts_phase.stop_after = Some(phase.to_string());
        let mut attempt = 0usize;
        let summary = loop {
            attempt += 1;
            let out = run_research(
                working_query.clone(),
                project_dir.clone(),
                opts_phase.clone(),
                config.clone(),
                on_progress.as_ref().map(|cb| {
                    let cb = Arc::clone(cb);
                    Arc::new(move |stage: &str, status: &str, detail: Option<&str>| cb(stage, status, detail)) as crate::ResearchProgress
                }),
            )
            .await;
            // ── Adjudicate: three-way ruling on the phase's artifacts ──
            let evidence = phase_evidence(&project_dir, phase);
            let providers = adjudication_providers(&config);
            let goal = format!(
                "Research subtask '{label}' for the request: {}",
                working_query.replace('\n', " / ")
            );
            match adjudicate::adjudicate(
                &goal,
                &format!(
                    "run_research executed with stop_after={phase} (attempt {attempt}); \
                     manifest stages: [{stage_status}]"
                ),
                &evidence,
                &providers,
                CancellationToken::new(),
            )
            .await
            {
                Ok(adj) => {
                    println!(
                        "   adjudicate: {:?} — {}",
                        adj.verdict,
                        adj.summary.chars().take(160).collect::<String>()
                    );
                    for u in &adj.unmet {
                        println!("      ⚠ {u}");
                    }
                    // 三方裁决（advocate→challenger→arbiter）对前端可见：
                    // 作为该阶段的执行详情推送，多智能体协作过程不再是黑盒。
                    emit(
                        phase,
                        "running",
                        Some(format!(
                            "⚖️ 三方裁决（advocate→challenger→arbiter）：{:?} — {}{}",
                            adj.verdict,
                            adj.summary.chars().take(120).collect::<String>(),
                            if adj.unmet.is_empty() {
                                String::new()
                            } else {
                                format!("；未满足项 {} 个", adj.unmet.len())
                            }
                        )),
                    );
                    if adj.verdict == adjudicate::AdjudicationVerdict::Complete || attempt >= 2 {
                        if attempt >= 2
                            && adj.verdict != adjudicate::AdjudicationVerdict::Complete
                        {
                            println!(
                                "   ⚠ repair round exhausted — recording residual issues and moving on"
                            );
                        }
                        summary_log.push(format!(
                            "{phase}: {:?} (attempt {attempt})",
                            adj.verdict
                        ));
                        break out;
                    }
                    // needs_repair: reset the phase's stages so resume
                    // re-executes them, then loop once more with the
                    // arbiter's suggestions logged for the audit trail.
                    println!("   repair: resetting phase stages and re-dispatching");
                    repair_reset(&project_dir, phase, &adj);
                }
                Err(e) => {
                    println!("   adjudicate unavailable ({e}) — accepting dispatch result");
                    summary_log.push(format!("{phase}: adjudication unavailable"));
                    break out;
                }
            }
        };

        // ── Progress check: did the phase actually complete? ──────────
        // If the phase's stages are still not "completed" in the manifest
        // after dispatch + repair, the pipeline is fatally blocked (e.g.
        // provider outage) — running the remaining subtasks would just
        // cascade the same failure, so stop here and report honestly.
        if !phase_completed(&project_dir, phase) {
            let dur = phase_start.elapsed().as_secs_f64();
            let msg = format!(
                "子任务 '{label}' 在 dispatch + 修复后仍未完成（上游故障，见 project.json 事件日志）— 停止后续子任务，避免级联空转。\n\n{}",
                summary
            );
            println!("┗━━ {label} BLOCKED after {dur:.1}s — stopping the loop sequence\n");
            emit(phase, "failed", Some(msg.clone()));
            return msg;
        }

        // ── Emit completed BEFORE the gate: structured cards (debated
        // hypotheses / validation plans) ride the completed event, so the
        // user decides accept/reject with the actual artifacts in view. ──
        let dur = phase_start.elapsed().as_secs_f64();
        println!("┗━━ {label} done in {dur:.1}s\n");
        emit(phase, "completed", Some(summary.chars().take(200).collect()));

        // ── P4 阶段产物审查门：关键阶段完成后请用户 accept/reject ──
        // 仅 gate 假说与验证计划（科研判断密度最高的两个产物）。ask 通道
        // 超时（用户离开）自动视为接受——不阻塞无人值守运行。
        let gated = interactive_gate
            && ask_hook.is_some()
            && matches!(*phase, "hypotheses" | "validation")
            && rejections < 2;
        if gated {
            let question = format!(
                "阶段『{label}』已完成。接受并继续下一阶段，还是打回重做（可在下一条消息附修改意见）？"
            );
            let opts = vec![
                "接受，继续".to_string(),
                "打回重做（请补充修改意见）".to_string(),
            ];
            let answer = (ask_hook.as_ref().unwrap())(question, opts).await;
            let accept = answer.is_empty() || answer.contains("接受");
            if accept {
                println!("   ✅ 审查门：用户接受，继续");
                summary_log.push(format!("{phase}: accepted"));
            } else {
                rejections += 1;
                println!("   ↩️  审查门：用户打回（第 {rejections} 次）——{answer}");
                summary_log.push(format!("{phase}: rejected ({answer})"));
                // 打回：重置该阶段与其上游产物阶段，内层循环重新派发
                // （resume 语义：只重跑该阶段的 stages）。
                let manifest_path = project_dir.join("project.json");
                if let Ok(raw) = std::fs::read_to_string(&manifest_path)
                    && let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(raw.as_bytes())
                {
                    let stages: Vec<&str> = stages_of(phase).to_vec();
                    if let Some(arr) = v["stages"].as_array_mut() {
                        arr.retain(|st| {
                            !(stages.contains(&st["name"].as_str().unwrap_or(""))
                                && st["status"] == "completed")
                        });
                    }
                    if let Ok(json) = serde_json::to_vec_pretty(&v) {
                        let _ = std::fs::write(&manifest_path, json);
                    }
                }
                // 用户意见并入工作请求，重派时吸收
                working_query.push_str(&format!("\n[用户打回意见] {answer}"));
                emit(
                    phase,
                    "running",
                    Some(format!("↩️ 审查门打回（第 {rejections} 次）：重跑本阶段并吸收修改意见")),
                );
                continue;
            }
        }
        break;
        } // ── 内层重跑循环结束 ──
    }

    let total_dur = total_start.elapsed();
    println!("\n╔══ Research × Loop Complete ({:.1}s) ═══╗", total_dur.as_secs_f64());
    let final_summary = build_final_summary(&project_dir, &summary_log, &working_query, total_dur);
    println!("{final_summary}");
    final_summary
}

/// Structured end-of-run summary aggregated from the manifest and phase
/// artifacts: stage timings, KG scale, hypothesis verdict counts, validation
/// plans, analysis outcomes. The full narrative lives in `<brief>.md`; this
/// is the chat-facing digest.
fn build_final_summary(
    project_dir: &std::path::Path,
    summary_log: &[String],
    working_query: &str,
    total_dur: std::time::Duration,
) -> String {
    let read_json = |name: &str| -> Option<serde_json::Value> {
        std::fs::read(project_dir.join(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    };
    let manifest = read_json("project.json").unwrap_or(serde_json::Value::Null);

    // Stage table (name · status · duration).
    let mut stage_lines = String::new();
    if let Some(stages) = manifest.get("stages").and_then(|v| v.as_array()) {
        for s in stages {
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let status = s.get("status").and_then(|v| v.as_str()).unwrap_or("?");
            let icon = match status {
                "completed" => "✅",
                "skipped" => "⏭️",
                _ => "⚠️",
            };
            let dur = s
                .get("duration_secs")
                .and_then(|v| v.as_f64())
                .map(|d| format!(" {:.0}s", d))
                .unwrap_or_default();
            stage_lines.push_str(&format!("\n| {icon} {name} | `{status}`{dur} |"));
        }
    }

    // Hypotheses + debate verdicts.
    let hyp_count = read_json("hypotheses_refined_full.json")
        .or_else(|| read_json("hypotheses_full.json"))
        .and_then(|v| v.as_array().map(|a| a.len()))
        .unwrap_or(0);
    let mut verdict_counts = std::collections::BTreeMap::new();
    if let Some(per) = read_json("debate_report.json")
        .and_then(|d| d.get("per_hypothesis").cloned())
        .and_then(|v| v.as_array().cloned())
    {
        for v in per {
            let verdict = v.get("verdict").and_then(|x| x.as_str()).unwrap_or("?");
            *verdict_counts.entry(verdict.to_string()).or_insert(0usize) += 1;
        }
    }
    let verdict_str = if verdict_counts.is_empty() {
        "未辩论".to_string()
    } else {
        verdict_counts
            .iter()
            .map(|(k, n)| format!("{k}×{n}"))
            .collect::<Vec<_>>()
            .join(" · ")
    };

    // Validation plans + analysis outcomes from the manifest.
    let plan_count = manifest
        .get("validation_plans")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let analyses = manifest.get("analyses").and_then(|v| v.as_array());
    let (an_ok, an_fail) = analyses
        .map(|arr| {
            (
                arr.iter().filter(|a| a.get("success").and_then(|s| s.as_bool()) == Some(true)).count(),
                arr.iter().filter(|a| a.get("success").and_then(|s| s.as_bool()) == Some(false)).count(),
            )
        })
        .unwrap_or((0, 0));
    let kg_line = manifest
        .get("kg_stats")
        .and_then(|k| {
            let e = k.get("entities").and_then(|v| v.as_u64())?;
            let r = k.get("relations").and_then(|v| v.as_u64())?;
            Some(format!("\n| 🕸️ 知识图谱 | {e} 实体 · {r} 关系 |"))
        })
        .unwrap_or_default();

    // Papers count from the corpus artifact.
    let papers = read_json("papers.json")
        .and_then(|v| v.as_array().map(|a| a.len()))
        .unwrap_or(0);

    let brief = miniagent_core::paths::sanitize_task_brief(
        working_query.lines().next().unwrap_or("research"),
    );
    format!(
        "# Research × Loop 完成（{:.1} 分钟）\n\n\
         > 面向用户的完整报告：`{brief}.md` · 审计轨迹：`project.json` / `run_report.md` / `report_review.json`\n\n\
         ## 产物总览\n\
         | 阶段 | 结果 |\n\
         |---|---|\n\
         | 📚 文献语料 | {papers} 篇入库 |{kg_line}\n\
         | 💡 假说 | {hyp_count} 条（辩论裁决：{verdict_str}） |\n\
         | 🧪 验证计划 | {plan_count} 份（数据分析任务 + 湿实验方案） |\n\
         | 📓 数据分析 | 成功 {an_ok} / 失败 {an_fail}（.ipynb + 溯源记录见 analysis/） |\n\n\
         ## 阶段执行（每子任务 explore→dispatch→三方裁决→repair）\n\
         | 阶段 | 状态·耗时 |{stage_lines}\n\n\
         ## 裁决与审查记录\n{}",
        total_dur.as_secs_f64() / 60.0,
        summary_log
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Whether every stage of the phase recorded "completed" in the manifest.
fn phase_completed(project_dir: &std::path::Path, phase: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(project_dir.join("project.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(raw.as_bytes()) else {
        return false;
    };
    let done = |name: &str| {
        v["stages"].as_array().is_some_and(|arr| {
            arr.iter()
                .any(|s| s["name"].as_str() == Some(name) && s["status"] == "completed")
        })
    };
    stages_of(phase).iter().all(|st| done(st))
}

/// LLM decides whether the request has material ambiguity worth asking
/// about (same decision the loop Clarify stage makes, inlined here so the
/// research crate does not need the agent machinery).
async fn plan_clarification(
    query: &str,
    config: Arc<AppConfig>,
) -> Option<Vec<(String, Vec<String>)>> {
    use miniagent_provider::traits::CompletionRequest;
    let providers = adjudication_providers(&config);
    let prompt = format!(
        r#"Decide whether this research request has MATERIAL ambiguity that would change how a literature-review pipeline should execute it (scope, disease focus, time window, corpus size, success criteria). Do not ask about trivia a sensible default covers.

Request: {query}

Output ONLY valid JSON:
{{"need_clarification": true|false,
  "questions": [{{"question": "<one concrete question>", "options": ["<answer 1>", "<answer 2>"]}}]}}"#
    );
    for p in &providers {
        let request = CompletionRequest {
            system: "You are a precise requirements clarifier. Output ONLY valid JSON.".into(),
            messages: vec![miniagent_core::message::Message::user(&prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(600),
                ..Default::default()
            },
        };
        if let Ok(resp) = p.complete(&request, CancellationToken::new()).await {
            let text: String = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            #[derive(serde::Deserialize)]
            struct QPlan {
                #[serde(default)]
                need_clarification: bool,
                #[serde(default)]
                questions: Vec<ClarifyQ>,
            }
            #[derive(serde::Deserialize)]
            struct ClarifyQ {
                question: String,
                #[serde(default)]
                options: Vec<String>,
            }
            if let Ok(plan) =
                serde_json::from_str::<QPlan>(&miniagent_core::json_util::extract_and_repair(&text))
            {
                if plan.need_clarification && !plan.questions.is_empty() {
                    return Some(
                        plan.questions
                            .into_iter()
                            .map(|q| (q.question, q.options))
                            .collect(),
                    );
                }
                return Some(Vec::new());
            }
        }
    }
    None
}

/// Evidence bundle for the phase's adjudication: artifact files on disk +
/// the manifest's record for the phase's stages.
fn phase_evidence(project_dir: &std::path::Path, phase: &str) -> String {
    let mut lines = Vec::new();
    let artifacts: &[(&str, &[&str])] = &[
        ("literature", &["papers.json", "papers_rejected.json", "pubmed_query.txt"]),
        ("kg", &["kg.json"]),
        ("prediction", &["candidates.json"]),
        ("hypotheses", &["hypotheses.json", "hypotheses_full.json"]),
        ("debate", &["debate_report.json", "debate_evidence.json"]),
        ("validation", &["plans/validation_plan_0.json", "plans/validation_plan_1.json"]),
        ("analysis", &["analysis"]),
        ("review", &["report_review.json"]),
    ];
    let files = artifacts
        .iter()
        .find(|(k, _)| *k == phase)
        .map(|(_, f)| *f)
        .unwrap_or(&[]);
    for f in files {
        let p = project_dir.join(f);
        if p.is_dir() {
            let n = std::fs::read_dir(&p).map(|d| d.count()).unwrap_or(0);
            lines.push(format!("{f}: dir with {n} entries"));
        } else if let Ok(meta) = std::fs::metadata(&p) {
            lines.push(format!("{f}: {} bytes", meta.len()));
        } else {
            lines.push(format!("{f}: MISSING"));
        }
    }
    if let Ok(m) = std::fs::read_to_string(project_dir.join("project.json")) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(m.as_bytes()) {
            if let Some(stages) = v["stages"].as_array() {
                for s in stages {
                    let name = s["name"].as_str().unwrap_or("");
                    if stages_of(phase).contains(&name) {
                        lines.push(format!(
                            "manifest {name}: {} ({})",
                            s["status"].as_str().unwrap_or("?"),
                            s["summary"].as_object().map(|o| {
                                o.iter()
                                    .map(|(k, v)| format!("{k}={v}"))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            }).unwrap_or_default(),
                        ));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

/// flash + cross-family fallbacks for adjudication roles.
fn adjudication_providers(
    config: &Arc<AppConfig>,
) -> Vec<std::sync::Arc<dyn LlmProvider>> {
    let mut providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![
        miniagent_provider::factory::active_provider_pair(config)
            .map(|(f, _)| f.into())
            .unwrap_or_else(|_| panic!("research loop: active model profile unusable")),
    ];
    providers.extend(
        miniagent_provider::factory::codegen_fallback_providers(config)
            .into_iter()
            .map(Into::into),
    );
    providers
}

/// Reset the phase's completed stages in the manifest so the next dispatch
/// re-executes them, and log the repair decision with the arbiter's advice.
fn repair_reset(project_dir: &std::path::Path, phase: &str, adj: &adjudicate::Adjudication) {
    let manifest_path = project_dir.join("project.json");
    if let Ok(raw) = std::fs::read_to_string(&manifest_path)
        && let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(raw.as_bytes())
    {
        let stages: Vec<&str> = stages_of(phase).to_vec();
        if let Some(arr) = v["stages"].as_array_mut() {
            arr.retain(|s| {
                !(stages.contains(&s["name"].as_str().unwrap_or(""))
                    && s["status"] == "completed")
            });
        }
        if let Ok(json) = serde_json::to_vec_pretty(&v) {
            let _ = std::fs::write(&manifest_path, json);
        }
    }
    tracing::info!(
        phase = %phase,
        unmet = ?adj.unmet,
        suggestions = ?adj.suggestions,
        "repair round: phase stages reset for re-dispatch"
    );
}
