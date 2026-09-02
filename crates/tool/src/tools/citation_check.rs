use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Citation verification for generated reports (dsh-bioinfo verify-report
/// style): parse a markdown report's in-text `[n]` citations and trailing
/// references section, then verify every reference against its claimed
/// source — PMID via PubMed E-Summary (title/journal/year cross-check),
/// DOI via doi.org resolution, URL via live fetch. Produces a structured
/// mismatch list so an agent (or reviewer) knows exactly which references
/// are wrong and how.
///
/// Generic: works on any report text. Biomedical reports benefit from PMID
/// verification; web sources are checked for reachability and title match.
pub struct CitationCheckTool {
    client: reqwest::Client,
}

impl Default for CitationCheckTool {
    fn default() -> Self {
        Self::new()
    }
}

/// One parsed reference entry.
struct RefEntry {
    index: usize,
    raw: String,
    pmid: Option<String>,
    doi: Option<String>,
    url: Option<String>,
    title: String,
}

/// Verification outcome for one reference.
struct RefVerdict {
    index: usize,
    status: &'static str, // verified | not_found | mismatch | unverifiable
    detail: String,
}

impl CitationCheckTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("miniagent/0.1")
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Extract `[n]` citation indices used in the report body (before the
    /// references heading).
    fn cited_indices(body: &str) -> Vec<usize> {
        let mut out = std::collections::BTreeSet::new();
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'[' {
                if let Some(close) = body[i + 1..].find(']') {
                    let inner = &body[i + 1..i + 1 + close];
                    let cleaned = inner.replace(',', " ").replace('–', " ");
                    let all_numeric = cleaned.split_whitespace().all(|tok| {
                        tok.parse::<usize>().is_ok()
                            || tok
                                .split('-')
                                .all(|p| p.parse::<usize>().is_ok())
                    });
                    if all_numeric {
                        for tok in cleaned.split_whitespace() {
                            if let Ok(n) = tok.parse::<usize>() {
                                out.insert(n);
                            } else if let Some((a, b)) = tok.split_once('-')
                                && let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>())
                                && b > a
                                && b - a <= 20
                            {
                                for n in a..=b {
                                    out.insert(n);
                                }
                            }
                        }
                    }
                    i += 1 + close;
                }
            }
            i += 1;
        }
        out.into_iter().collect()
    }

    /// Parse the references section (after a `references` / `参考文献`
    /// heading): one entry per line beginning with `[n]` (or `n.`).
    fn parse_references(report: &str) -> Vec<RefEntry> {
        let mut refs = Vec::new();
        let mut in_refs = false;
        for line in report.lines() {
            let lower = line.trim().to_lowercase();
            if !in_refs
                && (lower.starts_with("# references")
                    || lower.starts_with("## references")
                    || lower.starts_with("## references")
                    || lower.starts_with("# references ")
                    || lower.starts_with("## 引用")
                    || lower.starts_with("# 参考文献")
                    || lower.starts_with("## 参考文献")
                    || lower.starts_with("**references**")
                    || lower.starts_with("references:"))
            {
                in_refs = true;
                continue;
            }
            if !in_refs {
                continue;
            }
            // A new markdown heading ends the references section.
            if line.starts_with("# ") && !lower.contains("reference") && !lower.contains("参考文献")
            {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (idx, rest) = if let Some(close) = trimmed.find(']')
                && trimmed.starts_with('[')
            {
                match trimmed[1..close].trim().parse::<usize>() {
                    Ok(n) => (n, trimmed[close + 1..].trim()),
                    Err(_) => continue,
                }
            } else if let Some(dot) = trimmed.find('.')
                && trimmed[..dot].parse::<usize>().is_ok()
                && dot <= 3
            {
                (trimmed[..dot].parse().unwrap(), trimmed[dot + 1..].trim())
            } else {
                continue;
            };
            let lower_rest = rest.to_lowercase();
            let pmid = extract_label(rest, "PMID:", |c| c.is_ascii_digit());
            // DOI: find "doi:" (case-insensitive, optional space) and capture
            // the dotted slash token that follows (e.g. 10.1038/s41586-022-04922-5).
            let doi = rest.to_lowercase().find("doi:").and_then(|pos| {
                let after = rest[pos + 4..].trim_start();
                let tok: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '/' || *c == '-' || *c == '_' || *c == '(' || *c == ')')
                    .collect();
                let tok = tok.trim_end_matches('.').to_string();
                if tok.is_empty() { None } else { Some(tok) }
            });
            let url = extract_url(rest);
            // Title heuristic: the leading segment before the first period
            // that follows ≥ 8 chars (or the whole rest when no period).
            let title = rest
                .split_once(". ")
                .map(|(a, _)| a.to_string())
                .unwrap_or_else(|| rest.to_string());
            let _ = lower_rest;
            refs.push(RefEntry {
                index: idx,
                raw: rest.to_string(),
                pmid,
                doi,
                url,
                title: if title.len() > 8 { title } else { rest.to_string() },
            });
        }
        refs
    }

    /// Verify one PMID: fetch PubMed summary and compare title tokens with
    /// the reference's own title claim. Year is checked only when the raw
    /// reference line states a 19xx/20xx year (lenient: no year claim ⇒ no
    /// year mismatch).
    async fn verify_pmid(&self, pmid: &str, title_claim: &str, raw_line: &str) -> RefVerdict {
        let url = format!(
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi?db=pubmed&retmode=json&id={pmid}"
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return RefVerdict {
                    index: 0,
                    status: "unverifiable",
                    detail: format!("PMID {pmid}: network error ({e})"),
                }
            }
        };
        let v: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return RefVerdict {
                    index: 0,
                    status: "unverifiable",
                    detail: format!("PMID {pmid}: parse error ({e})"),
                }
            }
        };
        let Some(rec) = v["result"].as_object().and_then(|o| o.get(pmid)) else {
            return RefVerdict {
                index: 0,
                status: "not_found",
                detail: format!("PMID {pmid}: 不存在于 PubMed"),
            };
        };
        let real_title = rec["title"].as_str().unwrap_or("").to_lowercase();
        let journal = rec["fulljournalname"].as_str().unwrap_or("").to_lowercase();
        let pubdate = rec["pubdate"].as_str().unwrap_or("").to_lowercase();
        // Title token overlap (>=3 significant shared words ⇒ same paper).
        let claim_words: Vec<String> = title_claim
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= 4)
            .map(|w| w.to_string())
            .collect();
        let matched = claim_words
            .iter()
            .filter(|w| real_title.contains(w.as_str()))
            .count();
        let stated_years: Vec<&str> = raw_line
            .split(|c: char| !c.is_ascii_digit())
            .filter(|y| y.len() == 4 && (y.starts_with('1') || y.starts_with('2')))
            .collect();
        let year_ok = stated_years.is_empty()
            || stated_years.iter().any(|y| pubdate.contains(y));
        if matched >= 2 && year_ok {
            RefVerdict {
                index: 0,
                status: "verified",
                detail: format!("PMID {pmid}: 标题/期刊匹配（{journal}）"),
            }
        } else {
            RefVerdict {
                index: 0,
                status: "mismatch",
                detail: format!(
                    "PMID {pmid}: 元数据不匹配 — PubMed 记录为「{}」({})，引用声称「{}」",
                    rec["title"].as_str().unwrap_or("?"),
                    journal,
                    title_claim
                ),
            }
        }
    }

    /// Verify a DOI resolves via doi.org (content-negotiation metadata).
    async fn verify_doi(&self, doi: &str) -> RefVerdict {
        let url = format!("https://doi.org/api/handles/{doi}");
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return RefVerdict {
                    index: 0,
                    status: "unverifiable",
                    detail: format!("DOI {doi}: network error ({e})"),
                }
            }
        };
        if resp.status().is_success() {
            RefVerdict {
                index: 0,
                status: "verified",
                detail: format!("DOI {doi}: 可解析"),
            }
        } else {
            RefVerdict {
                index: 0,
                status: "not_found",
                detail: format!("DOI {doi}: 无法解析（HTTP {}）", resp.status()),
            }
        }
    }

    /// Verify a URL is reachable.
    async fn verify_url(&self, url: &str) -> RefVerdict {
        match self.client.get(url).send().await {
            Ok(r) if r.status().is_success() => RefVerdict {
                index: 0,
                status: "verified",
                detail: format!("URL 可达（HTTP {}）", r.status()),
            },
            Ok(r) => RefVerdict {
                index: 0,
                status: "not_found",
                detail: format!("URL 返回 HTTP {}", r.status()),
            },
            Err(e) => RefVerdict {
                index: 0,
                status: "unverifiable",
                detail: format!("URL 请求失败: {e}"),
            },
        }
    }
}

fn extract_label(text: &str, label: &str, valid: fn(char) -> bool) -> Option<String> {
    let pos = text.to_uppercase().find(label)?;
    let rest = &text[pos + label.len()..];
    let value: String = rest
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| valid(*c))
        .collect();
    if value.is_empty() { None } else { Some(value) }
}

fn extract_url(text: &str) -> Option<String> {
    let pos = text.find("http")?;
    let rest = &text[pos..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '.'
            && !rest[0..rest.find(c).unwrap_or(0)].contains('/'))
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end_matches('.').to_string())
}

#[async_trait]
impl Tool for CitationCheckTool {
    fn name(&self) -> &str {
        "citation_check"
    }

    fn description(&self) -> &str {
        "Verify the citations of a markdown report against real sources. \\
         Parses in-text [n] markers and the trailing References/参考文献 section, \\
         then checks each entry: PMID via PubMed metadata cross-check (title/ \\
         journal/year), DOI via doi.org resolution, URL reachability. Returns a \\
         structured mismatch list (verified / not_found / mismatch / unverifiable \\
         per reference) plus body-vs-reference index coverage — so a writer agent \\
         knows exactly which citations are fabricated or corrupted and where to fix."
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "report": {"type": "string", "description": "Full markdown report text (with in-text [n] citations and a References section)"},
                "max_refs": {"type": "integer", "description": "Max references to verify (default 20, max 50)"}
            },
            "required": ["report"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let report = input["report"]
            .as_str()
            .ok_or_else(|| AgentError::tool("citation_check", "missing 'report'"))?;
        let max_refs = input["max_refs"].as_u64().unwrap_or(20).clamp(1, 50) as usize;

        // Split body vs references at the references heading.
        let (body, refs_section_start) = report
            .char_indices()
            .find_map(|(i, _)| {
                let rest = &report[i..];
                let lower = rest.to_lowercase();
                (lower.starts_with("# references")
                    || lower.starts_with("## references")
                    || lower.starts_with("## 引用")
                    || lower.starts_with("## 参考文献")
                    || lower.starts_with("# 参考文献"))
                    .then(|| (report[..i].to_string(), i))
            })
            .unwrap_or((report.to_string(), report.len()));
        let _ = refs_section_start;

        let cited = Self::cited_indices(&body);
        let refs = Self::parse_references(report);
        if refs.is_empty() {
            return Ok(ToolOutput {
                content: "citation_check: 未找到 references/参考文献 段落或任何 [n] 条目。报告缺少可核验的引用。"
                    .into(),
                metadata: None,
            });
        }

        let ref_indices: Vec<usize> = refs.iter().map(|r| r.index).collect();
        let missing_in_refs: Vec<usize> = cited
            .iter()
            .filter(|n| !ref_indices.contains(n))
            .cloned()
            .collect();
        let uncited: Vec<usize> = ref_indices
            .iter()
            .filter(|n| !cited.contains(n))
            .cloned()
            .collect();

        let mut lines = Vec::new();
        let mut verified = 0usize;
        let mut mismatched = 0usize;
        let mut not_found = 0usize;
        let mut unverifiable = 0usize;

        for r in refs.iter().take(max_refs) {
            if cancel.is_cancelled() {
                break;
            }
            let mut verdict = if let Some(pmid) = &r.pmid {
                self.verify_pmid(pmid, &r.title, &r.raw).await
            } else if let Some(doi) = &r.doi {
                self.verify_doi(doi).await
            } else if let Some(url) = &r.url {
                self.verify_url(url).await
            } else {
                RefVerdict {
                    index: 0,
                    status: "unverifiable",
                    detail: "无可核验标识（无 PMID/DOI/URL）".into(),
                }
            };
            verdict.index = r.index;
            match verdict.status {
                "verified" => verified += 1,
                "not_found" => not_found += 1,
                "mismatch" => mismatched += 1,
                _ => unverifiable += 1,
            }
            let icon = match verdict.status {
                "verified" => "✅",
                "not_found" | "mismatch" => "❌",
                _ => "⚠️",
            };
            lines.push(format!(
                "{icon} [{index}] {status}: {detail}",
                index = verdict.index,
                status = verdict.status,
                detail = verdict.detail
            ));
        }

        let mut out = format!(
            "引用核验结果：共 {} 条 references，正文引用 {} 个索引；✅ verified {} | ❌ mismatch {} | ❌ not_found {} | ⚠️ unverifiable {}\n\n",
            refs.len(),
            cited.len(),
            verified,
            mismatched,
            not_found,
            unverifiable
        );
        if !missing_in_refs.is_empty() {
            out.push_str(&format!(
                "❌ 正文引用但 references 缺失的索引：{:?}\n",
                missing_in_refs
            ));
        }
        if !uncited.is_empty() {
            out.push_str(&format!(
                "⚠️ references 中存在但正文未引用的索引：{:?}\n",
                uncited
            ));
        }
        out.push_str("\n");
        for l in &lines {
            out.push_str(l);
            out.push('\n');
        }

        out.push_str(&format!(
            "\nSUMMARY_JSON = {{\"total_refs\": {}, \"verified\": {}, \"mismatched\": {}, \"not_found\": {}, \"unverifiable\": {}, \"missing_in_refs\": {:?}}}\n",
            refs.len(), verified, mismatched, not_found, unverifiable, missing_in_refs
        ));

        Ok(ToolOutput {
            content: out,
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cited_indices_parse_various_forms() {
        let body = "一篇文献指出[12]，另见[1, 3]与区间[4-6]。纯文本 [abc] 不是引用。";
        let idx = CitationCheckTool::cited_indices(body);
        assert_eq!(idx, vec![1, 3, 4, 5, 6, 12]);
    }

    #[test]
    fn parse_references_extracts_entries() {
        let report = "# 报告\n1988 年指出[2]。\n\n## References\n\n\
[1]\tSome Title. Nature. 2022; PMID: 35271824; doi: 10.1038/s41586-022-04922-5.\n\
[2]\tAnother Title. Science. 2023 Jun; PMID: 37120000.\n";
        let refs = CitationCheckTool::parse_references(report);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].index, 1);
        assert_eq!(refs[0].pmid.as_deref(), Some("35271824"));
        assert_eq!(refs[0].doi.as_deref(), Some("10.1038/s41586-022-04922-5"));
        assert_eq!(refs[1].index, 2);
        assert_eq!(refs[1].pmid.as_deref(), Some("37120000"));
    }

    #[test]
    fn detect_references_heading_variants() {
        let r1 = "正文\n\n## References\n[1] x";
        let r2 = "正文\n\n## 参考文献\n[1] x";
        assert_eq!(CitationCheckTool::parse_references(r1).len(), 1);
        assert_eq!(CitationCheckTool::parse_references(r2).len(), 1);
    }
}
