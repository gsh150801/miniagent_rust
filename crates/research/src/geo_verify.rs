//! GEO 数据集主题校验 / GEO dataset topic verification.
//!
//! Live incident: the validation-plan LLM hallucinated plausible-sounding
//! accessions (GSE141758 = *Plasmodium falciparum* RNA-seq, GSE138852 =
//! Alzheimer's cortex, GSE116775 = beef cattle multi-tissue) that passed the
//! existence-only grounding check and were downloaded and analysed for a
//! Parkinson gut–brain-axis question. This module adds three gates:
//!
//! 1. **Accession topic check** (pre-grounding): eutils esummary gives the
//!    official title/taxon/summary; hard lexical rules reject non-human/mouse
//!    organisms outright, then an LLM judges disease/tissue/cell-type fit.
//! 2. **Post-download matrix check**: the cleaned series matrix carries
//!    `ATTR_Sample_organism_ch` / `_source_name_ch` / `_characteristics_ch`;
//!    the organism in the file must match the esummary taxon (and be
//!    human/mouse).
//! 3. **Sample-type check**: an LLM compares the file's sample
//!    source/characteristics values against the task's cohort definition.
//!
//! Any gate failure forces the task into an honest dry-run instead of
//! executing a meaningless analysis.

use miniagent_hypothesis::DataAnalysisTask;
use miniagent_provider::traits::LlmProvider;
use tokio_util::sync::CancellationToken;

/// Official (GEO-registered) metadata for one series.
#[derive(Debug, Clone)]
pub struct GeoSeriesSummary {
    pub accession: String,
    pub title: String,
    pub taxon: String,
    pub summary: String,
    pub n_samples: u32,
}

/// One verification gate's outcome.
#[derive(Debug, Clone)]
pub struct CheckVerdict {
    pub compatible: bool,
    /// Human-readable reason, cited in events / dry-run reasons.
    pub reason: String,
}

const SUPPORTED_ORGANISMS: &[&str] = &["homo sapiens", "mus musculus"];

/// Fetch the official series metadata from NCBI eutils (esearch → esummary,
/// XML). `Err` on network/API failure; `Ok` even when NCBI's record is thin.
pub async fn fetch_geo_summary(
    accession: &str,
    client: &reqwest::Client,
    api_key: &str,
    cancel: CancellationToken,
) -> Result<GeoSeriesSummary, String> {
    let acc = accession.trim().to_uppercase();
    let key = if api_key.is_empty() {
        String::new()
    } else {
        format!("&api_key={api_key}")
    };
    // esearch: gds db, restricted to series entries (a GSM can share [ACCN]
    // via its series relation; ETYP=gse picks the series itself).
    let search_url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi?db=gds&term={acc}%5BACCN%5D+AND+gse%5BETYP%5D&retmax=1{key}"
    );
    let xml = tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled".into()),
        r = client.get(&search_url).send() => {
            r.map_err(|e| format!("esearch {acc}: {e}"))?
                .text()
                .await
                .map_err(|e| format!("esearch {acc} body: {e}"))?
        }
    };
    let id = xml
        .split("<Id>")
        .nth(1)
        .and_then(|rest| rest.split("</Id>").next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("esearch {acc}: no GDS id"))?;

    let sum_url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi?db=gds&id={id}{key}"
    );
    let xml = tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled".into()),
        r = client.get(&sum_url).send() => {
            r.map_err(|e| format!("esummary {acc}: {e}"))?
                .text()
                .await
                .map_err(|e| format!("esummary {acc} body: {e}"))?
        }
    };
    parse_gds_esummary(&xml, &acc).ok_or_else(|| format!("esummary {acc}: unparsable"))
}

/// Parse the single DocSum of a gds esummary response.
fn parse_gds_esummary(xml: &str, expect_acc: &str) -> Option<GeoSeriesSummary> {
    let doc = xml.split("<DocSum>").nth(1)?;
    let doc = &doc[..doc.find("</DocSum>").unwrap_or(doc.len())];
    let get = |name: &str| -> String {
        doc.split(&format!("Name=\"{name}\""))
            .nth(1)
            .and_then(|rest| rest.split("</Item>").next())
            .and_then(|item| item.split('>').nth(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let summary = get("summary");
    Some(GeoSeriesSummary {
        accession: expect_acc.to_string(),
        title: get("title"),
        taxon: get("taxon").to_lowercase(),
        summary,
        n_samples: get("n_samples").parse().unwrap_or(0),
    })
}

/// Hard, LLM-free compatibility rules. Catches the worst hallucinations
/// (malaria parasite / cattle data for a human-disease question) without
/// spending a model call.
pub fn lexical_check(
    task: &DataAnalysisTask,
    summary: &GeoSeriesSummary,
) -> CheckVerdict {
    let species_expected = format!(
        "{} {}",
        task.cohort_definition.to_lowercase(),
        task.objective.to_lowercase()
    );
    let wants_human = ["patient", "human", "个体", "患者", "人源", "ipsc", "临床"]
        .iter()
        .any(|k| species_expected.contains(k));
    let wants_mouse = ["mouse", "mice", "murine", "小鼠", "rodent", "动物模型"]
        .iter()
        .any(|k| species_expected.contains(k));

    if !SUPPORTED_ORGANISMS.contains(&summary.taxon.as_str()) {
        return CheckVerdict {
            compatible: false,
            reason: format!(
                "物种不匹配：GEO 官方记录的物种是 `{}`（仅支持 Homo sapiens / Mus musculus）",
                summary.taxon
            ),
        };
    }
    if wants_human && summary.taxon != "homo sapiens" {
        return CheckVerdict {
            compatible: false,
            reason: format!(
                "任务需要人类数据（cohort: {}），但数据集物种是 {}",
                truncate(&task.cohort_definition, 60),
                summary.taxon
            ),
        };
    }
    if wants_mouse && summary.taxon != "mus musculus" {
        return CheckVerdict {
            compatible: false,
            reason: format!(
                "任务需要小鼠数据，但数据集物种是 {}",
                summary.taxon
            ),
        };
    }
    CheckVerdict {
        compatible: true,
        reason: format!("物种匹配（{}）", summary.taxon),
    }
}

/// LLM 判断数据集主题/样本类型/细胞类型是否与任务匹配。返回 `None` 表示
/// LLM 不可用（调用方决定是否保守放行——物种硬规则已在 lexical_check 挡住
/// 最离谱的错配）。
pub async fn llm_topic_check(
    task: &DataAnalysisTask,
    hypothesis_statement: &str,
    summary: &GeoSeriesSummary,
    provider: &dyn LlmProvider,
    cancel: CancellationToken,
) -> Option<CheckVerdict> {
    let prompt = format!(
        r#"You are verifying that a public dataset matches an analysis task BEFORE running expensive computations.

**Analysis task:**
- objective: {objective}
- cohort definition: {cohort}
- statistical method: {method}
- hypothesis context: {hyp}

**Dataset (official GEO record):**
- accession: {acc}
- title: {title}
- organism: {taxon}
- samples: {n}
- summary: {summary}

Judge:
1. `topic_ok` — does the dataset study the same disease/biology the task needs?
2. `sample_type_ok` — are the samples the right specimen type (e.g. gut biopsy, blood, stool, brain region, iPSC-derived cells — whatever the task requires)?
3. `cell_type_ok` — does the measured cell/tissue composition fit the task (single-cell vs bulk is fine either way; what matters is that the relevant cell types/tissues are present)?

Be strict: a "calibration" or "loosely related" dataset is NOT compatible — the live failure this prevents is running a Parkinson's gut-microbiome analysis on a malaria-parasite dataset. Answer honestly when unsure.

Output ONLY valid JSON:
{{"compatible": true|false, "topic_ok": true|false, "sample_type_ok": true|false, "cell_type_ok": true|false, "reason": "one sentence citing what the dataset actually is"}}"#,
        objective = task.objective,
        cohort = task.cohort_definition,
        method = task.statistical_method,
        hyp = truncate(hypothesis_statement, 300),
        acc = summary.accession,
        title = truncate(&summary.title, 200),
        taxon = summary.taxon,
        n = summary.n_samples,
        summary = truncate(&summary.summary, 600),
    );
    let request = miniagent_provider::traits::CompletionRequest {
        system: "You are a strict dataset curator. Output ONLY valid JSON.".into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.0),
            max_tokens: Some(1_000),
            ..Default::default()
        },
    };
    let resp = provider.complete(&request, cancel).await.ok()?;
    let text: String = resp
        .content
        .iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let repaired = miniagent_core::json_util::extract_and_repair(&text);
    let v: serde_json::Value = serde_json::from_str(&repaired).ok()?;
    let failed = |name: &str| -> Option<String> {
        v.get(name)
            .and_then(|x| x.as_bool())
            .map(|ok| if ok { None } else { Some(name.to_string()) })
            .unwrap_or(Some(name.to_string()))
    };
    let fails: Vec<String> = ["topic_ok", "sample_type_ok", "cell_type_ok"]
        .iter()
        .filter_map(|n| failed(n))
        .collect();
    let compatible = v.get("compatible").and_then(|x| x.as_bool()).unwrap_or(false) && fails.is_empty();
    let mut reason = v
        .get("reason")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if !fails.is_empty() {
        reason = format!("不匹配项: {} — {}", fails.join(", "), reason);
    }
    Some(CheckVerdict { compatible, reason })
}

/// Metadata extracted from a cleaned series matrix (post-download gate).
#[derive(Debug, Clone, Default)]
pub struct SeriesMatrixMeta {
    pub organisms: Vec<String>,
    pub sources: Vec<String>,
    pub characteristics: Vec<String>,
    pub title: String,
}

/// Stream the cleaned TSV's `ATTR_Sample_*` rows and pull the fields the
/// sample-type check needs. Memory-safe: line-streamed, stops at the
/// expression-table header.
pub fn parse_series_matrix_meta(path: &std::path::Path) -> Option<SeriesMatrixMeta> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let mut meta = SeriesMatrixMeta::default();
    let mut saw_attr = false;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.starts_with("ID_REF") {
            break; // expression table begins
        }
        if let Some(rest) = line.strip_prefix("ATTR_Sample_") {
            saw_attr = true;
            let (key, values) = split_row(&format!("ATTR_Sample_{rest}"));
            // key 带 `ATTR_Sample_` 前缀，且 GEO 键带 channel 后缀
            // （organism_ch1 / source_name_ch1 / characteristics_ch1）——
            // 用中缀判断而不是精确匹配。
            if key.contains("organism") {
                meta.organisms = values;
            } else if key.contains("source_name") {
                meta.sources = values;
            } else if key.contains("characteristics") {
                meta.characteristics = values;
            } else if key.ends_with("title") {
                meta.title = values.first().cloned().unwrap_or_default();
            }
        }
    }
    saw_attr.then_some(meta)
}

/// Post-download gate: the file's own organism rows must name a supported
/// organism and agree with the esummary record when that is known.
pub fn matrix_organism_check(
    meta: &SeriesMatrixMeta,
    summary: Option<&GeoSeriesSummary>,
) -> CheckVerdict {
    if meta.organisms.is_empty() {
        return CheckVerdict {
            compatible: false,
            reason: "series matrix 缺少 ATTR_Sample_organism 行，无法确认物种".into(),
        };
    }
    let mut uniq: Vec<String> = meta
        .organisms
        .iter()
        .map(|o| o.trim().to_lowercase())
        .filter(|o| !o.is_empty())
        .collect();
    uniq.sort();
    uniq.dedup();
    for org in &uniq {
        if !SUPPORTED_ORGANISMS.contains(&org.as_str()) {
            return CheckVerdict {
                compatible: false,
                reason: format!("下载文件内物种是 `{org}`（仅支持 Homo sapiens / Mus musculus）"),
            };
        }
    }
    if let Some(s) = summary
        && !s.taxon.is_empty()
    {
        let mismatch = uniq.iter().any(|o| !o.is_empty() && *o != s.taxon);
        if mismatch {
            return CheckVerdict {
                compatible: false,
                reason: format!(
                    "文件内物种 {:?} 与 GEO 官方记录 `{}` 不一致（可能下载错文件）",
                    uniq, s.taxon
                ),
            };
        }
    }
    CheckVerdict {
        compatible: true,
        reason: format!("文件内物种 {:?} 合规", uniq),
    }
}

/// Post-download gate 2: the file's sample source/characteristics values vs
/// the task's cohort definition (LLM judge; `None` on LLM failure — caller
/// keeps the deterministic gates' verdict only).
pub async fn llm_sample_type_check(
    task: &DataAnalysisTask,
    meta: &SeriesMatrixMeta,
    provider: &dyn LlmProvider,
    cancel: CancellationToken,
) -> Option<CheckVerdict> {
    let preview = |v: &[String], n: usize| -> String {
        v.iter().take(n).map(|s| truncate(s, 60)).collect::<Vec<_>>().join(" | ")
    };
    let prompt = format!(
        r#"You are checking that a DOWNLOADED dataset's samples are what an analysis task needs, before the analysis runs.

**Task cohort definition:** {cohort}
**Task objective:** {objective}

**Downloaded file's per-sample metadata (first values):**
- source_name: {sources}
- characteristics: {chars}

Do these samples plausibly match the cohort the task needs (same organism confirmed already; judge TISSUE/SPECIMEN and GROUPS here)? A mismatch means the analysis would be meaningless. Be strict but practical: differences in subtype naming are fine; wrong tissue/organism-level specimen or entirely unrelated groups are not.

Output ONLY valid JSON:
{{"compatible": true|false, "reason": "one sentence"}}"#,
        cohort = truncate(&task.cohort_definition, 250),
        objective = truncate(&task.objective, 200),
        sources = preview(&meta.sources, 5),
        chars = preview(&meta.characteristics, 8),
    );
    let request = miniagent_provider::traits::CompletionRequest {
        system: "You are a strict sample curator. Output ONLY valid JSON.".into(),
        messages: vec![miniagent_core::message::Message::user(&prompt)],
        tools: vec![],
        config: miniagent_core::config::InferenceConfig {
            temperature: Some(0.0),
            max_tokens: Some(600),
            ..Default::default()
        },
    };
    let resp = provider.complete(&request, cancel).await.ok()?;
    let text: String = resp
        .content
        .iter()
        .filter_map(|b| match b {
            miniagent_core::event::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let repaired = miniagent_core::json_util::extract_and_repair(&text);
    let v: serde_json::Value = serde_json::from_str(&repaired).ok()?;
    let compatible = v.get("compatible").and_then(|x| x.as_bool())?;
    Some(CheckVerdict {
        compatible,
        reason: v
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Split a TSV row into its row key and cell values (same convention as
/// `analysis::geo`).
fn split_row(line: &str) -> (String, Vec<String>) {
    let mut parts = line.split('\t');
    let key = parts.next().unwrap_or_default().trim_matches('"').to_string();
    let values: Vec<String> = parts.map(|v| v.trim_matches('"').to_string()).collect();
    (key, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?>
<!DOCTYPE eSummaryResult>
<eSummaryResult><DocSum><Id>200141758</Id>
<Item Name="Accession" Type="String">GSE141758</Item>
<Item Name="title" Type="String">HMGB1 is required for virulence gene expression in Plasmodium falciparum [RNA-seq]</Item>
<Item Name="taxon" Type="String">Plasmodium falciparum</Item>
<Item Name="n_samples" Type="Integer">18</Item>
<Item Name="summary" Type="String">This SuperSeries is composed of the SubSeries listed below.</Item>
</DocSum></eSummaryResult>"#;

    #[test]
    fn esummary_parse_extracts_fields() {
        let s = parse_gds_esummary(XML, "GSE141758").expect("parsable");
        assert_eq!(s.accession, "GSE141758");
        assert_eq!(s.taxon, "plasmodium falciparum");
        assert_eq!(s.n_samples, 18);
        assert!(s.title.contains("HMGB1"));
    }

    #[test]
    fn lexical_check_rejects_non_supported_organism() {
        let (task, _make) = test_task("PD 患者结肠黏膜活检 vs 健康对照");
        let summary = GeoSeriesSummary {
            accession: "GSE141758".into(),
            title: "malaria".into(),
            taxon: "plasmodium falciparum".into(),
            summary: String::new(),
            n_samples: 18,
        };
        let v = lexical_check(&task, &summary);
        assert!(!v.compatible);
        assert!(v.reason.contains("物种不匹配"));
    }

    #[test]
    fn lexical_check_rejects_cattle_for_human_cohort() {
        let (task, _) = test_task("PD 患者结肠活检");
        let summary = GeoSeriesSummary {
            accession: "GSE116775".into(),
            title: "beef cattle".into(),
            taxon: "bos taurus".into(),
            summary: String::new(),
            n_samples: 189,
        };
        assert!(!lexical_check(&task, &summary).compatible);
    }

    #[test]
    fn lexical_check_passes_human_for_human() {
        let (task, _) = test_task("PD 患者中脑样本");
        let summary = GeoSeriesSummary {
            accession: "GSE157783".into(),
            title: "midbrain atlas".into(),
            taxon: "homo sapiens".into(),
            summary: String::new(),
            n_samples: 11,
        };
        assert!(lexical_check(&task, &summary).compatible);
    }

    #[test]
    fn parse_series_matrix_meta_handles_channel_suffix_keys() {
        let dir = std::env::temp_dir().join("miniagent_geo_verify_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sm.tsv");
        std::fs::write(&path,
            "ATTR_Sample_title\t\"S1\"\t\"S2\"\n\
             ATTR_Sample_organism_ch1\t\"Homo sapiens\"\t\"Homo sapiens\"\n\
             ATTR_Sample_source_name_ch1\t\"colon mucosa\"\t\"colon mucosa\"\n\
             ATTR_Sample_characteristics_ch1\t\"disease: PD\"\t\"disease: control\"\n\
             ID_REF\t\"S1\"\t\"S2\"\n\
             GENE1\t1\t2\n").unwrap();
        let meta = parse_series_matrix_meta(&path).expect("parsable");
        assert_eq!(meta.organisms, vec!["Homo sapiens", "Homo sapiens"]);
        assert!(meta.sources[0].contains("colon"));
        assert!(meta.characteristics[0].starts_with("disease:"));
        let v = matrix_organism_check(&meta, None);
        assert!(v.compatible, "{:?}", v.reason);
    }

    #[test]
    fn matrix_organism_check_flags_mismatch_vs_esummary() {
        let meta = SeriesMatrixMeta {
            organisms: vec!["Homo sapiens".into()],
            ..Default::default()
        };
        let summary = GeoSeriesSummary {
            accession: "GSE999".into(),
            title: String::new(),
            taxon: "mus musculus".into(),
            summary: String::new(),
            n_samples: 3,
        };
        let v = matrix_organism_check(&meta, Some(&summary));
        assert!(!v.compatible);
        // Missing organism rows also fail closed.
        let empty = SeriesMatrixMeta::default();
        assert!(!matrix_organism_check(&empty, None).compatible);
    }

    fn test_task(cohort: &str) -> (DataAnalysisTask, ()) {
        use miniagent_hypothesis::{AnalysisVariables, DatasetSource};
        (
            DataAnalysisTask {
                id: "DA-T".into(),
                objective: "Test differential abundance".into(),
                dataset_source: DatasetSource::Geo,
                dataset_accession: None,
                dataset_note: None,
                cohort_definition: cohort.into(),
                variables: AnalysisVariables {
                    independent: vec!["group".into()],
                    dependent: vec!["y".into()],
                    covariates: vec![],
                },
                statistical_method: "Wilcoxon".into(),
                expected_outcome: "x".into(),
                deliverable: "csv".into(),
                priority: 0.5,
            },
            (),
        )
    }
}
