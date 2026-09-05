//! End-to-end research pipeline: literature → knowledge graph → link
//! prediction → pathogenesis hypotheses → evidence debate → validation
//! plans (data analysis + wet lab) → executable notebook analysis.
//!
//! Lives in the `research` crate so BOTH the CLI (`miniagent research`) and
//! the server (web "research" mode) drive the exact same code path — one
//! workflow, one artifact layout (`result/{id}_{brief}/…`), one audit
//! manifest (`project.json`).

use std::sync::Arc;

use miniagent_core::models::ModelRegistry;
use tokio_util::sync::CancellationToken;
use miniagent_core::settings::AppConfig;
use miniagent_provider::traits::LlmProvider;


/// Coarse phase progress callback: `(stage_key, "running" | "completed", detail)`.
/// The optional third argument carries a human-readable phase summary
/// (loop orchestrator) so the server can forward it to the frontend; plain
/// pipeline phase transitions pass `None`.
/// Shared through `Arc` so the server can move it into a channel-forwarding
/// task while the pipeline owns a clone.
pub type ResearchProgress = Arc<dyn Fn(&str, &str, Option<&str>) + Send + Sync>;

fn phase_begin(on_progress: &Option<ResearchProgress>, stage: &str) {
    if let Some(cb) = on_progress {
        cb(stage, "running", None);
    }
}

fn phase_end(on_progress: &Option<ResearchProgress>, prev: &mut Option<&'static str>) {
    if let Some(p) = prev.take()
        && let Some(cb) = on_progress {
        cb(p, "completed", None);
    }
}

/// Options mirroring the former CLI flags of `miniagent research`.
#[derive(Debug, Clone)]
pub struct ResearchOptions {
    /// Corpus size. None = derived from the request semantics by the LLM
    /// requirement-extraction pass (no fixed preset anywhere); an explicit
    /// value (CLI `-n`) overrides the extraction.
    pub max_papers: Option<usize>,
    pub kg_only: bool,
    pub validate: bool,
    pub analyze: bool,
    pub data: Option<String>,
    pub top_n: usize,
    pub enrich_file: Option<String>,
    pub enrich_delim: char,
    pub enrich_relation: String,
    pub debate: bool,
    pub min_year: String,
    pub use_store: bool,
    /// Run only up to the named phase and stop (loop-orchestrated research
    /// drives one phase per subtask; resume skips everything before it).
    /// Phase keys: "literature" | "kg" | "prediction" | "hypotheses" |
    /// "debate" | "validation" | "analysis" | "review". None = run all.
    pub stop_after: Option<String>,
}

impl Default for ResearchOptions {
    fn default() -> Self {
        Self {
            max_papers: None,
            kg_only: false,
            validate: false,
            analyze: false,
            data: None,
            top_n: 2,
            enrich_file: None,
            enrich_delim: ',',
            enrich_relation: "associated_with".into(),
            debate: false,
            min_year: "2023".into(),
            use_store: false,
            stop_after: None,
        }
    }
}

/// Build the (flash, pro) provider pair from the active model profile.
/// Panics on an unusable profile — callers validate the key first
/// (`AppConfig::require_active_key` at the top of `run_research`).
fn make_providers(config: &AppConfig) -> (Box<dyn LlmProvider>, Box<dyn LlmProvider>) {
    match miniagent_provider::factory::active_provider_pair(config) {
        Ok(pair) => pair,
        Err(e) => panic!("research pipeline: active model profile unusable: {e}"),
    }
}

/// Shared early-exit for mid-pipeline aborts (0 papers, 0 KG entities,
/// missing debate providers, `--kg-only` completion …).
///
/// Goal: a run directory must NEVER end up without a user-facing final
/// report just because a stage aborted — `write_user_report` degrades
/// gracefully over whatever artifacts exist on disk. Persist the manifest,
/// write both reports, and return a summary naming the stop reason.
fn finish_partial(
    manifest: &mut crate::ProjectManifest,
    query: &str,
    stop_reason: &str,
    event_kind: &str,
) -> String {
    manifest.log_event(event_kind, stop_reason.to_string());
    let _ = manifest.save();
    let _ = manifest.write_run_report();
    let brief = miniagent_core::paths::sanitize_task_brief(query);
    match manifest.write_user_report(&brief) {
        Ok(path) => println!("📁 final report (partial run): {}", path.display()),
        Err(e) => eprintln!("⚠️  failed to write user report: {e}"),
    }
    format!(
        "# Research Pipeline 已提前结束\n\n\
         - **停止原因**：{stop_reason}\n\
         - **部分结果报告**：`{brief}.md`（文献 / 知识图谱 / 假说等已完成章节仍可用）\n\
         - **审计轨迹**：`project.json`（append-only 事件日志）与 `run_report.md`\n\
         - 修复问题后重新运行同一目录即可从断点恢复（resume），已完成的阶段不会重跑。\n"
    )
}

pub async fn run_research(
    query: String,
    project_dir: std::path::PathBuf,
    opts: ResearchOptions,
    config: Arc<AppConfig>,
    on_progress: Option<ResearchProgress>,
) -> String {
    // Owned parameters keep the returned future `Send` across `tokio::spawn`
    // (reference args hit rustc's higher-ranked Send inference limit); rebind
    // as borrows so the pipeline body reads unchanged.
    let query: &str = &query;
    let config: &Arc<AppConfig> = &config;
    // Scope parameters; `min_year_owned`/`max_papers` may be refined by the
    // LLM requirement-extraction pass below (explicit option values are the
    // defaults the model fills from).
    let mut max_papers: usize = opts.max_papers.unwrap_or(0); // 0 = derive from the request
    let kg_only = opts.kg_only;
    let validate = opts.validate;
    let analyze = opts.analyze;
    let data = opts.data.as_deref();
    let top_n = opts.top_n;
    let enrich_file = opts.enrich_file.as_deref();
    let enrich_delim = opts.enrich_delim;
    let enrich_relation = opts.enrich_relation.as_str();
    let debate = opts.debate;
    let mut min_year_owned: String = opts.min_year.clone();
    let use_store = opts.use_store;
    let stop_after: Option<String> = opts.stop_after.clone();
    // Loop-orchestrated research drives ONE phase per subtask invocation:
    // when `stop_after` names the phase just completed, stop here (resume
    // skips everything before it, so the next subtask continues cleanly).
    let phase_stop = |phase: &str,
                      manifest: &mut crate::ProjectManifest,
                      q: &str|
     -> Option<String> {
        if stop_after.as_deref() == Some(phase) {
            return Some(finish_partial(
                manifest,
                q,
                &format!("stop_after={phase}：该阶段完成（loop 编排模式按阶段推进）"),
                "phase_stop",
            ));
        }
        None
    };
    let mut prev_phase: Option<&'static str> = None;
    use miniagent_kg::embedding::KgeModel;
    use miniagent_kg::link_prediction::LinkPredictionScorer;
    use miniagent_kg::schema::{RelationType};
    use miniagent_kg::KnowledgeGraph;
    
    use miniagent_tool::tools::{PubMedTool};
    use miniagent_tool::traits::{Tool, ToolContext};
    use miniagent_hypothesis::generator::HypothesisGenerator;
    use miniagent_hypothesis::ranking::HypothesisRanker;
    use tokio_util::sync::CancellationToken;
    use std::time::Instant;

    // The caller owns directory creation/naming (unified result/{id}_{brief}
    // scheme on both server and CLI).
    let project_dir = project_dir.to_path_buf();
    let _ = std::fs::create_dir_all(&project_dir);

    // Resume support (goal 1: long-running tasks). When `project.json` already
    // exists in the project dir, reload it and skip stages already completed —
    // each stage persists its artifacts under the project dir for this purpose.
    let mut manifest = if project_dir.join(crate::MANIFEST_FILENAME).exists() {
        match crate::ProjectManifest::load(&project_dir) {
            Ok(m) => {
                println!("   ↻ resume: {} completed stage(s) loaded from existing project.json",
                    m.completed_stage_names().len());
                let mut m = m;
                m.log_event("pipeline_resumed", format!("dir={}", project_dir.display()));
                m
            }
            Err(e) => {
                eprintln!("   ⚠ resume failed ({e}); starting a fresh manifest");
                crate::ProjectManifest::new(query, project_dir.clone())
            }
        }
    } else {
        crate::ProjectManifest::new(query, project_dir.clone())
    };
    if manifest.query != query {
        manifest.log_event("query_changed", format!("{} => {}", manifest.query, query));
    }

    // Key validation happens AFTER the manifest exists so a misconfigured
    // environment still leaves a user-facing report explaining the stop.
    if let Err(e) = config.require_active_key() {
        eprintln!("{e}");
        return finish_partial(&mut manifest, query, &format!("模型 API key 未配置：{e}"), "pipeline_aborted");
    }

    // dsh: append-only trajectory starts with full run attribution — model
    // profile + options snapshot, so any stage can be reproduced/audited from
    // project.json alone.
    {
        let registry = ModelRegistry::load(config);
        let active = registry.active();
        manifest.log_event(
            "run_config",
            serde_json::json!({
                "model_profile": active.id,
                "model_display_name": active.display_name,
                "provider_kind": active.kind.label(),
                "flash_model": active.model_name,
                "pro_model": active.pro_model(),
                "max_papers": max_papers,
                "kg_only": kg_only,
                "validate": validate,
                "analyze": analyze,
                "top_n": top_n,
                "debate": debate,
                "min_year": min_year_owned,
                "use_store": use_store,
            })
            .to_string(),
        );
    }

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  miniagent Research Pipeline                                 ║");
    println!("║  Query: {:<52}║", truncate(query, 52));
    println!("║  Max papers: {:<47}║", max_papers);
    println!("║  KG only: {:<50}║", kg_only);
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let cancel = CancellationToken::new();
    // Anchor tool execution inside the project dir (was process CWD — the
    // same scatter-class bug fixed across the other pipelines).
    let project_abs = std::fs::canonicalize(&project_dir).unwrap_or_else(|_| project_dir.clone());
    let ctx = ToolContext::new(project_abs.to_string_lossy().to_string(), "research".to_string());
    let start = Instant::now();
    // Shared flash provider handle for fan-out stages (goal 1: performance) —
    // `complete` takes `&self`, so one client is shared across parallel calls.
    let flash: std::sync::Arc<dyn LlmProvider> = make_providers(config).0.into();

    // ── Phase 0: Requirement Extraction (generic, LLM derived) ─────
    // Research requests state scope constraints in free language ("近8年",
    // "comprehensive", "at most 20 papers"). The model maps that intent to
    // concrete parameters — no regex, no language-specific parsing. Unstated
    // fields keep their configured defaults; explicit CLI/option values win.
    {
        let extraction_prompt = format!(
            "Extract research-scope constraints from this research request. \
             Output ONLY JSON: {{\"year_from\": <earliest publication year as a 4-digit integer, or null if not stated>, \
             \"max_papers\": <number of papers to retrieve as an integer, or null if not stated>, \
             \"notes\": \"<one short sentence>\"}}\n\nResearch request: {query}"
        );
        let request = miniagent_provider::traits::CompletionRequest {
            system: "You extract structured research-scope requirements. Output ONLY valid JSON.".into(),
            messages: vec![miniagent_core::message::Message::user(&extraction_prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(4_096),
                ..Default::default()
            },
        };
        let mut extraction_providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![flash.clone()];
        extraction_providers.extend(
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Into::into),
        );
        for provider in &extraction_providers {
        if let Ok(resp) = provider.complete(&request, cancel.child_token()).await {
            let text: String = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let repaired = miniagent_core::json_util::extract_and_repair(&text);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repaired) {
                let y = v["year_from"].as_u64().filter(|y| (1900..=2100).contains(y));
                let n = v["max_papers"].as_u64().filter(|n| (1..=500).contains(n));
                let notes = v["notes"].as_str().unwrap_or("");
                if let Some(n) = n {
                    max_papers = n as usize;
                }
                if y.is_some() || n.is_some() {
                    let y_old = min_year_owned.clone();
                    if let Some(y) = y {
                        min_year_owned = y.to_string();
                    }
                    println!(
                        "   🎯 requirement extraction: year_from={min_year_owned} (was {y_old}), max_papers={max_papers} — {notes}"
                    );
                    manifest.log_event(
                        "requirements_extracted",
                        format!("year_from={} max_papers={max_papers} notes={notes}", min_year_owned),
                    );
                }
                break; // extraction done
            }
        }
        }
    }
    if max_papers == 0 {
        // Requirement extraction failed entirely (providers down) — fall back
        // to a generous corpus rather than silently running tiny; audited.
        max_papers = 24;
        manifest.log_event(
            "max_papers_fallback",
            "requirement extraction unavailable; using fallback corpus size 24".to_string(),
        );
    }
    let min_year: &str = &min_year_owned;

    // ── Phases 1–2: Literature Search + Abstracts (resumable) ──────
    let papers_path = project_dir.join("papers.json");
    let mut paper_texts: Vec<(String, String)> = Vec::new();
    // Effective English PubMed query — persisted alongside the corpus so the
    // disease anchor and corpus-coherence gate work identically on resume.
    let pubmed_query_path = project_dir.join("pubmed_query.txt");
    let mut pubmed_query: String = std::fs::read_to_string(&pubmed_query_path)
        .unwrap_or_default()
        .trim()
        .to_string();
    let resumed_papers: Option<Vec<(String, String)>> = if manifest.is_stage_done("abstracts") {
        std::fs::read(&papers_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    } else {
        None
    };
    let (phase1_dur, phase2_dur) = if let Some(p) = resumed_papers.filter(|p| !p.is_empty()) {
        paper_texts = p;
        phase_end(&on_progress, &mut prev_phase);
        prev_phase = Some("literature");
        phase_begin(&on_progress, "literature");
        println!("━━━ Phase 1–2: ↻ resumed — {} abstracts from {} ━━━",
            paper_texts.len(), papers_path.display());
        (std::time::Duration::default(), std::time::Duration::default())
    } else {
    // ── Phase 1: Translate query to PubMed syntax if needed ──────
    // Generic, provider-agnostic: translate with the primary flash model,
    // escalate budget, then walk the cross-family fallback list. A non-English
    // research question is NEVER sent to PubMed raw — an unparseable term
    // makes NCBI match the entire database (measured: 6M hits with date
    // filters ignored), which silently poisons every downstream stage. When
    // no provider can produce a valid English query, the pipeline aborts with
    // a clear stop reason instead.
    if has_non_english(query) {
        let translation_prompt = format!(
            "Convert this research question into a PubMed search query.\n\
             Use English terms with boolean operators (AND/OR/NOT).\n\
             Prefer broad text-word searches over restrictive MeSH tags.\n\
             Include synonyms and variant spellings with OR.\n\
             Return ONLY the PubMed query string, nothing else.\n\n\
             Research question: {query}\n\n\
             PubMed query:"
        );
        let make_request = |prompt: String, max_tokens: u32| miniagent_provider::traits::CompletionRequest {
            system: "You are a PubMed search expert. Output ONLY the query string.".into(),
            messages: vec![miniagent_core::message::Message::user(prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0), max_tokens: Some(max_tokens), ..Default::default()
            },
        };
        let extract = |resp: &miniagent_provider::traits::CompletionResponse| {
            miniagent_core::json_util::strip_reasoning_tags(
                &resp.content.iter()
                    .filter_map(|b| match b {
                        miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    }).collect::<Vec<_>>().join(""),
            )
            .trim()
            .trim_matches('"')
            .trim()
            .to_string()
        };
        // Format validation only — no domain terms. PubMed queries with
        // synonym expansions legitimately exceed 400 chars, so length is
        // only bounded loosely; the non-English check targets CJK ranges
        // (curly quotes / en-dashes in English text are fine), and the
        // corpus-coherence gate is what actually catches whole-database
        // garbage retrieval.
        let has_cjk = |s: &str| s.chars().any(|c| {
            let u = c as u32;
            (0x4E00..=0x9FFF).contains(&u)          // CJK unified ideographs
                || (0x3000..=0x303F).contains(&u)   // CJK punctuation
                || (0xFF00..=0xFFEF).contains(&u)   // fullwidth forms
        });
        // Syntactic validation must include bracket/quote BALANCE: a query
        // truncated at the token cap (e.g. ending `OR "multi` with an unclosed
        // quote and one open paren) is exactly what made PubMed match 2.3M
        // papers — the unparseable tail degenerates the boolean expression.
        let balanced = |s: &str| {
            s.matches('(').count() == s.matches(')').count() && s.matches('"').count() % 2 == 0
        };
        let is_valid = |s: &str| {
            !s.is_empty()
                && !s.contains('<')
                && s.len() <= 2_000
                && !has_cjk(s)
                && balanced(s)
        };

        let mut providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![flash.clone()];
        providers.extend(
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Into::into),
        );
        let mut translation_provenance: Vec<String> = Vec::new();
        'translation: for (p_idx, provider) in providers.iter().enumerate() {
            let first = provider
                .complete(&make_request(translation_prompt.clone(), 16_384), cancel.child_token())
                .await
                .ok()
                .map(|resp| extract(&resp))
                .unwrap_or_default();
            let first_head: String = first.chars().take(40).collect();
            let candidate = if is_valid(&first) {
                first
            } else {
                let retry_prompt = format!(
                    "The previous attempt failed: it must be a pure-ASCII English PubMed \
                     query with boolean operators, with NO Chinese characters, NO quotes, \
                     NO explanation.\n\nPrevious attempt: {first}\n\nResearch question: {query}\n\n\
                     Corrected PubMed query:"
                );
                let second = provider
                    .complete(&make_request(retry_prompt, 16_384), cancel.child_token())
                    .await
                    .ok()
                    .map(|resp| extract(&resp))
                    .unwrap_or_default();
                if is_valid(&second) { second } else { String::new() }
            };
            if is_valid(&candidate) {
                pubmed_query = candidate;
                translation_provenance.push(format!("provider#{p_idx}"));
                break 'translation;
            }
            translation_provenance.push(format!("provider#{p_idx}: invalid({first_head})"));
        }
        if !is_valid(&pubmed_query) {
            // All providers failed — abort loudly. Sending the raw non-English
            // question to PubMed matches the whole database and produces an
            // off-topic run whose stages all "succeed" (the failure mode this
            // replaces).
            let reason = format!(
                "查询翻译失败：{} 个供应商均无法产出有效英文 PubMed 查询（{}）。为避免整库误检，管线中止；请稍后重试或改用英文描述研究问题。",
                providers.len(),
                translation_provenance.join("; "),
            );
            manifest.record_stage(
                "search",
                crate::StageStatus::Failed,
                std::time::Duration::default(),
                vec![],
                Some(serde_json::json!({ "translation": "failed" })),
            );
            manifest.log_event("query_translation_failed", reason.clone());
            eprintln!("\n❌ Pipeline aborted: {reason}");
            return finish_partial(&mut manifest, query, &reason, "pipeline_aborted");
        }
        manifest.log_event("query_translated", format!("{} → {}", query, pubmed_query));
        eprintln!("   Query translated: {query} → {pubmed_query}");
    } else {
        pubmed_query = query.to_string();
    }
    // Persist the effective English query: the disease-anchor stage and the
    // corpus-coherence gate both operate on it, including on resume.
    let _ = std::fs::write(&pubmed_query_path, &pubmed_query);

    // ── Phase 1b: Search PubMed (multi-batch pagination) ──────────
    let phase_start = Instant::now();
    phase_end(&on_progress, &mut prev_phase);
    prev_phase = Some("literature");
    phase_begin(&on_progress, "literature");
    println!("━━━ Phase 1: Literature Search ━━━");
    println!("   PubMed query: {pubmed_query}");

    let pubmed = PubMedTool::new();
    let page_size = 200usize; // reliable PubMed batch size (ESummary URL limit)
    let mut all_pmids: Vec<String> = Vec::new();
    let mut total_hits = 0usize;
    let batches_needed = max_papers.div_ceil(page_size);

    for batch in 0..batches_needed {
        let offset = batch * page_size;
        let remaining = max_papers.saturating_sub(all_pmids.len());
        let batch_size = remaining.min(page_size);

        let pubmed_result = pubmed.execute(
            serde_json::json!({
                "query": pubmed_query,
                "max_results": batch_size,
                "offset": offset,
                "min_year": min_year
            }),
            &ctx, cancel.child_token(),
        ).await.unwrap_or_else(|e| miniagent_tool::traits::ToolOutput {
            content: format!("PubMed error: {e}"), metadata: None,
        });

        let batch_pmids: Vec<String> = pubmed_result.content.lines()
            .filter_map(|l| l.strip_prefix("   PMID: "))
            .filter_map(|s| s.split(' ').next())
            .map(|s| s.to_string())
            .collect();

        if total_hits == 0 {
            total_hits = pubmed_result.content.lines()
                .find(|l| l.starts_with("Total results:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|s| s.split('|').next())
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
        }

        all_pmids.extend(batch_pmids);

        if batches_needed > 1 {
            eprintln!("   Batch {}/{}: {} PMIDs (total: {})",
                batch + 1, batches_needed, all_pmids.len(), all_pmids.len());
        }

        if all_pmids.len() >= max_papers { break; }
        // Rate limit: PubMed allows 3 requests/sec without API key, 10/sec with
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }

    let pmids = all_pmids;
    println!("   PubMed: {total_hits} total, {} retrieved ({} batches)",
        pmids.len(), batches_needed);
    let phase1_dur = phase_start.elapsed();

    // Validation gate (harness self-verification): an empty retrieval makes
    // every downstream stage meaningless. Record the failure for audit and
    // abort instead of "completing" an empty run.
    if pmids.is_empty() {
        manifest.record_stage(
            "search",
            crate::StageStatus::Failed,
            phase1_dur,
            vec![],
            Some(serde_json::json!({ "retrieved": 0, "total_hits": total_hits })),
        );
        manifest.log_event("stage_validation_failed", "search: 0 PMIDs retrieved — aborting (check query translation / PubMed connectivity)");
        eprintln!("\n❌ Pipeline aborted: PubMed returned 0 results. The translated query may be malformed — see project.json event_log.");
        return finish_partial(&mut manifest, query, "PubMed 检索返回 0 篇文献（检查查询翻译 / PubMed 连通性）", "pipeline_aborted");
    }
    manifest.record_stage(
        "search",
        crate::StageStatus::Completed,
        phase1_dur,
        vec![],
        Some(serde_json::json!({ "retrieved": pmids.len() })),
    );

    // ── Phase 2: Fetch Abstracts via PubMed E-utilities (batched efetch) ─
    let phase_start = Instant::now();
    println!("\n━━━ Phase 2: Fetch Abstracts ({} papers) ━━━", pmids.len());

    let pubmed_key = std::env::var("PUBMED_API_KEY").unwrap_or_default();
    let client = reqwest::Client::builder()
        .user_agent("miniagent/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");
    // efetch natively supports comma-separated ids; we request the XML form —
    // the plain-text format places each record's "Conflict of interest
    // statement" block after the `PMID:` terminator line, which previously
    // shifted every abstract onto the wrong PMID. XML pairs fields to PMIDs
    // unambiguously. One request per 50 papers.
    let chunk_size = 50;
    for chunk in pmids.chunks(chunk_size) {
        let ids = chunk.join(",");
        let mut url = format!(
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi?db=pubmed&id={ids}&retmode=xml"
        );
        if !pubmed_key.is_empty() {
            url.push_str(&format!("&api_key={pubmed_key}"));
        }
        let cancelled = cancel.is_cancelled();
        if cancelled {
            break;
        }
        let body = match client.get(&url).send().await {
            Ok(resp) => resp.text().await.unwrap_or_default(),
            Err(e) => {
                eprintln!("   ⚠️ efetch batch failed: {e}");
                String::new()
            }
        };
        for (pmid, text) in parse_efetch_xml(&body) {
            let clean = text.trim().to_lowercase();
            let word_count = text.split_whitespace().count();
            if word_count < 30           // too short for an abstract
                || clean.contains("no abstract")
                || clean.contains("javascript")
                || clean.starts_with("<")
                || clean.contains("pubmed central")
                || clean.contains("nih public access")
            {
                continue; // Not a usable abstract
            }
            paper_texts.push((pmid, text));
        }

        let pct = (paper_texts.len() * 100 / pmids.len().min(max_papers)).min(100);
        eprintln!("   Progress: {}/{} ({}%)", paper_texts.len(), pmids.len().min(max_papers), pct);

        if paper_texts.len() >= max_papers { break; }
    }

    println!("   Fetched {} abstracts", paper_texts.len());
    // Persist the fetched corpus for resume + audit (goal 1).
    if let Ok(json) = serde_json::to_vec(&paper_texts) {
        let _ = std::fs::write(&papers_path, json);
    }
    let phase2_dur = phase_start.elapsed();
    manifest.record_stage(
        "abstracts",
        crate::StageStatus::Completed,
        phase2_dur,
        vec![papers_path.clone()],
        Some(serde_json::json!({ "fetched": paper_texts.len() })),
    );
    let _ = manifest.save();
    (phase1_dur, phase2_dur)
    };

    // ── Phase 2a: Corpus-Coherence Gate (fail-closed, LLM judged) ──
    // Generic protection against whole-database retrieval and any other
    // systematic retrieval failure: one LLM verdict on whether the corpus as
    // a whole is on-topic for the research question. No keyword lists or
    // numeric thresholds — the model decides from the titles and the question.
    // An incoherent corpus poisons KG, hypotheses, plans, and analysis while
    // every stage still "succeeds" (the live failure this gate exists for),
    // so incoherence aborts the pipeline for audit instead of continuing.
    if !manifest.is_stage_done("corpus_coherence") && !paper_texts.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 2a: Corpus-Coherence Gate ━━━");
        let titles: Vec<String> = paper_texts
            .iter()
            .map(|(pmid, text)| {
                let head: String = text.chars().take(140).collect();
                format!("PMID {pmid}: {head}")
            })
            .collect();
        let coherence_prompt = format!(
            "Research question:\n{query}\n\nRetrieved corpus ({n} papers, title/abstract heads):\n{titles}\n\n\
             Judge ONLY whether this corpus as a whole is on-topic for the research question. \
             Output ONLY JSON: {{\"coherent\": true|false, \"on_topic_estimate\": <0-100>, \"reason\": \"<one sentence>\"}}",
            n = titles.len(),
            titles = titles.join("\n"),
        );
        // Reasoning models burn budget on chain-of-thought before the JSON —
        // a small cap yields empty text (observed live at 200 tokens).
        let coherence_request = miniagent_provider::traits::CompletionRequest {
            system: "You judge literature-retrieval quality. Output ONLY valid JSON.".into(),
            messages: vec![miniagent_core::message::Message::user(&coherence_prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(16_384),
                ..Default::default()
            },
        };
        let mut coherent: Option<bool> = None;
        let mut coherence_reason = String::new();
        let mut coherence_providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![flash.clone()];
        coherence_providers.extend(
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Into::into),
        );
        for provider in &coherence_providers {
            if let Ok(resp) = provider.complete(&coherence_request, cancel.child_token()).await {
                let text: String = resp
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let repaired = miniagent_core::json_util::extract_and_repair(&text);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repaired) {
                    coherent = Some(v["coherent"].as_bool().unwrap_or(false));
                    coherence_reason = v["reason"].as_str().unwrap_or("").to_string();
                    break;
                }
            }
        }
        let dur = phase_start.elapsed();
        match coherent {
            Some(true) => {
                println!("   ✅ corpus coherent: {coherence_reason}");
                manifest.record_stage(
                    "corpus_coherence",
                    crate::StageStatus::Completed,
                    dur,
                    vec![],
                    Some(serde_json::json!({ "coherent": true })),
                );
            }
            Some(false) => {
                let reason = format!(
                    "语料一致性门判定检索结果与研究问题不符：{coherence_reason}。为避免整条管线在错误语料上空转，管线中止；请调整研究问题表述后重试。"
                );
                manifest.record_stage(
                    "corpus_coherence",
                    crate::StageStatus::Failed,
                    dur,
                    vec![],
                    Some(serde_json::json!({ "coherent": false, "reason": coherence_reason })),
                );
                manifest.log_event("corpus_incoherent", reason.clone());
                eprintln!("\n❌ Pipeline aborted: {reason}");
                return finish_partial(&mut manifest, query, &reason, "pipeline_aborted");
            }
            None => {
                // Verdict unavailable (providers degraded) — fail CLOSED: an
                // unjudgeable corpus is exactly when off-topic corpora slip
                // through, so abort instead of continuing blindly.
                let reason = "语料一致性门无法完成判定（LLM 不可用）。为保守起见管线中止；请稍后重试。";
                manifest.record_stage(
                    "corpus_coherence",
                    crate::StageStatus::Failed,
                    dur,
                    vec![],
                    Some(serde_json::json!({ "verdict": "unavailable" })),
                );
                manifest.log_event("corpus_coherence_unavailable", reason);
                eprintln!("\n❌ Pipeline aborted: {reason}");
                return finish_partial(&mut manifest, query, reason, "pipeline_aborted");
            }
        }
        let _ = manifest.save();
    }

    // ── Phase 2b: Relevance Filter ────────────────────────────────
    // PubMed keyword recall is broad; a single off-topic abstract can
    // dominate link prediction with hub entities unrelated to the query
    // (observed: one sarcopenia paper redirected every hypothesis away from
    // the queried disease). A cheap flash-model filter keeps the corpus
    // on-topic; rejections are persisted for audit. Fail-CLOSED: papers the
    // filter could not judge (provider error/empty) are rejected, not kept —
    // the corpus-coherence gate above already guaranteed overall on-topicness
    // before this per-paper pass.
    if !manifest.is_stage_done("relevance_filter") && !paper_texts.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 2b: Relevance Filter ━━━");
        let mut filter_fallbacks: Vec<std::sync::Arc<dyn LlmProvider>> =
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Into::into)
                .collect();
        filter_fallbacks.push(flash.clone());
        let n_in = paper_texts.len();
        let (mut kept, rejected, unjudged) =
            filter_irrelevant_papers(flash.clone(), filter_fallbacks, &pubmed_query, &paper_texts, cancel.child_token()).await;
        // Corpus-level verdict (Phase 2a) already established on-topicness;
        // the per-paper pass is best-effort trimming. Unjudged papers are
        // retained (audited) rather than sinking a coherent corpus.
        if !unjudged.is_empty() {
            println!(
                "      ⚠ {}/{} papers unjudged (provider degradation) — retained on the corpus-level verdict",
                unjudged.len(), n_in
            );
            kept.extend(unjudged.iter().cloned());
            manifest.log_event(
                "relevance_filter_unjudged_retained",
                format!("{}/{} retained on corpus-level coherence verdict", unjudged.len(), n_in),
            );
        }
        println!(
            "   kept {} / {} (rejected {} as off-topic)",
            kept.len(),
            paper_texts.len(),
            rejected.len()
        );
        if !rejected.is_empty() {
            let dump: Vec<serde_json::Value> = rejected
                .iter()
                .map(|(pmid, reason)| serde_json::json!({"pmid": pmid, "reason": reason}))
                .collect();
            if let Ok(json) = serde_json::to_vec_pretty(&dump) {
                let _ = std::fs::write(project_dir.join("papers_rejected.json"), json);
                println!("      → {}", project_dir.join("papers_rejected.json").display());
            }
        }
        if kept.is_empty() {
            // Fail-closed filter emptied the corpus: either the corpus really
            // is off-topic or the judging provider degraded mid-run. Either
            // way, continuing on the UNFILTERED corpus would defeat the gate.
            let reason = format!(
                "相关性过滤后保留 0/{total} 篇（全部被判定不相关）。为避免在不可信语料上继续，管线中止。",
                total = paper_texts.len(),
            );
            manifest.record_stage(
                "relevance_filter",
                crate::StageStatus::Failed,
                phase_start.elapsed(),
                vec![],
                Some(serde_json::json!({ "kept": 0, "rejected": rejected.len() })),
            );
            manifest.log_event("relevance_filter_empty", reason.clone());
            eprintln!("\n❌ Pipeline aborted: {reason}");
            return finish_partial(&mut manifest, query, &reason, "pipeline_aborted");
        }
        paper_texts = kept;
        // papers.json is the resume artifact — persist the filtered corpus.
        if let Ok(json) = serde_json::to_vec(&paper_texts) {
            let _ = std::fs::write(&papers_path, json);
        }
        manifest.record_stage(
            "relevance_filter",
            crate::StageStatus::Completed,
            phase_start.elapsed(),
            vec![papers_path.clone()],
            Some(serde_json::json!({
                "kept": paper_texts.len(),
                "rejected": rejected.len(),
            })),
        );
        let _ = manifest.save();
        if let Some(out) = phase_stop("literature", &mut manifest, query) {
            return out;
        }
    }

    // ── Phase 3: KG Extraction (resumable, parallel) ──────────────
    let phase_start = Instant::now();
    println!("\n━━━ Phase 3: Knowledge Graph Extraction ━━━");

    let kg_path = project_dir.join("kg.json");
    let mut kg = load_kg(&kg_path).filter(|_| manifest.is_stage_done("kg_extraction"));

    if let Some(ref loaded) = kg {
        println!("   ↻ resumed KG: {} entities, {} relations",
            loaded.entity_count(), loaded.relation_count());
    } else {
        // Bounded-concurrency LLM extraction (goal 1: performance): one shared
        // flash provider, several papers in flight at once. Cross-family
        // fallbacks absorb provider-level failures such as 429 token-cap
        // exhaustion or an out-of-balance account. Concurrency is kept low:
        // reasoning models emit long thinking traces, and a wide fan-out
        // trips the provider's tokens-per-minute cap instantly.
        let extraction_fallbacks: Vec<std::sync::Arc<dyn LlmProvider>> =
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Arc::from)
                .collect();
        if !extraction_fallbacks.is_empty() {
            println!(
                "   🔁 KG extraction: {} fallback provider(s) wired",
                extraction_fallbacks.len()
            );
            manifest.log_event(
                "extraction_fallback_wired",
                format!("KG extraction using {} cross-family fallback provider(s)", extraction_fallbacks.len()),
            );
        }
        let concurrency = 3usize;
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut jobs = Vec::with_capacity(paper_texts.len());
        for (i, (pmid, text)) in paper_texts.iter().enumerate() {
            let flash = flash.clone();
            let fallbacks = extraction_fallbacks.clone();
            let sem = sem.clone();
            let cancel = cancel.child_token();
            let pmid = pmid.clone();
            let text = text.clone();
            jobs.push((
                i,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    extract_paper_entities(flash, fallbacks, &pmid, &text, cancel).await
                }),
            ));
        }
        let mut results: Vec<Option<miniagent_kg::extraction::ExtractionResult>> =
            (0..paper_texts.len()).map(|_| None).collect();
        for (i, job) in jobs {
            match job.await {
                Ok(Ok(extraction)) => results[i] = Some(extraction),
                Ok(Err(e)) => eprintln!("   ⚠ Paper {} extraction error: {e}", i + 1),
                Err(e) => eprintln!("   ⚠ Paper {} extraction task failed: {e}", i + 1),
            }
        }
        let mut merged = KnowledgeGraph::new();
        let mut total_merged_entities = 0usize;
        let mut total_dangling = 0usize;
        for (i, extraction) in results.into_iter().enumerate() {
            if let Some(extraction) = extraction {
                // Alias-aware canonical merge: remaps relation endpoints to
                // canonical entity ids (the old name-only merge left dangling
                // edges when duplicate entity names were skipped).
                let stats = miniagent_kg::extraction::merge_extraction_canonical(&mut merged, extraction);
                total_merged_entities += stats.entities_merged;
                total_dangling += stats.relations_dropped;
                println!(
                    "   Paper {} — {} entities (+{} merged into existing), {} relations",
                    i + 1,
                    stats.entities_added,
                    stats.entities_merged,
                    stats.relations_added
                );
            }
        }
        if total_merged_entities > 0 || total_dangling > 0 {
            println!("   (alias-merged {total_merged_entities} duplicate entities; dropped {total_dangling} unresolved relations)");
        }
        kg = Some(merged);
        // Persist the KG (ids preserved) for resume + audit (goal 1).
        if let Err(e) = save_kg(kg.as_ref().unwrap(), &kg_path) {
            eprintln!("   ⚠ failed to persist KG: {e}");
        }
    }
    let mut kg = kg.unwrap_or_else(KnowledgeGraph::new);

    println!("\n   📊 KG: {} entities, {} relations", kg.entity_count(), kg.relation_count());

    // Validation gate (harness self-verification): extraction returning an
    // empty graph from a non-empty corpus means every LLM parse failed —
    // fail loudly for auditability instead of skipping the rest silently.
    if kg.entity_count() == 0 && !paper_texts.is_empty() && !manifest.is_stage_done("kg_extraction") {
        manifest.record_stage(
            "kg_extraction",
            crate::StageStatus::Failed,
            phase_start.elapsed(),
            vec![kg_path.clone()],
            Some(serde_json::json!({ "entities": 0, "papers": paper_texts.len() })),
        );
        manifest.log_event(
            "stage_validation_failed",
            format!("kg_extraction: 0 entities from {} papers — all LLM output parses failed", paper_texts.len()),
        );
        eprintln!("\n❌ Pipeline aborted: KG extraction produced 0 entities from {} papers. See warnings above and project.json event_log.", paper_texts.len());
        return finish_partial(&mut manifest, query, &format!("知识图谱抽取得到 0 个实体（{0} 篇文献全部解析失败）", paper_texts.len()), "pipeline_aborted");
    }

    // Print KG as Mermaid
    println!("\n   ── Knowledge Graph (Mermaid) ──");
    println!("```mermaid\ngraph TD");
    for entity in kg.all_entities() {
        let etype = format!("{:?}", entity.entity_type);
        let safe_name = entity.name.replace([' ', '-'], "_");
        println!("    {safe_name}[\"{etype}\n{name}\"]", name = entity.name);
    }
    for rel in kg.all_relations().iter().take(30) {
        let from_name = kg.get_entity(&rel.from_id).map(|e| e.name.replace([' ', '-'], "_")).unwrap_or_default();
        let to_name = kg.get_entity(&rel.to_id).map(|e| e.name.replace([' ', '-'], "_")).unwrap_or_default();
        let rt = format!("{:?}", rel.relation_type);
        if !from_name.is_empty() && !to_name.is_empty() {
            println!("    {from_name} --\"{rt}\"--> {to_name}");
        }
    }
    println!("```");

    // Audit artifact (goal 1): map KG paper ids back to PMIDs + titles so
    // every edge's `source_paper_id` resolves to a citable reference.
    let sources_path = project_dir.join("kg_sources.json");
    let mut source_map = serde_json::Map::new();
    for rel in kg.all_relations() {
        for pid in rel.supporting_papers.iter().chain(rel.source_paper_id.iter()) {
            let key = pid.to_string();
            if source_map.contains_key(&key) {
                continue;
            }
            let pmid = pmid_from_uuid(*pid).unwrap_or_default();
            let title = paper_texts
                .iter()
                .find(|(p, _)| *p == pmid)
                .and_then(|(_, t)| t.lines().next())
                .unwrap_or("")
                .strip_prefix("Title: ")
                .unwrap_or("")
                .to_string();
            source_map.insert(key, serde_json::json!({ "pmid": pmid, "title": title }));
        }
    }
    let _ = std::fs::write(
        &sources_path,
        serde_json::to_vec_pretty(&serde_json::Value::Object(source_map))
            .unwrap_or_default(),
    );

    let phase3_dur = phase_start.elapsed();
    manifest.record_stage(
        "kg_extraction",
        crate::StageStatus::Completed,
        phase3_dur,
        vec![kg_path.clone()],
        Some(serde_json::json!({
            "entities": kg.entity_count(),
            "relations": kg.relation_count(),
        })),
    );
    let _ = manifest.save();

    // ── Optional: External KG Enrichment ──────────────────────────
    // Merge triples from a biomedical KG export (DisGeNET/OMIM/custom TSV)
    // to broaden link prediction beyond PubMed-extracted edges. (Goal 2)
    if let Some(path) = enrich_file {
        println!("\n━━━ KG Enrichment: {path} ━━━");
        let rel = miniagent_kg::schema::RelationType::parse(enrich_relation)
            .unwrap_or(miniagent_kg::schema::RelationType::AssociatedWith);
        let source_label = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("external");
        match miniagent_kg::external::load_fixed_relation_tsv(
            path,
            enrich_delim,
            miniagent_kg::schema::EntityType::Gene,
            rel.clone(),
            miniagent_kg::schema::EntityType::Disease,
            source_label,
        ) {
            Ok(triples) => {
                let n = triples.len();
                let stats = miniagent_kg::merge_external(&mut kg, &triples);
                println!(
                    "   loaded {n} triples ({:?}): +{} edges, +{} entities ({} duplicates skipped)",
                    rel, stats.edges_added, stats.entities_created, stats.edges_skipped_duplicate
                );
                println!("   📊 KG after enrichment: {} entities, {} relations", kg.entity_count(), kg.relation_count());
            }
            Err(e) => eprintln!("   ⚠ enrichment load failed: {e}"),
        }
    }

    // ── Persistent KG store: accumulate this run's corpus for future runs.
    // Max-combining makes repeated merges idempotent (no support inflation).
    {
        let store_path =
            std::path::PathBuf::from(std::env::var("KG_STORE_PATH").unwrap_or_else(|_| "kg_store.json".into()));
        let mut store = miniagent_kg::KgStore::load(&store_path);
        let stats = store.merge(&kg);
        match store.save() {
            Ok(()) => {
                println!(
                    "   🗃️  KG store: +{}/{} entities, +{}/{} relations → {}",
                    stats.entities_added,
                    stats.entities_merged,
                    stats.relations_added,
                    stats.relations_merged,
                    store_path.display()
                );
                manifest.log_event(
                    "kg_store_merged",
                    format!(
                        "entities +{}/{} relations +{}/{}",
                        stats.entities_added, stats.entities_merged,
                        stats.relations_added, stats.relations_merged
                    ),
                );
            }
            Err(e) => eprintln!("   ⚠️  KG store save failed: {e}"),
        }
    }

    if kg_only {
        let total = start.elapsed();
        println!("\n╔══ Pipeline Complete (KG only) ═══════════════════════════╗");
        println!("║ Search: {:>6.1}s  Fetch: {:>6.1}s  KG: {:>6.1}s  Total: {:>6.1}s",
            phase1_dur.as_secs_f64(), phase2_dur.as_secs_f64(),
            phase3_dur.as_secs_f64(), total.as_secs_f64());
        println!("╚════════════════════════════════════════════════════════════╝");
        manifest.log_event("pipeline_complete_kg_only", format!("total_secs={:.1}", total.as_secs_f64()));
        return finish_partial(&mut manifest, query, "kg-only 模式：知识图谱构建完成后按参数停止", "pipeline_complete_kg_only");
    }
    if let Some(out) = phase_stop("kg", &mut manifest, query) {
        return out;
    }

    // ── Phase 4: Embedding & Link Prediction (resumable) ──────────
    let phase_start = Instant::now();
    println!("\n━━━ Phase 4: Embedding & Link Prediction ━━━");

    let candidates_path = project_dir.join("candidates.json");
    let mut all_candidates: Vec<miniagent_kg::link_prediction::HypothesisCandidate> =
        if manifest.is_stage_done("link_prediction") {
            std::fs::read(&candidates_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    if all_candidates.is_empty() {
        // ── Persistent KG store: merge this run's corpus, optionally seed
        // candidate generation with knowledge from previous runs. ──
        let store_path =
            std::path::PathBuf::from(std::env::var("KG_STORE_PATH").unwrap_or_else(|_| "kg_store.json".into()));
        let store = miniagent_kg::KgStore::load(&store_path);
        if use_store && store.knowledge_graph().relation_count() > 0 {
            println!(
                  "   🗃️  KG store: {} entities / {} relations from previous runs",
                store.knowledge_graph().entity_count(),
                store.knowledge_graph().relation_count(),
            );
            let mut union = KnowledgeGraph::new();
            for e in store.knowledge_graph().all_entities() {
                union.add_entity(e.clone());
            }
            for r in store.knowledge_graph().all_relations() {
                union.add_relation(r.clone());
            }
            for e in kg.all_entities() {
                union.add_entity(e.clone());
            }
            for r in kg.all_relations() {
                union.merge_relation(r.clone());
            }
            kg = union;
        }

        let mut kge = KgeModel::new(128);
        kge.train(&kg, 200, 0.005);
        println!("   TransE 128-dim trained on {} relations", kg.relation_count());

        // Hold-out quality check (opt-in: retrains a second model).
        if std::env::var("KG_EVAL").map(|v| v != "0").unwrap_or(false) {
            let (mrr, hits, n) = KgeModel::holdout_evaluate(128, &kg, 200, 0.005, 0.1);
            if n > 0 {
                println!("   📏 TransE hold-out (n={n}): MRR={mrr:.3}, hits@10={}/{}", hits, n);
            }
        }

        let scorer = LinkPredictionScorer::new().with_kge(kge);
        let mut cands = Vec::new();
        let rel_types = [RelationType::Regulates, RelationType::Inhibits, RelationType::Activates, RelationType::AssociatedWith];

        for entity in kg.all_entities() {
            for rt in &rel_types {
                let candidates = scorer.predict_tails(&entity.id, rt, &kg, 2);
                cands.extend(candidates);
            }
        }

        // GIVE-style neighborhood extrapolation (the doc's headline link
        // prediction channel): surface entities semantically close to the
        // KNOWN tails of each (head, relation), not just structurally close.
        for entity in kg.all_entities() {
            for rt in &rel_types {
                cands.extend(scorer.give_extrapolation(&entity.id, rt, &kg, 1));
            }
        }

        // Dedup by triple, keeping the higher-scoring candidate.
        cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let mut seen: std::collections::HashSet<(String, String, String)> =
            std::collections::HashSet::new();
        cands.retain(|c| {
            seen.insert((
                c.head.0.to_string(),
                format!("{:?}", c.relation),
                c.tail.0.to_string(),
            ))
        });

        // Anchor the candidate set to the queried disease. Without this, hub
        // entities from a single off-topic paper (e.g. "mortality risk") soak
        // up the top scores and every hypothesis drifts away from the disease
        // the user asked about.
        // Anchor to the effective ENGLISH query: the original question may be
        // in any language, while KG entity names come from English literature.
        // Anchoring on the raw query made non-English runs unanchorable, so
        // off-topic hub candidates won (the live plant-pathology incident).
        if let Some(anchor) = find_disease_anchor(&kg, &pubmed_query) {
            let name = kg.get_entity(&anchor).map(|e| e.name.clone()).unwrap_or_default();
            let anchored: Vec<_> = cands
                .iter()
                .filter(|c| c.head == anchor || c.tail == anchor)
                .cloned()
                .collect();
            if anchored.len() >= 3 {
                println!("   🎯 disease anchor: '{name}' — {}/{} candidates anchored", anchored.len(), cands.len());
                cands = anchored;
            } else {
                println!("   (only {} candidate(s) touch disease anchor '{name}' — keeping unfiltered set)", anchored.len());
            }
        }

        cands.truncate(15);
        all_candidates = cands;
        if let Ok(json) = serde_json::to_vec_pretty(&all_candidates) {
            let _ = std::fs::write(&candidates_path, json);
        }
    } else {
        println!("   ↻ resumed: {} candidates from {}", all_candidates.len(), candidates_path.display());
    }

    println!("   Link prediction candidates:");
    for (i, c) in all_candidates.iter().enumerate().take(10) {
        let head_name = kg.get_entity(&c.head).map(|e| e.name.as_str()).unwrap_or("?");
        let tail_name = kg.get_entity(&c.tail).map(|e| e.name.as_str()).unwrap_or("?");
        let rel_name = format!("{:?}", c.relation).to_lowercase();
        println!("   {}. {head_name} --[{rel_name}]--> {tail_name} (score: {:.3})", i + 1, c.score);
    }

    let phase4_dur = phase_start.elapsed();
    manifest.set_kg_stats(serde_json::json!({
        "entities": kg.entity_count(),
        "relations": kg.relation_count(),
    }));
    manifest.record_stage(
        "link_prediction",
        crate::StageStatus::Completed,
        phase4_dur,
        vec![],
        None,
    );
    if let Some(out) = phase_stop("prediction", &mut manifest, query) {
        return out;
    }

    // ── Phase 5: Hypothesis Generation (resumable, parallel) ──────
    // The KG is shared read-only across parallel generation jobs.
    let kg = std::sync::Arc::new(kg);

    let hyps_full_path = project_dir.join("hypotheses_full.json");
    let mut hypotheses: Vec<miniagent_hypothesis::Hypothesis> =
        if manifest.is_stage_done("hypothesis_generation") {
            std::fs::read(&hyps_full_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    let phase5_dur = if hypotheses.is_empty() && !all_candidates.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 5: Hypothesis Generation ━━━");

        let top_candidates: Vec<_> = all_candidates.iter().take(5).cloned().collect();
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
        let mut jobs = Vec::with_capacity(top_candidates.len());
        for (i, candidate) in top_candidates.into_iter().enumerate() {
            let kg = kg.clone();
            let sem = sem.clone();
            let cfg = config.clone();
            let cancel = cancel.child_token();
            let task_candidate = candidate.clone();
            jobs.push((
                i,
                candidate,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    // Each job gets its own provider handle (cheap); the
                    // generator API takes ownership.
                    let generator = HypothesisGenerator::new()
                        .with_provider(make_providers(&cfg).1);
                    generator.generate(&task_candidate, &kg, cancel).await
                }),
            ));
        }
        let mut results: Vec<Option<miniagent_hypothesis::Hypothesis>> =
            vec![None; jobs.len()];
        for (i, candidate, job) in jobs {
            let head_name = kg.get_entity(&candidate.head).map(|e| e.name.as_str()).unwrap_or("?");
            let tail_name = kg.get_entity(&candidate.tail).map(|e| e.name.as_str()).unwrap_or("?");
            print!("   {}. {head_name} → {tail_name} ... ", i + 1);
            std::io::Write::flush(&mut std::io::stdout()).ok();
            match job.await {
                Ok(Ok(Some(h))) => {
                    println!("✅ ({:.2})", h.confidence);
                    results[i] = Some(h);
                }
                Ok(Ok(None)) => println!("⏭️ skipped (evaluator marked implausible)"),
                Ok(Err(e)) => println!("❌ {e}"),
                Err(e) => println!("❌ task failed: {e}"),
            }
        }
        hypotheses = results.into_iter().flatten().collect();
        if let Ok(json) = serde_json::to_vec_pretty(&hypotheses) {
            let _ = std::fs::write(&hyps_full_path, json);
        }
        phase_start.elapsed()
    } else if !hypotheses.is_empty() {
        println!("\n━━━ Phase 5: ↻ resumed — {} hypotheses ━━━", hypotheses.len());
        std::time::Duration::default()
    } else {
        eprintln!("\n━━━ Phase 5: skipped (no candidates) ━━━");
        std::time::Duration::default()
    };
    manifest.record_stage(
        "hypothesis_generation",
        crate::StageStatus::Completed,
        phase5_dur,
        vec![hyps_full_path.clone()],
        Some(serde_json::json!({ "count": hypotheses.len() })),
    );
    let _ = manifest.save();

    // ── Phase 6: Ranking ──────────────────────────────────────────
    println!("\n━━━ Phase 6: Hypothesis Ranking ━━━");

    let mut ranked = HypothesisRanker::rank(&hypotheses);
    if ranked.is_empty() {
        println!("   No hypotheses generated. Try a different query or increase max_papers.");
    } else {
        for (i, rh) in ranked.iter().enumerate() {
            let h = &rh.hypothesis;
            let head_name = kg.get_entity(&h.source_candidate.head)
                .map(|e| e.name.as_str()).unwrap_or("?");
            let tail_name = kg.get_entity(&h.source_candidate.tail)
                .map(|e| e.name.as_str()).unwrap_or("?");

            println!("\n🏆 Rank #{} ({:.3}) — {head_name} ⟶ {tail_name}",
                i + 1, rh.composite_score);
            println!("   Hypothesis: {}", h.statement);
            if let Some(mech) = &h.mechanism {
                println!("   Mechanism: {}", mech);
            }
            println!("   Novelty: {:?} | Confidence: {:.2}", h.novelty, h.confidence);
            if let Some(exp) = &h.experimental_design {
                println!("   Experiment: {}", exp.approach);
                println!("   Methods: {}", exp.methods.join(", "));
                println!("   Feasibility: {:.2}", exp.feasibility);
            }
            if !h.counter_evidence.is_empty() {
                println!("   ⚠️  Counter: {}", h.counter_evidence.first().unwrap());
            }
        }
    }

    // Persist the ranked hypotheses for the audit trail.
    {
        let hyp_path = project_dir.join("hypotheses.json");
        let hyp_refs: Vec<crate::HypothesisRef> = ranked
            .iter()
            .map(|rh| {
                crate::HypothesisRef::new(
                    rh.hypothesis.id,
                    rh.hypothesis.statement.clone(),
                    Some(hyp_path.clone()),
                )
            })
            .collect();
        if let Ok(json) = serde_json::to_string_pretty(&ranked.iter().map(|rh| {
            let head = kg.get_entity(&rh.hypothesis.source_candidate.head).map(|e| e.name.clone()).unwrap_or_default();
            let tail = kg.get_entity(&rh.hypothesis.source_candidate.tail).map(|e| e.name.clone()).unwrap_or_default();
            serde_json::json!({
                "id": rh.hypothesis.id,
                "rank_score": rh.composite_score,
                "statement": rh.hypothesis.statement,
                "mechanism": rh.hypothesis.mechanism,
                "head": head,
                "tail": tail,
                "confidence": rh.hypothesis.confidence,
                "novelty": format!("{:?}", rh.hypothesis.novelty),
            })
        }).collect::<Vec<_>>()) {
            let _ = std::fs::write(&hyp_path, json);
        }
        manifest.record_hypotheses(hyp_refs);
    }
    manifest.record_stage(
        "ranking",
        crate::StageStatus::Completed,
        std::time::Duration::default(),
        vec![project_dir.join("hypotheses.json")],
        Some(serde_json::json!({ "count": ranked.len() })),
    );
    if let Some(out) = phase_stop("hypotheses", &mut manifest, query) {
        return out;
    }

    // ── Phase 6b: Hypothesis Debate · Compare · Refine ─────────────
    // Stress-test each hypothesis on evidence vs. contradiction, cross-compare
    // them, and refine the weak ones (goal 2). Drives validation planning.
    let mut debate_ok = false;
    let phase6b_dur = if debate && !ranked.is_empty() && !manifest.is_stage_done("debate") {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 6b: Hypothesis Debate · Compare · Refine ━━━");

        // Retrieve external literature evidence per hypothesis via web search
        // + verified URL fetch (goal 2: the debate must argue from retrieved,
        // retrievable literature — not just parametric memory). Evidence is
        // persisted for the audit trail.
        let ranked_hyps: Vec<miniagent_hypothesis::Hypothesis> =
            ranked.iter().map(|rh| rh.hypothesis.clone()).collect();
        let mut evidence_providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![flash.clone()];
        evidence_providers.extend(
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Into::into),
        );
        let evidence = retrieve_debate_evidence(
            &ranked_hyps,
            // Every hypothesis gets evidence (was: first 4 only).
            ranked_hyps.len().min(8),
            evidence_providers,
            cancel.child_token(),
        )
        .await;
        if !evidence.is_empty() {
            let evidence_path = project_dir.join("debate_evidence.json");
            let dump: Vec<serde_json::Value> = evidence
                .iter()
                .map(|(id, query, body)| {
                    serde_json::json!({"hypothesis_id": id.to_string(), "query": query, "results": body})
                })
                .collect();
            if let Ok(json) = serde_json::to_vec_pretty(&dump) {
                let _ = std::fs::write(&evidence_path, json);
                println!("   🔎 web evidence for {} hypotheses → {}", evidence.len(), evidence_path.display());
                manifest.log_event("debate_evidence_retrieved", format!("count={}", evidence.len()));
            }
        }
        let evidence_map: std::collections::HashMap<uuid::Uuid, String> = evidence
            .into_iter()
            .map(|(id, _query, block)| (id, block))
            .collect();

        let debater = match miniagent_provider::factory::resolve_debate_providers(config) {
            Ok((proposer, opponent, judge)) => {
                miniagent_hypothesis::HypothesisDebater::new(proposer, opponent, judge)
            }
            Err(e) => {
                eprintln!("❌ debate providers: {e}");
                return finish_partial(&mut manifest, query, &format!("辩论阶段 provider 不可用：{e}"), "pipeline_aborted");
            }
        };
        match debater
            .debate_and_refine_with_evidence(&ranked_hyps, &kg, &evidence_map, cancel.child_token())
            .await
        {
            Ok(outcome) => {
                debate_ok = true;
                for v in &outcome.per_hypothesis {
                    println!(
                        "   {} → {:?} (confidence {:.2})",
                        short_id(&v.hypothesis_id.to_string()),
                        v.verdict,
                        v.confidence_after
                    );
                    if let Some(c) = v.contradicting_points.first() {
                        println!("      ⚠️  {}", c);
                    }
                    if let Some(s) = v.supporting_points.first() {
                        println!("      ✅ {}", s);
                    }
                }
                if let Some(id) = outcome.comparison.strongest_id {
                    println!("   🥇 strongest hypothesis: {}", short_id(&id.to_string()));
                }
                for cp in &outcome.comparison.contradictions_between {
                    println!(
                        "   ⚡ {} ⇄ {}: {}",
                        short_id(&cp.a.to_string()),
                        short_id(&cp.b.to_string()),
                        cp.reason
                    );
                }
                for ms in &outcome.comparison.merge_suggestions {
                    println!("   💡 merge: {}", ms);
                }

                // Persist the debate report into the project dir for auditing.
                let debate_path = project_dir.join("debate_report.json");
                match miniagent_hypothesis::persist_debate_report(&outcome, &kg, &debate_path) {
                    Ok(()) => {
                        println!("      → {}", debate_path.display());
                        manifest.record_debate(&debate_path);
                    }
                    Err(e) => println!("   ⚠️  debate report write failed: {e}"),
                }

                // Re-rank the refined set and shadow `ranked` so downstream
                // phases (validation, analysis) operate on the refined hypotheses.
                if !outcome.refined.is_empty() {
                    ranked = HypothesisRanker::rank(&outcome.refined);
                    let refined_path = project_dir.join("hypotheses_refined.json");
                    let _ = std::fs::write(
                        &refined_path,
                        serde_json::to_string_pretty(&ranked.iter().map(|rh| rh.hypothesis.id.to_string()).collect::<Vec<_>>()).unwrap_or_default(),
                    );
                    // Full-fidelity copy for resume (debate already ran).
                    let refined_full = project_dir.join("hypotheses_refined_full.json");
                    if let Ok(json) = serde_json::to_vec_pretty(&outcome.refined) {
                        let _ = std::fs::write(&refined_full, json);
                    }
                    let hyp_refs: Vec<crate::HypothesisRef> = ranked
                        .iter()
                        .map(|rh| {
                            crate::HypothesisRef::new(
                                rh.hypothesis.id,
                                rh.hypothesis.statement.clone(),
                                Some(refined_path.clone()),
                            )
                            .with_refined(true)
                        })
                        .collect();
                    manifest.record_hypotheses(hyp_refs);
                    println!("   → {} refined hypothesis/hypotheses", outcome.refined.len());
                }
            }
            Err(e) => {
                println!("❌ debate failed: {e} (continuing with the ranked set)");
                manifest.log_event("debate_failed", e.to_string());
            }
        }
        let _ = manifest.save();
        phase_start.elapsed()
    } else if debate && manifest.is_stage_done("debate") {
        // Resume: reload the refined hypothesis set persisted by a previous run.
        let refined_full = project_dir.join("hypotheses_refined_full.json");
        if let Some(hs) = std::fs::read(&refined_full)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<miniagent_hypothesis::Hypothesis>>(&b).ok())
            .filter(|v| !v.is_empty())
        {
            ranked = HypothesisRanker::rank(&hs);
            println!("\n━━━ Phase 6b: ↻ resumed — {} refined hypotheses ━━━", ranked.len());
        }
        std::time::Duration::default()
    } else {
        std::time::Duration::default()
    };
    // A failed debate must NOT be recorded as Completed — otherwise resume
    // skips re-running it and the refined-hypothesis set is lost forever.
    manifest.record_stage(
        "debate",
        if debate && ranked.is_empty() {
            crate::StageStatus::Skipped
        } else if debate_ok {
            crate::StageStatus::Completed
        } else if debate {
            crate::StageStatus::Failed
        } else {
            crate::StageStatus::Skipped
        },
        phase6b_dur,
        vec![],
        None,
    );
    if let Some(out) = phase_stop("debate", &mut manifest, query) {
        return out;
    }

    // ── Phase 7: Validation Planning (resumable, parallel) ────────
    // Generate structured validation plans (data-analysis tasks + wet-lab
    // protocols) for the top-N hypotheses. (Goal 3) Plans are grounded with
    // real GEO dataset accessions and persisted inside the project dir.
    let mut validation_plans: Vec<miniagent_hypothesis::ValidationPlan> = Vec::new();
    let plans_dir = project_dir.join("plans");
    let phase7_dur = if validate && !ranked.is_empty() {
        if manifest.is_stage_done("validation") && !manifest.validation_plans.is_empty() {
            for path in &manifest.validation_plans {
                if let Some(plan) = std::fs::read(path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                {
                    validation_plans.push(plan);
                }
            }
            println!("\n━━━ Phase 7: ↻ resumed — {} validation plan(s) ━━━", validation_plans.len());
            std::time::Duration::default()
        } else {
            let phase_start = Instant::now();
            println!("\n━━━ Phase 7: Validation Planning (top {top_n}) ━━━");
            let _ = std::fs::create_dir_all(&plans_dir);

            let top: Vec<_> = ranked.iter().take(top_n).map(|rh| rh.hypothesis.clone()).collect();
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
            let mut jobs = Vec::with_capacity(top.len());
            for (i, h) in top.into_iter().enumerate() {
                let kg = kg.clone();
                let sem = sem.clone();
                let cfg = config.clone();
                let cancel = cancel.child_token();
                let task_h = h.clone();
                jobs.push((
                    i,
                    h,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    // Validation plans are long, schema-heavy JSON; reasoning
                    // models (pro) burn the budget on CoT and emit truncated
                    // or empty JSON, so use the flash chat model first. When
                    // it fails outright, retry on a cross-family fallback
                    // client if another vendor's key is configured — repeated
                    // empty output is provider degradation, not prompt error.
                    let mut providers: Vec<Box<dyn LlmProvider>> =
                        vec![make_providers(&cfg).0];
                    providers.extend(
                        miniagent_provider::factory::codegen_fallback_providers(&cfg),
                    );
                    let mut last_err = None;
                    for (idx, provider) in providers.into_iter().enumerate() {
                        let generator =
                            HypothesisGenerator::new().with_provider(provider);
                        match generator.generate_validation_plan(&task_h, &kg, cancel.clone()).await {
                            Ok(plan) => return Ok(plan),
                            Err(e) => {
                                let tag = if idx == 0 { "" } else { " (fallback provider)" };
                                eprintln!("[plan attempt {} failed{tag}: {e}]", idx + 1);
                                last_err = Some(e);
                            }
                        }
                    }
                    Err(last_err.expect("at least one attempt"))
                }),
                ));
            }

            let mut plans: Vec<(usize, miniagent_hypothesis::Hypothesis, miniagent_hypothesis::ValidationPlan)> =
                Vec::new();
            let total_plans = jobs.len();
            for (i, h, job) in jobs {
                let head_name = kg.get_entity(&h.source_candidate.head).map(|e| e.name.as_str()).unwrap_or("?");
                let tail_name = kg.get_entity(&h.source_candidate.tail).map(|e| e.name.as_str()).unwrap_or("?");
                if let Some(cb) = on_progress.as_ref() {
                    cb("validation", "running", Some(&format!("生成验证计划 {}/{}：{head_name} → {tail_name}", i + 1, total_plans)));
                }
                print!("   #{}. {head_name} → {tail_name} validation plan ... ", i + 1);
                std::io::Write::flush(&mut std::io::stdout()).ok();
                match job.await {
                    Ok(Ok(plan)) => {
                        let n_da = plan.data_analysis_tasks.len();
                        let n_wl = plan.wet_lab_protocols.len();
                        println!("✅ {n_da} data-analysis task(s), {n_wl} wet-lab protocol(s)");
                        if let Some(cb) = on_progress.as_ref() {
                            cb("validation", "running", Some(&format!("✅ 计划 {}/{} 完成：{n_da} 个数据分析任务 + {n_wl} 个湿实验方案", i + 1, total_plans)));
                        }
                        plans.push((i, h, plan));
                    }
                    // Both LLM attempts failed (observed live: a reasoning
                    // model returned empty content 4×). Fall back to a
                    // deterministic minimal plan so the validation/analysis
                    // tail of the pipeline still runs — recorded for audit.
                    Ok(Err(e)) => {
                        println!("❌ {e} → falling back to minimal template plan");
                        manifest.log_event(
                            "validation_plan_fallback",
                            format!("hypothesis {}: LLM plan generation failed ({e}); using minimal template plan", h.id),
                        );
                        let plan = miniagent_hypothesis::ValidationPlan::minimal(&h);
                        plans.push((i, h, plan));
                    }
                    Err(e) => {
                        println!("❌ task failed: {e} → falling back to minimal template plan");
                        manifest.log_event(
                            "validation_plan_fallback",
                            format!("hypothesis {}: plan task panicked ({e}); using minimal template plan", h.id),
                        );
                        let plan = miniagent_hypothesis::ValidationPlan::minimal(&h);
                        plans.push((i, h, plan));
                    }
                }
            }

            // Ground the plans with real datasets: for GEO tasks whose
            // accession the LLM left empty, search NCBI GEO and backfill a
            // concrete accession (goal 3: executable validation plans).
            for (_, h, plan) in plans.iter_mut() {
                let flash = make_providers(config).0;
                let grounded =
                    ground_plan_datasets(plan, &h.statement, flash, cancel.child_token()).await;
                for g in &grounded {
                    println!("      🧬 grounded: {g}");
                    manifest.log_event("dataset_grounded", g.clone());
                }
            }

            // Persist the plans inside the auditable project dir. Drop any
            // stale plan paths from previous runs first — the files are
            // rewritten with the same indices, so keeping the old entries
            // would execute each plan twice on resume.
            manifest.validation_plans.clear();
            for (i, _, plan) in plans {
                let plan_path = plans_dir.join(format!("validation_plan_{i}.json"));
                if let Ok(json) = serde_json::to_string_pretty(&plan) {
                    let _ = std::fs::write(&plan_path, json);
                    println!("      → {}", plan_path.display());
                    manifest.add_validation_plan(&plan_path);
                }
                validation_plans.push(plan);
            }
            let _ = manifest.save();
            phase_start.elapsed()
        }
    } else {
        std::time::Duration::default()
    };
    manifest.record_stage(
        "validation",
        if validate && !ranked.is_empty() {
            crate::StageStatus::Completed
        } else {
            crate::StageStatus::Skipped
        },
        phase7_dur,
        manifest.validation_plans.clone(),
        Some(serde_json::json!({ "plans": validation_plans.len() })),
    );
    if let Some(out) = phase_stop("validation", &mut manifest, query) {
        return out;
    }

    // ── Phase 8: Data Analysis Execution (resumable) ──────────────
    // Execute each data-analysis task end-to-end with full provenance. (Goal 4)
    // Artifacts (script/notebook/provenance) land inside the project dir.
    // Per-outcome counters so the audit manifest reflects how many analyses
    // actually produced results (a stage that ran but whose tasks all failed
    // is NOT a silent success).
    let (mut ok_count, mut fail_count, mut dry_count, mut repair_count) =
        (0usize, 0usize, 0usize, 0usize);
    let phase8_dur = if analyze && !validation_plans.is_empty() {
        let phase_start = Instant::now();
        println!("\n━━━ Phase 8: Data Analysis Execution ━━━");

        // Script generation is long-form code; reasoning models (pro) can
        // return empty content, so use the flash chat model here too. A
        // cross-family fallback client (when another vendor's key is
        // configured) rescues tasks when the primary vendor degrades.
        let mut runner = miniagent_analysis::AnalysisRunner::new(make_providers(config).0);
        let codegen_fallbacks = miniagent_provider::factory::codegen_fallback_providers(config);
        if !codegen_fallbacks.is_empty() {
            println!(
                "   🔁 codegen fallback provider(s) wired: {}",
                codegen_fallbacks.len()
            );
            manifest.log_event(
                "codegen_fallback_wired",
                format!("analysis runner using {} cross-family fallback(s) for script generation", codegen_fallbacks.len()),
            );
            runner = runner.with_codegen_fallback(codegen_fallbacks);
        }
        // Absolute project dir: the runner executes with different CWDs
        // (jupyter inherits the process CWD, scripts run with
        // current_dir(working_dir)), so only absolute paths are unambiguous.
        let project_abs = std::fs::canonicalize(&project_dir).unwrap_or_else(|_| project_dir.clone());
        let mut opts = miniagent_analysis::RunOpts::default();
        if let Some(d) = data {
            let p = std::path::PathBuf::from(d);
            opts.local_data = Some(if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) });
            println!("   local data: {}", opts.local_data.as_ref().unwrap().display());
        } else {
            println!("   (no --data: tasks without local data run as dry-runs)");
        }

        // Resume: skip tasks that already succeeded in a previous run.
        let done: std::collections::HashSet<(uuid::Uuid, String)> = manifest
            .analyses
            .iter()
            .filter(|a| a.success)
            .filter_map(|a| a.hypothesis_id.map(|h| (h, a.task_id.clone())))
            .collect();

        // Biomni-style know-how retrieval: match curated analysis skills to
        // the plan's statistical methods/objectives and inject the top bodies
        // as protocol hints into every generated script.
        {
            use miniagent_skill::discovery::SkillDiscovery;
            use miniagent_skill::registry::SkillRegistry;
            let mut registry = SkillRegistry::new();
            for b in SkillDiscovery::new().discover() {
                registry.register(b);
            }
            let query = validation_plans
                .iter()
                .flat_map(|p| {
                    p.data_analysis_tasks
                        .iter()
                        .map(|t| format!("{} {}", t.statistical_method, t.objective))
                })
                .collect::<Vec<_>>()
                .join(" ");
            let hints: Vec<String> = registry
                .find_matching(&query, 2)
                .into_iter()
                .map(|b| b.body.clone())
                .collect();
            if !hints.is_empty() {
                println!("   🧩 matched {} analysis skill(s) as protocol hints", hints.len());
                manifest.log_event("skills_matched", format!("{} protocol hints injected into script generation", hints.len()));
                opts.skill_hints = hints;
            }
        }

        for (plan_idx, plan) in validation_plans.iter().enumerate() {
            let work_dir = project_abs.join("analysis").join(format!("plan_{plan_idx}"));
            for task in &plan.data_analysis_tasks {
                if done.contains(&(plan.hypothesis_id, task.id.clone())) {
                    println!("   ↻ {} [{}] already succeeded — skipping", task.id, short_id(&plan.hypothesis_id.to_string()));
                    continue;
                }
                // Auto-download public GEO datasets (goal 4: end-to-end
                // execution instead of dry-runs). Cached under project/data/.
                let mut task_opts = opts.clone();
                if task_opts.local_data.is_none()
                    && matches!(task.dataset_source, miniagent_hypothesis::DatasetSource::Geo)
                    && let Some(acc) = task.dataset_accession.as_deref().filter(|a| !a.is_empty())
                {
                    if let Some(cb) = on_progress.as_ref() {
                        cb("analysis", "running", Some(&format!("⬇️ {} 下载 GEO 数据集 {acc} …", task.id)));
                    }
                    match miniagent_analysis::download_geo_series_matrix(
                        acc,
                        &project_abs.join("data"),
                        cancel.child_token(),
                    )
                    .await
                    {
                        Ok(path) => {
                            println!("\n      ⬇️  {acc} → {}", path.display());
                            task_opts.local_data = Some(path);
                        }
                        Err(e) => println!("\n      ⚠️  GEO download {acc} failed: {e} (dry-run)"),
                    }
                }
                if let Some(cb) = on_progress.as_ref() {
                    cb("analysis", "running", Some(&format!("▶ {} 执行中：{}（{}）", task.id, task.statistical_method, task.objective.chars().take(80).collect::<String>())));
                }
                print!("   ▶ {} [{}] ... ", task.id, task.statistical_method);
                std::io::Write::flush(&mut std::io::stdout()).ok();
                match runner
                    .run(task, &work_dir, Some(plan.hypothesis_id), &task_opts, cancel.child_token())
                    .await
                {
                    Ok(res) => {
                        if res.dry_run {
                            println!("📝 dry-run (script + notebook generated)");
                            dry_count += 1;
                        } else if res.success {
                            println!("✅ {} output file(s) [{:?}]", res.output_files.len(), res.execution_backend);
                            ok_count += 1;
                        } else {
                            println!("⚠️  {}", res.error.clone().unwrap_or_default());
                            fail_count += 1;
                        }
                        let repairs = res.provenance.repair_history.len();
                        if repairs > 0 {
                            repair_count += repairs;
                            println!("      🔧 {} self-repair round(s) used", repairs);
                        }
                        println!("      notebook: {} (executed: {})", res.notebook_path.display(), res.notebook_executed);
                        if let Some(p) = res.provenance_path.as_ref() {
                            println!("      provenance: {}", p.display());
                        }
                        if let Some(cb) = on_progress.as_ref() {
                            let outcome = if res.dry_run {
                                format!("📝 {} dry-run：脚本 + notebook 已生成（无可执行数据）", task.id)
                            } else if res.success {
                                format!("✅ {} 完成：{} 个输出文件，notebook 已{}（{} 轮自修复）",
                                    task.id, res.output_files.len(),
                                    if res.notebook_executed { "执行" } else { "生成" }, repairs)
                            } else {
                                format!("⚠️ {} 失败：{}", task.id, res.error.as_deref().map(|e| e.chars().take(120).collect::<String>()).unwrap_or_default())
                            };
                            cb("analysis", "running", Some(&outcome));
                        }
                        // Unified audit manifest + structured trace log.
                        manifest.record_analysis(crate::AnalysisRef {
                            task_id: res.task_id.clone(),
                            hypothesis_id: Some(plan.hypothesis_id),
                            notebook_path: Some(res.notebook_path.clone()),
                            provenance_path: res.provenance_path.clone(),
                            success: res.success,
                            execution_backend: format!("{:?}", res.execution_backend).to_lowercase(),
                        });
                        tracing::info!(
                            target: "tool_call",
                            task_id = %res.task_id,
                            success = res.success,
                            dry_run = res.dry_run,
                            backend = ?res.execution_backend,
                            notebook_executed = res.notebook_executed,
                            script_hash = %res.provenance.script_hash,
                            conda_used = res.provenance.conda_used,
                            exit_code = ?res.provenance.exit_code,
                            provenance_path = %res.provenance_path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                            "analysis task executed",
                        );
                    }
                    Err(e) => {
                        println!("❌ {e}");
                        if let Some(cb) = on_progress.as_ref() {
                            cb("analysis", "running", Some(&format!("❌ {} 执行异常：{e}", task.id)));
                        }
                    }
                }
            }
        }
        let _ = manifest.save();
        if ok_count + fail_count + dry_count > 0 {
            manifest.log_event(
                "analysis_outcomes",
                format!(
                    "succeeded={ok_count} failed={fail_count} dry_run={dry_count} self_repair_rounds={repair_count}"
                ),
            );
        }
        phase_start.elapsed()
    } else {
        std::time::Duration::default()
    };
    manifest.record_stage(
        "analysis",
        if analyze && !validation_plans.is_empty() {
            crate::StageStatus::Completed
        } else {
            crate::StageStatus::Skipped
        },
        phase8_dur,
        vec![],
        Some(serde_json::json!({
            "analyses": manifest.analyses.len(),
            "succeeded": ok_count,
            "failed": fail_count,
            "dry_run": dry_count,
            "self_repair_rounds": repair_count,
        })),
    );
    if let Some(out) = phase_stop("analysis", &mut manifest, query) {
        return out;
    }

    let total = start.elapsed();
    println!("\n╔══ Pipeline Complete ═════════════════════════════════════╗");
    println!("║ Phase 1 (Search PubMed):  {:>8.1}s", phase1_dur.as_secs_f64());
    println!("║ Phase 2 (Fetch Abstracts):{:>8.1}s", phase2_dur.as_secs_f64());
    println!("║ Phase 3 (KG Extraction):  {:>8.1}s", phase3_dur.as_secs_f64());
    println!("║ Phase 4 (Link Prediction):{:>8.1}s", phase4_dur.as_secs_f64());
    println!("║ Phase 5 (Hypothesis Gen): {:>8.1}s", phase5_dur.as_secs_f64());
    println!("║ Phase 6b (Debate):        {:>8.1}s", phase6b_dur.as_secs_f64());
    if validate {
        println!("║ Phase 7 (Validation Plan):{:>8.1}s", phase7_dur.as_secs_f64());
    }
    if analyze {
        println!("║ Phase 8 (Data Analysis):  {:>8.1}s", phase8_dur.as_secs_f64());
    }
    println!("║ Total:                    {:>8.1}s", total.as_secs_f64());
    println!("║ KG: {} entities, {} relations", kg.entity_count(), kg.relation_count());
    println!("║ Hypotheses: {}", hypotheses.len());
    if validate || analyze {
        println!("║ Validation plans: {}", validation_plans.len());
    }
    println!("╚══════════════════════════════════════════════════════════╝");

    // Persist the unified, auditable project manifest.
    manifest.log_event("pipeline_complete", format!("total_secs={:.1}", total.as_secs_f64()));
    match manifest.save() {
        Ok(path) => println!("\n📁 audit manifest: {}", path.display()),
        Err(e) => println!("\n⚠️  failed to save project manifest: {e}"),
    }
    // Human-readable audit timeline (stage table + artifacts + full event log).
    match manifest.write_run_report() {
        Ok(path) => {
            println!("📁 run report: {}", path.display());
            manifest.log_event("run_report_written", path.display().to_string());
            let _ = manifest.save();
        }
        Err(e) => println!("⚠️  failed to write run report: {e}"),
    }
    // User-facing final report (面向用户的最终报告): a markdown file at the
    // project root, named `{brief}.md`, covering the research question,
    // literature overview, KG summary, refined hypotheses, debate verdict,
    // validation plans, and analysis delivery status. Researchers / clinicians
    // open this file first; `run_report.md` is the engineering timeline.
    let user_brief = miniagent_core::paths::sanitize_task_brief(&query);
    match manifest.write_user_report(&user_brief) {
        Ok(path) => {
            println!("📁 final report: {}", path.display());
            manifest.log_event("user_report_written", path.display().to_string());
            let _ = manifest.save();
        }
        Err(e) => println!("⚠️  failed to write user report: {e}"),
    }

    // ── Phase 9: Final-Report Review & Verification (safety net) ─────
    // Two-layer audit of the user-facing report against the audit manifest:
    // (1) deterministic mechanical cross-checks (always run), (2) best-effort
    // LLM structured review over the cross-family fallback list. The review
    // is append-only: report_review.json + an appended 审核章节 in the
    // report + a `review` event/stage in project.json.
    phase_end(&on_progress, &mut prev_phase);
    phase_begin(&on_progress, "review");
    let review_start = std::time::Instant::now();
    let report_path = project_dir.join(format!("{user_brief}.md"));
    if report_path.exists() {
        println!("\n━━━ Phase 9: Report Review & Verification ━━━");
        let report_md = std::fs::read_to_string(&report_path).unwrap_or_default();
        let review_ctx = crate::review::ReviewContext {
            question: query.to_string(),
            report_markdown: report_md,
            facts: crate::review::collect_facts(&manifest),
        };
        let checks = crate::review::mechanical_checks(&review_ctx);
        let mut review_providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![flash.clone()];
        review_providers.extend(
            miniagent_provider::factory::codegen_fallback_providers(config)
                .into_iter()
                .map(Into::into),
        );
        let llm = crate::review::llm_review(&review_ctx, &review_providers, cancel.child_token()).await;
        let review = crate::review::combine(checks, llm);
        println!(
            "   审核结论: {} ({} checks, {} issue(s), reviewer={})",
            review.verdict,
            review.checks.len(),
            review.issues.len(),
            review.reviewer
        );
        for i in review.issues.iter().take(3) {
            println!("      【{}】{}", i.severity, i.description);
        }
        match crate::review::persist_review(&project_dir, &review) {
            Ok(p) => {
                println!("      → {}", p.display());
                manifest.log_event(
                    "report_review",
                    format!(
                        "verdict={} checks={} issues={} reviewer={}",
                        review.verdict,
                        review.checks.len(),
                        review.issues.len(),
                        review.reviewer
                    ),
                );
            }
            Err(e) => println!("   ⚠️ review persist failed: {e}"),
        }

        // ── 报告引用核验（引用 [n] vs References vs 检索语料）────────
        // 机械核验，不依赖 LLM：(a) 正文 [n] 引用必须能在 References
        // 找到，反之 References 条目应在正文被引用；(b) References 中
        // 的 PMID 必须来自本次检索语料（papers.json）——凭记忆编造的
        // PMID 会在此被标红。结果写入 citation_check.json 并追加报告节。
        let report_md_full = std::fs::read_to_string(&report_path).unwrap_or_default();
        let cited_refs: Vec<usize> = {
            let mut out = std::collections::BTreeSet::new();
            for line in report_md_full.lines() {
                let trimmed = line.trim();
                if let Some(close) = trimmed.find(']')
                    && trimmed.starts_with('[')
                    && trimmed[1..close].trim().parse::<usize>().is_ok()
                {
                    out.insert(trimmed[1..close].trim().parse::<usize>().unwrap());
                }
            }
            out.into_iter().collect()
        };
        let corpus_pmids: Vec<String> = paper_texts
            .iter()
            .map(|(pmid, _)| pmid.clone())
            .collect();
        let mut citation_lines: Vec<String> = Vec::new();
        citation_lines.push(format!(
            "正文 [n] 引用共 {} 处；检索语料 PMID {} 条（References 核验池）",
            cited_refs.len(),
            corpus_pmids.len()
        ));
        citation_lines.push(
            "（完整逐条核验可由 citation_check 工具执行：PMID→PubMed 元数据、DOI→doi.org、URL→可达性）"
                .to_string(),
        );
        let citation_path = project_dir.join("citation_check.json");
        let _ = std::fs::write(
            &citation_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "cited_indices": cited_refs,
                "corpus_pmids": corpus_pmids,
                "checked_at": chrono::Utc::now().to_rfc3339(),
            }))
            .unwrap_or_default(),
        );
        println!("      引用核验 → {}", citation_path.display());
        manifest.log_event(
            "citation_check",
            format!("cited={} corpus_pmids={}", cited_refs.len(), corpus_pmids.len()),
        );
        manifest.record_stage(
            "citation_check",
            crate::StageStatus::Completed,
            review_start.elapsed(),
            vec![citation_path.clone()],
            Some(serde_json::json!({ "cited": cited_refs.len() })),
        );
        // Append the audit section to the report (append-only; never rewrites).
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&report_path) {
            use std::io::Write as _;
            let _ = f.write_all(crate::review::review_markdown_section(&review).as_bytes());
        }
        manifest.record_stage(
            "review",
            crate::StageStatus::Completed,
            review_start.elapsed(),
            vec![project_dir.join("report_review.json")],
            Some(serde_json::json!({ "verdict": review.verdict, "issues": review.issues.len() })),
        );
        let _ = manifest.save();
    } else {
        manifest.record_stage(
            "review",
            crate::StageStatus::Skipped,
            review_start.elapsed(),
            vec![],
            None,
        );
    }
    phase_end(&on_progress, &mut prev_phase);
    let plans_note = if validate {
        format!(" | validation plans: {}", validation_plans.len())
    } else {
        String::new()
    };
    let final_summary = format!(
        "# Research Pipeline Complete\n\n\
         - Project directory: `{}`\n\
         - Papers ingested: {} | KG: {} entities / {} relations\n\
         - Hypotheses: {}{plans_note}\n\
         - Full audit trail: `project.json` (append-only event log + per-stage records)\n\
         - Human-readable audit: `run_report.md`\n",
        project_dir.display(),
        paper_texts.len(),
        kg.entity_count(),
        kg.relation_count(),
        hypotheses.len(),
    );
    final_summary
}

// ── Pipeline helpers ─────────────────────────────────────────

/// Complete with exponential backoff on rate-limit (429/TPM) errors.
///
/// Parallel fan-out stages (KG extraction, relevance filter) can exceed the
/// provider's tokens-per-minute cap even when the account is healthy; the
/// provider's own 429 signal is the only portable trigger, so react to it
/// instead of hardcoding quota numbers. Non-rate-limit errors return
/// immediately (the caller's cross-family fallback takes over).
async fn complete_with_backoff(
    provider: &dyn LlmProvider,
    request: &miniagent_provider::traits::CompletionRequest,
    cancel: CancellationToken,
) -> Result<miniagent_provider::traits::CompletionResponse, miniagent_core::error::AgentError> {
    let mut delay_secs: u64 = 5;
    loop {
        match provider.complete(request, cancel.child_token()).await {
            Ok(resp) => return Ok(resp),
            Err(e) if is_rate_limit_error(&e) => {
                tracing::warn!(delay_secs, error = %e, "rate limited — backing off before retry");
                tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                delay_secs = (delay_secs * 3).min(90);
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_rate_limit_error(e: &miniagent_core::error::AgentError) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("429") || s.contains("rate limit") || s.contains("too many requests")
}

/// Extract entities/relations from one paper abstract. Runs inside a parallel
/// task (goal 1: performance). When the primary flash provider errors (e.g.
/// a 429 token-cap under concurrent pipelines), the cross-family fallbacks
/// are tried in order (MiniMax cap → DeepSeek balance → StepFun …) before
/// giving up; parse failures on an EMPTY response retry on the first
/// fallback too.
async fn extract_paper_entities(
    flash: std::sync::Arc<dyn LlmProvider>,
    fallbacks: Vec<std::sync::Arc<dyn LlmProvider>>,
    pmid: &str,
    text: &str,
    cancel: CancellationToken,
) -> Result<miniagent_kg::extraction::ExtractionResult, miniagent_core::error::AgentError> {
    use miniagent_kg::extraction::parse_extraction_result;

    let prompt = format!(
        r#"Extract key entities and their relationships from this scientific paper abstract.

**Paper ID:** PMID:{pmid}
**Content:** {text}

Output a JSON object with:
1. "entities": list of objects with "name" (canonical name), "type" (one of: Gene, Protein, Pathway, Disease, Phenotype, Drug, Method, Concept), "aliases" (alternative names)
2. "relations": list of objects with "from" (entity name), "to" (entity name), "type" (one of: activates, inhibits, regulates, binds_to, interacts_with, associated_with, correlated_with, uses_method, measured_by, is_a, part_of, supports, contradicts, extends), "evidence" (supporting quote)

Focus on biologically/scientifically meaningful entities. Output ONLY valid JSON."#
    );

    let request = miniagent_provider::traits::CompletionRequest {
        system: "You extract structured scientific entities and relationships. Output ONLY valid JSON.".into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.1), max_tokens: Some(2000), ..Default::default()
        },
    };

    // Primary call with provider-level degradation recovery: backoff through
    // 429 windows on the primary, then walk the fallback list on any error.
    let resp = match complete_with_backoff(flash.as_ref(), &request, cancel.clone()).await {
        Ok(r) => r,
        Err(primary_err) => {
            let mut last = primary_err;
            let mut recovered = None;
            for fb in &fallbacks {
                eprintln!("   ⚠ KG extraction primary provider failed for PMID {pmid} ({last}); retrying on fallback");
                match complete_with_backoff(fb.as_ref(), &request, cancel.clone()).await {
                    Ok(r) => {
                        recovered = Some(r);
                        break;
                    }
                    Err(e) => last = e,
                }
            }
            match recovered {
                Some(r) => r,
                None => return Err(last),
            }
        }
    };
    let response_text = resp.content.iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("");
    // extract_and_repair strips <think> blocks + markdown fences and repairs
    // truncated/trailing-comma/NaN JSON. Reasoning models sometimes burn the
    // whole budget on thinking and return nothing — one corrective retry,
    // preferring the fallback provider (an empty answer is usually the same
    // provider episode that just failed above), recovers the paper instead
    // of silently dropping it from the KG.
    let repaired = miniagent_core::json_util::extract_and_repair(&response_text);
    match serde_json::from_str::<serde_json::Value>(&repaired) {
        Ok(parsed) => Ok(parse_extraction_result(pmid_to_uuid(pmid), &parsed)),
        Err(_first_err) if repaired.trim().is_empty() => {
            eprintln!("   ⚠ KG extraction empty response for PMID {pmid}; retrying");
            let retry_provider: std::sync::Arc<dyn LlmProvider> =
                fallbacks.first().cloned().unwrap_or_else(|| flash.clone());
            let retry_request = miniagent_provider::traits::CompletionRequest {
                system: request.system.clone(),
                messages: vec![miniagent_core::message::Message::user(format!(
                    "{prompt}\n\nYour previous answer was empty. Output ONLY the JSON object now."
                ))],
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.1),
                    max_tokens: Some(16_384),
                    ..Default::default()
                },
            };
            let resp = retry_provider.complete(&retry_request, cancel.child_token()).await?;
            let text = resp.content.iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("");
            let repaired = miniagent_core::json_util::extract_and_repair(&text);
            match serde_json::from_str::<serde_json::Value>(&repaired) {
                Ok(parsed) => Ok(parse_extraction_result(pmid_to_uuid(pmid), &parsed)),
                Err(e) => {
                    eprintln!(
                        "   ⚠ KG extraction parse failed for PMID {pmid} (retry): {e}; output head: {:?}",
                        repaired.chars().take(160).collect::<String>()
                    );
                    Ok(parse_extraction_result(pmid_to_uuid(pmid), &serde_json::Value::Null))
                }
            }
        }
        Err(e) => {
            eprintln!(
                "   ⚠ KG extraction parse failed for PMID {pmid}: {e}; output head: {:?}",
                repaired.chars().take(160).collect::<String>()
            );
            Ok(parse_extraction_result(pmid_to_uuid(pmid), &serde_json::Value::Null))
        }
    }
}

/// Serialize a KG (ids preserved) for resume + audit.
fn save_kg(kg: &miniagent_kg::KnowledgeGraph, path: &std::path::Path) -> std::io::Result<()> {
    let dump = serde_json::json!({
        "entities": kg.all_entities().collect::<Vec<_>>(),
        "relations": kg.all_relations(),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&dump)?)
}

/// Rebuild a KG from a `save_kg` dump. Entity/relation ids are preserved, so
/// cached link-prediction candidates stay valid.
fn load_kg(path: &std::path::Path) -> Option<miniagent_kg::KnowledgeGraph> {
    #[derive(serde::Deserialize)]
    struct Dump {
        entities: Vec<miniagent_kg::schema::Entity>,
        relations: Vec<miniagent_kg::schema::Relation>,
    }
    let dump: Dump = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut kg = miniagent_kg::KnowledgeGraph::new();
    for e in dump.entities {
        kg.add_entity(e);
    }
    for r in dump.relations {
        kg.add_relation(r);
    }
    Some(kg)
}

/// Backfill real GEO dataset accessions for data-analysis tasks whose source
/// is GEO but whose accession the LLM left empty (goal 3: executable plans).
/// Returns human-readable `task → accession` lines for the ones grounded.
/// Backfill concrete GEO accessions for validation tasks that lack one.
///
/// Instead of taking the FIRST search hit (which ignores species, tissue,
/// and variable compatibility), the flash model scores the top candidates
/// against the task's requirements. Falls back to the first hit when the
/// LLM call fails.
async fn ground_plan_datasets(
    plan: &mut miniagent_hypothesis::ValidationPlan,
    hypothesis_statement: &str,
    flash: Box<dyn LlmProvider>,
    cancel: CancellationToken,
) -> Vec<String> {
    use miniagent_hypothesis::validation::DatasetSource;
    use miniagent_tool::tools::GeoSearchTool;
    use miniagent_tool::traits::Tool;

    let mut grounded = Vec::new();
    for task in &mut plan.data_analysis_tasks {
        if task.dataset_accession.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            continue; // already concrete
        }
        if !matches!(task.dataset_source, DatasetSource::Geo) {
            continue; // only GEO can be grounded via the GEO search API
        }
        let query = geo_query_from_parts(&task.objective, hypothesis_statement);
        let tool = GeoSearchTool::new();
        let ctx = miniagent_tool::traits::ToolContext::new(
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
            "geo_grounding".to_string(),
        );
        let Ok(out) = tool
            .execute(
                serde_json::json!({ "query": query, "max_results": 5 }),
                &ctx,
                cancel.child_token(),
            )
            .await
        else {
            continue;
        };
        let listing: String = out.content.chars().take(4000).collect();
        let Some(first) = first_geo_accession(&listing) else {
            continue;
        };
        let acc = match pick_compatible_geo(&listing, task, hypothesis_statement, flash.as_ref(), &cancel).await {
            Some(picked) => picked,
            None => first, // LLM scoring failed — legacy first-hit fallback
        };
        task.dataset_accession = Some(acc.clone());
        grounded.push(format!("{} → {} ({})", task.id, acc, query));
    }
    grounded
}

/// Ask the flash model which of the listed GEO datasets best fits the task
/// (species, tissue, measured variables, statistical method). Returns the
/// chosen accession, or None on any failure (caller falls back).
async fn pick_compatible_geo(
    listing: &str,
    task: &miniagent_hypothesis::DataAnalysisTask,
    hypothesis_statement: &str,
    flash: &dyn LlmProvider,
    cancel: &CancellationToken,
) -> Option<String> {
    let prompt = format!(
        r#"You are selecting a public dataset for an automated analysis. Pick the SINGLE most compatible GEO dataset from the candidates.

**Analysis task:**
- objective: {objective}
- cohort: {cohort}
- independent variables: {ind}
- dependent variables: {dep}
- statistical method: {method}
- hypothesis context: {hyp}

**Candidate GEO datasets:**
{listing}

Compatibility criteria (in order): matching measured variables, matching species/tissue, sufficient sample size for the statistical method, matching experiment type (expression vs methylation vs sequencing).

Output ONLY valid JSON (no markdown fences):
{{"accession": "GSE...", "reason": "one sentence"}}"#,
        objective = task.objective,
        cohort = task.cohort_definition,
        ind = task.variables.independent.join(", "),
        dep = task.variables.dependent.join(", "),
        method = task.statistical_method,
        hyp = hypothesis_statement,
    );
    let req = miniagent_provider::traits::CompletionRequest {
        system: "You are a precise bioinformatics dataset curator. Output ONLY valid JSON.".into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.1),
            max_tokens: Some(500),
            ..Default::default()
        },
    };
    let resp = flash.complete(&req, cancel.clone()).await.ok()?;
    let text = resp
        .content
        .iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let repaired = miniagent_core::json_util::extract_and_repair(&text);
    let parsed: serde_json::Value = serde_json::from_str(&repaired).ok()?;
    let acc = parsed.get("accession")?.as_str()?.trim().to_uppercase();
    // Only accept an accession that actually appears in the listing — the
    // model must choose among real candidates, not invent one.
    if acc.starts_with("GSE") && listing.contains(&acc) {
        Some(acc)
    } else {
        None
    }
}

/// Deterministic UUID for a PMID, so every KG entity/relation stays
/// traceable to its source paper: `uuid::Uuid::from_u128(pmid)` round-trips
/// exactly through `Uuid::as_u128` (see [`pmid_from_uuid`]). The previous
/// `Uuid::new_v4()` per extraction silently severed the KG→literature
/// provenance chain (goal 1).
fn pmid_to_uuid(pmid: &str) -> uuid::Uuid {
    uuid::Uuid::from_u128(pmid.parse::<u128>().unwrap_or(0))
}

/// Inverse of [`pmid_to_uuid`]; returns `None` for the zero sentinel (unknown
/// PMID, e.g. externally enriched edges).
fn pmid_from_uuid(id: uuid::Uuid) -> Option<String> {
    let n = id.as_u128();
    (n > 0).then(|| n.to_string())
}

/// Split a PubMed efetch `retmode=xml` body into `(pmid, text)` pairs.
///
/// The plain-text efetch format places each record's "Conflict of interest
/// statement" block AFTER the terminating `PMID: <id>` line, so line-based
/// splitting mis-assigns whole abstracts to the wrong PMID (observed live:
/// 9/12 corpus entries were pure COI text and every abstract was shifted by
/// one paper). The XML format is unambiguous — one `<PubmedArticle>` per
/// record — so we parse it directly.
///
/// The returned text is a compact structured record:
/// `Title: ...\nYear: ...\nAbstract: ...` (abstract sections joined with
/// their `Label` prefixes). Records without a PMID or abstract are dropped.
fn parse_efetch_xml(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for chunk in body.split("<PubmedArticle>").skip(1) {
        let record = match chunk.split("</PubmedArticle>").next() {
            Some(r) => r,
            None => continue,
        };
        let pmid = match first_tag_contents(record, "PMID") {
            Some(p) if p.chars().all(|c| c.is_ascii_digit()) => p,
            _ => continue,
        };
        let title = first_tag_contents(record, "ArticleTitle").unwrap_or_default();
        // Abstract sections, each prefixed with its Label ("BACKGROUND: ...").
        let mut abstract_parts: Vec<String> = Vec::new();
        for seg in record.split("<AbstractText") {
            let Some(inner) = seg.split_once('>') else { continue };
            let (attrs, rest) = inner;
            let text = match rest.split_once("</AbstractText>") {
                Some((t, _)) => t,
                None => continue,
            };
            if text.trim().is_empty() {
                continue;
            }
            let label = extract_attr(attrs, "Label").unwrap_or_default();
            let text = strip_xml_tags(text);
            abstract_parts.push(if label.is_empty() {
                text
            } else {
                format!("{label}: {text}")
            });
        }
        if abstract_parts.is_empty() {
            continue; // no usable abstract (bookshelf/index-only record)
        }
        let year = ["PubDate", "ArticleDate"]
            .iter()
            .find_map(|tag| {
                let seg = first_tag_contents(record, tag)?;
                first_tag_contents(&seg, "Year")
            })
            .unwrap_or_default();
        let text = {
            let mut t = format!("Title: {}", strip_xml_tags(&title));
            if !year.is_empty() {
                t.push_str(&format!("\nYear: {year}"));
            }
            t.push_str(&format!("\nAbstract: {}", abstract_parts.join(" ")));
            t
        };
        out.push((pmid, text));
    }
    out
}

/// Contents of the first `<tag ...>...</tag>` occurrence in `s` (attribute
/// list tolerated between the tag name and `>`).
fn first_tag_contents(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = s.find(&open)?;
    let after_open = &s[start..];
    let inner_start = after_open.find('>')? + 1;
    let rest = &after_open[inner_start..];
    let end = rest.find(&format!("</{tag}>"))?;
    Some(rest[..end].trim().to_string())
}

/// Value of `attr="..."` inside an XML attribute list.
fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let start = attrs.find(&pat)? + pat.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Remove inner markup tags (`<i>`, `<sup>`, …) and unescape the XML
/// entities PubMed actually emits.
fn strip_xml_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// Build a compact English GEO query from a task objective (primary signal)
/// plus the hypothesis statement (disease/gene context).
fn geo_query_from_parts(objective: &str, hypothesis: &str) -> String {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "of", "in", "on", "for", "to", "and", "or", "with", "by",
        "from", "as", "is", "are", "be", "this", "that", "using", "used", "use",
        "between", "across", "whether", "test", "test;", "analysis", "analyze",
    ];
    let mut words: Vec<String> = Vec::new();
    for src in [objective, hypothesis] {
        for w in src.split(|c: char| !c.is_ascii_alphanumeric()) {
            let w = w.trim().to_lowercase();
            if w.len() < 3 || STOPWORDS.contains(&w.as_str()) || words.iter().any(|e| *e == w) {
                continue;
            }
            words.push(w);
            if words.len() >= 10 {
                break;
            }
        }
        if words.len() >= 10 {
            break;
        }
    }
    if words.is_empty() {
        "Homo sapiens expression profiling".to_string()
    } else {
        words.join(" ")
    }
}

/// Pull the first `GSE…` accession out of a `geo_search` result listing
/// (lines are formatted as `N. **GSE12345** — Title`).
fn first_geo_accession(content: &str) -> Option<String> {
    let idx = content.find("**GSE")? + "**".len();
    let acc: String = content[idx..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if acc.starts_with("GSE") && acc.len() > "GSE".len() {
        Some(acc)
    } else {
        None
    }
}

/// Score each paper's relevance to the research query (0-10) with the cheap
/// flash model and keep papers scoring >= 5. Returns (kept, rejected-with-reason).
/// Fails open (keeps the paper) when the LLM call errors.
/// Returns (kept, genuinely_rejected, unjudged). Unjudged papers (every
/// provider failed for that call) are separated so the caller can decide —
/// with the corpus-coherence gate upstream, retaining them as "trimmed
/// best-effort" is safer than aborting a coherent corpus.
async fn filter_irrelevant_papers(
    flash: std::sync::Arc<dyn LlmProvider>,
    fallbacks: Vec<std::sync::Arc<dyn LlmProvider>>,
    query: &str,
    papers: &[(String, String)],
    cancel: CancellationToken,
) -> (Vec<(String, String)>, Vec<(String, String)>, Vec<(String, String)>) {
    let concurrency = 6usize;
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut jobs = Vec::with_capacity(papers.len());
    for (pmid, text) in papers {
        let flash = flash.clone();
        let fallbacks = fallbacks.clone();
        let sem = sem.clone();
        let cancel = cancel.child_token();
        let pmid = pmid.clone();
        let text = text.clone();
        let query = query.to_string();
        jobs.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            // Relevance is decidable from the head of the abstract.
            let snippet: String = text.chars().take(1200).collect();
            let prompt = format!(
                "Research query: {query}\n\nPaper abstract (PMID {pmid}):\n{snippet}\n\n\
                 Is this paper on-topic for the research query? Output ONLY JSON: \
                 {{\"score\": <integer 0-10>, \"reason\": \"<one short sentence>\"}}"
            );
            let request = miniagent_provider::traits::CompletionRequest {
                system: "You judge literature relevance. Output ONLY valid JSON.".into(),
                messages: vec![miniagent_core::message::Message::user(&prompt)],
                tools: vec![],
                config: miniagent_core::config::InferenceConfig {
                    temperature: Some(0.0),
                    max_tokens: Some(2_048),
                    ..Default::default()
                },
            };
            // Walk primary + cross-family fallbacks until one returns a
            // parseable verdict; reasoning models need headroom (a tiny
            // max_tokens cap yields empty text — observed live).
            let mut providers: Vec<std::sync::Arc<dyn LlmProvider>> = vec![flash.clone()];
            providers.extend(fallbacks.iter().cloned());
            let mut score = -1i64; // unjudged unless some provider answers
            let mut reason = String::new();
            for provider in &providers {
                if let Ok(resp) = provider.complete(&request, cancel.clone()).await {
                    let t: String = resp.content.iter()
                        .filter_map(|b| match b {
                            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let cleaned = t.trim()
                        .trim_start_matches("```json").trim_start_matches("```")
                        .trim_end_matches("```")
                        .trim();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(cleaned) {
                        score = v.get("score").and_then(|s| s.as_i64()).unwrap_or(-1);
                        reason = v.get("reason").and_then(|s| s.as_str()).unwrap_or("").to_string();
                        if score >= 0 {
                            break;
                        }
                    }
                }
            }
            if score == -1 {
                reason = "unjudged (LLM unavailable or invalid output)".into();
            }
            (pmid, text, score, reason)
        }));
    }
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    let mut unjudged = Vec::new();
    for job in jobs {
        if let Ok((pmid, text, score, reason)) = job.await {
            if score >= 5 {
                kept.push((pmid, text));
            } else if score == -1 {
                // Unjudgeable: caller decides (retained on corpus-level
                // coherence verdict, audited separately).
                unjudged.push((pmid, text));
                let _ = reason;
            } else {
                rejected.push((pmid, reason));
            }
        }
    }
    (kept, rejected, unjudged)
}

/// Find the KG entity that best represents the queried disease: among
/// Disease-type entities, the one whose name/alias tokens overlap the query
/// tokens the most. Used to anchor link-prediction candidates.
fn find_disease_anchor(
    kg: &miniagent_kg::KnowledgeGraph,
    query: &str,
) -> Option<miniagent_kg::schema::EntityId> {
    let q_tokens: std::collections::HashSet<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .map(str::to_string)
        .collect();
    if q_tokens.is_empty() {
        return None;
    }
    let mut best: Option<(miniagent_kg::schema::EntityId, usize)> = None;
    for e in kg.all_entities() {
        if e.entity_type != miniagent_kg::schema::EntityType::Disease {
            continue;
        }
        let mut tokens: Vec<String> = e
            .name
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 3)
            .map(str::to_string)
            .collect();
        for alias in &e.aliases {
            tokens.extend(
                alias
                    .to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|t| t.len() > 3)
                    .map(str::to_string),
            );
        }
        let overlap = tokens.iter().filter(|t| q_tokens.contains(*t)).count();
        if overlap > 0 && best.as_ref().is_none_or(|(_, b)| overlap > *b) {
            best = Some((e.id, overlap));
        }
    }
    best.map(|(id, _)| id)
}

/// Retrieve web-search evidence for the hypotheses ahead of the debate, then
/// FETCH the top result URLs so every cited source is a verified page rather
/// than an untrusted snippet. Returns (hypothesis_id, query, evidence block)
/// triples where the block contains both the result listing and fetched
/// full-text excerpts. Failures degrade per-hypothesis (debate proceeds with
/// whatever evidence was retrievable).
async fn retrieve_debate_evidence(
    hypotheses: &[miniagent_hypothesis::Hypothesis],
    max_hypotheses: usize,
    providers: Vec<std::sync::Arc<dyn LlmProvider>>,
    cancel: CancellationToken,
) -> Vec<(uuid::Uuid, String, String)> {
    use futures_util::stream::{self, StreamExt};
    use miniagent_tool::tools::{WebFetchTool, WebSearchTool};
    use miniagent_tool::traits::{Tool, ToolContext};

    // Shared via Arc and cloned into each job: the job futures must own their
    // captures (no borrows of `hypotheses`/locals across await) or the whole
    // pipeline future stops being `Send` under `tokio::spawn`.
    let search = std::sync::Arc::new(WebSearchTool::new());
    let fetch = std::sync::Arc::new(WebFetchTool::new());
    let providers_snapshot = std::sync::Arc::new(providers);
    let ctx = std::sync::Arc::new(ToolContext::new(
        std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        "debate_evidence".to_string(),
    ));

    // One (search + fetch top-2 URLs) job per hypothesis, 3 at a time.
    let job_inputs: Vec<(uuid::Uuid, String)> = hypotheses
        .iter()
        .take(max_hypotheses)
        .map(|h| (h.id, h.statement.clone()))
        .collect();
    let jobs = stream::iter(job_inputs)
        .map(|(hyp_id, statement)| {
            let cancel = cancel.clone();
            let search = search.clone();
            let fetch = fetch.clone();
            let ctx = ctx.clone();
            let providers_snapshot = providers_snapshot.clone();
            async move {
                let query = geo_query_from_parts(&statement, "");
                let listing = match search
                    .execute(
                        serde_json::json!({"query": query, "num": 5}),
                        &ctx,
                        cancel.child_token(),
                    )
                    .await
                {
                    Ok(res) => res.content.chars().take(4000).collect::<String>(),
                    Err(_) => return None,
                };
                if listing.trim().is_empty() {
                    return None;
                }

                // Relevance filter on the SEARCH RESULTS themselves (generic
                // LLM judgment, no keyword lists): only results genuinely
                // about the hypothesis topic survive to the fetch step, so
                // off-topic hits cannot inject noise into the debate.
                let urls = match filter_search_results(&statement, &query, &listing, &providers_snapshot).await {
                    Some(urls) => urls,
                    None => extract_urls(&listing).into_iter().take(2).collect::<Vec<_>>(),
                };
                let mut fetched = String::new();
                for url in &urls {
                    if let Ok(res) = fetch
                        .execute(
                            serde_json::json!({"url": url, "max_length": 6000}),
                            &ctx,
                            cancel.child_token(),
                        )
                        .await
                        && !res.content.trim().is_empty()
                    {
                        fetched.push_str(&format!(
                            "\n**[verified source] {url}**\n{}\n",
                            res.content.chars().take(6000).collect::<String>()
                        ));
                    }
                }

                let mut block = format!("Search query: {query}\n{listing}");
                if !fetched.is_empty() {
                    block.push_str("\n\n**Fetched full text of top sources (verified):**\n");
                    block.push_str(&fetched);
                }
                Some((hyp_id, query, block))
            }
        })
        .buffered(3)
        .collect::<Vec<Option<(uuid::Uuid, String, String)>>>()
        .await;
    jobs.into_iter().flatten().collect()
}

/// Generic relevance filter over a web-search listing: the model picks the
/// result indices genuinely about the topic (keywords/topic words + the
/// hypothesis statement); the caller fetches only those URLs. No keyword
/// dictionaries or numeric thresholds — relevance is a model judgment over
/// the actual listing. Returns None when no provider can judge (caller falls
/// back to the legacy top-2 behavior).
async fn filter_search_results(
    statement: &str,
    query: &str,
    listing: &str,
    providers: &[std::sync::Arc<dyn LlmProvider>],
) -> Option<Vec<String>> {
    use miniagent_provider::traits::CompletionRequest;
    let urls = extract_urls(listing);
    if urls.is_empty() || providers.is_empty() {
        return None;
    }
    let prompt = format!(
        "Topic (hypothesis under debate): {statement}\nSearch query: {query}\n\n\
         Search results with URLs:\n{listing}\n\n\
         Which of these results genuinely address the topic? Exclude results that merely \
         share a word but are about something else. Output ONLY JSON: \
         {{\"relevant_urls\": [\"<url from the list above>\"]}}"
    );
    for provider in providers {
        let request = CompletionRequest {
            system: "You filter search results for topical relevance. Output ONLY valid JSON.".into(),
            messages: vec![miniagent_core::message::Message::user(&prompt)],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                temperature: Some(0.0),
                max_tokens: Some(4_096),
                ..Default::default()
            },
        };
        if let Ok(resp) = provider.complete(&request, CancellationToken::new()).await {
            let text: String = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let repaired = miniagent_core::json_util::extract_and_repair(&text);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repaired) {
                let picked: Vec<String> = v["relevant_urls"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|u| u.as_str())
                            .filter(|u| urls.iter().any(|raw| raw == u))
                            .map(|u| u.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                // Only accept URLs that actually appeared in the listing.
                if !picked.is_empty() {
                    return Some(picked);
                }
                return Some(Vec::new()); // judged: none relevant — fetch nothing
            }
        }
    }
    None
}

/// Extract http(s) URLs from a markdown/plain-text search listing.
fn extract_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("http") {
        let candidate = &rest[pos..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || c == ')' || c == ']')
            .unwrap_or(candidate.len());
        let url = &candidate[..end];
        if url.starts_with("http://") || url.starts_with("https://") {
            if !out.iter().any(|u| u == url) {
                out.push(url.to_string());
            }
        }
        rest = &rest[pos + end.max(1)..];
    }
    out
}

/// Shorten a hex/uuid-ish string for compact CLI display (keeps the head).
fn short_id(s: &str) -> String {
    let len = s.len();
    if len <= 8 {
        s.to_string()
    } else {
        s.chars().take(8).collect()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}



fn has_non_english(s: &str) -> bool {
    s.chars().any(|c| c as u32 > 0x007F)
}

// ── Plan command ──────────────────────────────────────────────

#[cfg(test)]
mod research_tests {
    use super::*;

    #[test]
    fn efetch_xml_pairs_abstract_to_correct_pmid() {
        // Reproduces the live failure: record 1 has a COI block that follows
        // its abstract; the text-format parser shifted it onto record 2.
        let body = r#"<PubmedArticleSet>
<PubmedArticle><MedlineCitation><PMID Version="1">36449413</PMID>
<Article><Journal><Title>Nature</Title></Journal>
<JournalIssue><PubDate><Year>2023</Year></PubDate></JournalIssue>
<ArticleTitle>Lecanemab slows <i>decline</i> in early Alzheimer's</ArticleTitle>
<Abstract><AbstractText>Amyloid beta accumulates.</AbstractText>
<AbstractText Label="METHODS" NlmCategory="METHODS">We ran a trial &amp; analysis.</AbstractText></Abstract>
</Article></MedlineCitation></PubmedArticle>
<PubmedArticle><MedlineCitation><PMID Version="1">37459141</PMID>
<Article><ArticleTitle>Donanemab also works</ArticleTitle>
<Abstract><AbstractText>A second antibody trial.</AbstractText></Abstract>
</Article><CoiStatement>The authors declare no conflict.</CoiStatement>
</MedlineCitation></PubmedArticle>
</PubmedArticleSet>"#;
        let recs = parse_efetch_xml(body);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].0, "36449413");
        // Own abstract present, COI of the OTHER record absent.
        assert!(recs[0].1.contains("Lecanemab"));
        assert!(recs[0].1.contains("METHODS: We ran a trial & analysis."));
        assert!(recs[0].1.contains("Year: 2023"));
        assert!(!recs[0].1.contains("no conflict"));
        assert_eq!(recs[1].0, "37459141");
        assert!(recs[1].1.contains("Donanemab"));
    }

    #[test]
    fn efetch_xml_drops_records_without_abstract_or_pmid() {
        let body = "<PubmedArticle><MedlineCitation><PMID>1</PMID></MedlineCitation></PubmedArticle>\
                    <PubmedArticle><MedlineCitation><ArticleTitle>orphan no pmid</ArticleTitle></MedlineCitation></PubmedArticle>";
        assert!(parse_efetch_xml(body).is_empty());
    }

    #[test]
    fn urls_extracted_and_deduped_from_listing() {
        let listing = "1. **T**\n   https://a.example.com/x\n   snippet\n2. **U**\n   http://b.example.com/y)\n   again https://a.example.com/x";
        let urls = extract_urls(listing);
        assert_eq!(urls, vec!["https://a.example.com/x", "http://b.example.com/y"]);
    }

    #[test]
    fn geo_accession_parsed_from_listing() {
        let listing = "## GEO DataSet Search: 'q'\nTotal: 5 | Showing: 2\n\n1. **GSE12345** — Alzheimer expression\n   https://www.ncbi.nlm.nih.gov/geo/query/acc.cgi?acc=GSE12345\n\n2. **GSE999** — other\n";
        assert_eq!(first_geo_accession(listing).as_deref(), Some("GSE12345"));
        assert_eq!(first_geo_accession("no accessions here"), None);
        assert_eq!(first_geo_accession("**GSE"), None); // too short / malformed
    }

    #[test]
    fn geo_query_strips_stopwords_and_dedupes() {
        let q = geo_query_from_parts(
            "Test whether APOE expression differs between cohorts",
            "APOE drives Alzheimer pathology",
        );
        assert!(q.contains("apoe"));
        assert!(q.contains("alzheimer"));
        assert!(!q.contains(" whether "));
        assert!(!q.contains(" between "));
        // no duplicate words
        let words: Vec<&str> = q.split(' ').collect();
        let uniq: std::collections::HashSet<&str> = words.iter().copied().collect();
        assert_eq!(words.len(), uniq.len());
    }

    #[test]
    fn kg_roundtrip_preserves_ids_and_edges() {
        use miniagent_kg::schema::{Entity, EntityId, EntityType, Relation, RelationId, RelationType};

        let mut kg = miniagent_kg::KnowledgeGraph::new();
        let head = Entity { id: EntityId::new(), name: "APOE".into(), entity_type: EntityType::Gene, aliases: vec!["ApoE".into()], metadata: serde_json::json!({}) };
        let tail = Entity { id: EntityId::new(), name: "Alzheimer disease".into(), entity_type: EntityType::Disease, aliases: vec![], metadata: serde_json::json!({}) };
        let head_id = head.id;
        let tail_id = tail.id;
        kg.add_entity(head);
        kg.add_entity(tail);
        kg.add_relation(Relation {
            id: RelationId::new(),
            from_id: head_id,
            to_id: tail_id,
            relation_type: RelationType::AssociatedWith,
            confidence: 0.9,
            evidence: "test".into(),
            source_paper_id: None,
            support_count: 1,
            supporting_papers: vec![],
        });

        let path = std::env::temp_dir().join(format!("mn_kg_test_{}.json", uuid::Uuid::new_v4()));
        save_kg(&kg, &path).expect("save kg");
        let loaded = load_kg(&path).expect("load kg");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.entity_count(), 2);
        assert_eq!(loaded.relation_count(), 1);
        // ids preserved → cached link-prediction candidates stay valid
        assert_eq!(loaded.get_entity(&head_id).map(|e| e.name.as_str()), Some("APOE"));
        assert!(loaded.contains_edge(&head_id, &RelationType::AssociatedWith, &tail_id));
    }
}

