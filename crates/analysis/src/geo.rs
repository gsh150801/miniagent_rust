//! GEO dataset downloading.
//!
//! Turns a `GSE…` accession into a local TSV the analysis runner can
//! actually execute against, killing the "dry-run: no local data" path for
//! public-dataset tasks. Downloads the series-matrix file from GEO's FTP
//! mirror, decompresses it, and rewrites it as a clean TSV:
//!
//! * `ATTR_Sample_*` rows (title, geo_accession, characteristics, …) carry
//!   the per-sample metadata needed for cohort grouping,
//! * followed by the expression matrix between the
//!   `!series_matrix_table_begin/end` markers.
//!
//! Both blocks share the same column layout (samples as columns), so a
//! single `read_csv(sep='\t')` in the generated notebook sees one table.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Derive the GEO FTP bucket directory for an accession:
/// GSE12345 → "GSE12nnn", GSE120003 → "GSE120nnn".
pub fn geo_bucket(accession: &str) -> Option<String> {
    let digits = accession.strip_prefix("GSE").or_else(|| accession.strip_prefix("gse"))?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    let bucket = n / 1000;
    Some(format!("GSE{bucket}nnn"))
}

/// Build a compact, generation- and repair-friendly summary of a cleaned
/// GEO series-matrix TSV: every `ATTR_Sample_*` key with its first few
/// values, the expression-table header, the first probe rows, and the
/// matrix dimensions. `None` when the file does not look like a cleaned
/// series matrix.
///
/// Scripts generated from raw 20-line previews kept referencing columns that
/// do not exist (observed live: `KeyError: 'ATTR_Sample_title'` on a series
/// whose title attribute has a different name). A schema-level summary lets
/// the LLM write — and later repair — against the real structure.
///
/// Memory-safe: streams the file instead of loading it (series matrices can
/// exceed 100 MB).
pub fn summarize_series_matrix(path: &Path) -> Option<String> {
    use std::io::BufRead;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut attrs: Vec<(String, Vec<String>)> = Vec::new();
    let mut header: Option<String> = None;
    let mut first_rows: Vec<String> = Vec::new();
    let mut n_expr_rows = 0usize;
    let mut saw_attr = false;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if let Some(rest) = line.strip_prefix("ATTR_Sample_") {
            saw_attr = true;
            let (key, values) = split_row(&format!("ATTR_Sample_{rest}"));
            attrs.push((key, values));
        } else if line.starts_with("ID_REF") || line.starts_with("\"ID_REF") {
            header = Some(line);
        } else if header.is_some() && !line.trim().is_empty() {
            if first_rows.len() < 3 {
                let (row_id, _) = split_row(&line);
                first_rows.push(row_id);
            }
            n_expr_rows += 1;
        } else if header.is_some() && !line.trim().is_empty() {
            n_expr_rows += 1;
        }
    }
    if !saw_attr {
        return None;
    }

    let n_samples = attrs.first().map(|(_, v)| v.len()).unwrap_or(0);
    let mut out = String::new();
    out.push_str(&format!(
        "Cleaned GEO series matrix: samples are COLUMNS ({n_samples}); per-sample metadata rows are keyed ATTR_Sample_*, the expression matrix follows with genes/probes as rows (ID_REF first column).\n"
    ));
    out.push_str("Sample attribute keys (key: first 3 values):\n");
    for (key, values) in attrs.iter().take(40) {
        let preview: Vec<String> = values.iter().take(3).map(|v| shorten(v, 40)).collect();
        out.push_str(&format!("  {key}: {}\n", preview.join(" | ")));
    }
    if attrs.len() > 40 {
        out.push_str(&format!("  … (+{} more attribute rows)\n", attrs.len() - 40));
    }
    if let Some(h) = header {
        out.push_str(&format!(
            "Expression table header (first 6 columns): {}\n",
            h.split('\t').take(6).collect::<Vec<_>>().join(", ")
        ));
    }
    if !first_rows.is_empty() {
        out.push_str(&format!("First row IDs: {}\n", first_rows.join(", ")));
    }
    out.push_str(&format!(
        "Expression matrix: {n_expr_rows} gene/probe rows; load with pandas.read_csv(path, sep='\\t') — samples are columns, so cohort/phenotype data lives in the ATTR_Sample_* rows."
    ));
    Some(out)
}

/// Split a TSV row into its row key and cell values.
fn split_row(line: &str) -> (String, Vec<String>) {
    let mut parts = line.split('\t');
    let key = parts.next().unwrap_or_default().trim_matches('"').to_string();
    let values: Vec<String> = parts.map(|v| v.trim_matches('"').to_string()).collect();
    (key, values)
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Download (with on-disk cache) the series matrix for `accession` and write
/// the cleaned TSV into `dest_dir`. Returns the TSV path.
///
/// Multi-platform series have no combined matrix (the combined URL 404s);
/// in that case the per-platform `{GSE}-{GPL}_series_matrix.txt.gz` files are
/// listed and the largest is downloaded instead. Large downloads get one
/// retry on transient connection resets.
pub async fn download_geo_series_matrix(
    accession: &str,
    dest_dir: &Path,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<PathBuf, String> {
    let accession = accession.trim().to_uppercase();
    let bucket = geo_bucket(&accession)
        .ok_or_else(|| format!("invalid GEO accession: {accession}"))?;
    let dest = dest_dir.join(format!("{accession}_series_matrix.tsv"));
    if dest.exists() {
        return Ok(dest); // cached from a previous run
    }
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("create {}: {e}", dest_dir.display()))?;

    let dir_url = format!("https://ftp.ncbi.nlm.nih.gov/geo/series/{bucket}/{accession}/matrix/");
    let mut url = format!("{dir_url}{accession}_series_matrix.txt.gz");

    let client = reqwest::Client::builder()
        .user_agent("miniagent/0.1")
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;

    // Multi-platform fallback: no combined matrix → pick the largest
    // per-platform matrix from the directory listing. Single attempt — a
    // throttled NCBI link must not stall the pipeline for 2× the timeout.
    let first = fetch_gz(&client, &url, &cancel, true).await;
    let gz = match first {
        Ok(b) => b,
        Err(e) if e.contains("404") => {
            let platform = largest_platform_matrix(&client, &dir_url, &accession, &cancel)
                .await
                .ok_or_else(|| {
                    format!(
                        "{accession}: no series matrix (combined 404 and no per-platform file listed)"
                    )
                })?;
            url = format!("{dir_url}{platform}");
            fetch_gz(&client, &url, &cancel, false)
                .await
                .map_err(|e| format!("download {url}: {e}"))?
        }
        Err(e) => return Err(format!("download {url}: {e}")),
    };

    // Decompress and clean in one pass, then gate usability BEFORE persisting:
    // a matrix without sample attributes or expression data only produces
    // analyses that fail mid-run.
    let mut decoder = flate2::read::GzDecoder::new(&gz[..]);
    let mut raw = String::new();
    decoder
        .read_to_string(&mut raw)
        .map_err(|e| format!("gunzip {accession}: {e}"))?;
    let (tsv, stats) = clean_series_matrix_with_stats(&raw);
    validate_cleaned_tsv(&tsv).map_err(|e| format!("{accession}: {e}"))?;
    tracing::info!(
        accession = %accession,
        attr_rows = stats.attr_rows,
        expr_rows = stats.expr_rows,
        "GEO series matrix usable"
    );
    std::fs::write(&dest, tsv).map_err(|e| format!("write {}: {e}", dest.display()))?;
    tracing::info!(accession = %accession, path = %dest.display(), "GEO series matrix downloaded");
    Ok(dest)
}

/// Fetch a `.txt.gz` body, optionally retrying once on transient failures.
async fn fetch_gz(
    client: &reqwest::Client,
    url: &str,
    cancel: &tokio_util::sync::CancellationToken,
    retry: bool,
) -> Result<Vec<u8>, String> {
    let attempts = if retry { 2 } else { 1 };
    let mut last_err = String::new();
    for _ in 0..attempts {
        let fetched = tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled".into()),
            resp = client.get(url).send() => resp
                .map_err(|e| format!("request: {e}"))?
                .error_for_status()
                .map_err(|e| format!("{e}"))?
                .bytes()
                .await
                .map_err(|e| format!("read body: {e}")),
        };
        match fetched {
            Ok(b) => return Ok(b.to_vec()),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Pick the `{GSE}-{GPL}_series_matrix.txt.gz` with the largest listed size
/// from an FTP-style HTML directory listing (pure parser, unit-testable).
fn pick_platform_from_listing(listing: &str, accession: &str) -> Option<String> {
    let mut best: Option<(String, u64)> = None;
    for line in listing.lines() {
        let Some(idx) = line.find(&format!("{accession}-GPL")) else {
            continue;
        };
        let name: String = line[idx..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"')
            .collect();
        if !name.ends_with("_series_matrix.txt.gz") {
            continue;
        }
        let size = line
            .split_whitespace()
            .filter(|t| t.ends_with('M') || t.ends_with('K') || t.chars().all(|c| c.is_ascii_digit()))
            .filter_map(parse_size)
            .max()
            .unwrap_or(0);
        if best.as_ref().is_none_or(|(_, s)| size > *s) {
            best = Some((name, size));
        }
    }
    best.map(|(name, _)| name)
}

/// Fetch the FTP directory listing and pick the largest per-platform matrix.
async fn largest_platform_matrix(
    client: &reqwest::Client,
    dir_url: &str,
    accession: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<String> {
    let listing = tokio::select! {
        _ = cancel.cancelled() => return None,
        resp = client.get(dir_url).send() => resp.ok()?.text().await.ok()?,
    };
    pick_platform_from_listing(&listing, accession)
}

/// Parse an FTP-listing size token (`34M`, `512K`, `1048576`) into bytes.
fn parse_size(tok: &str) -> Option<u64> {
    let tok = tok.trim();
    if let Some(m) = tok.strip_suffix('M') {
        m.trim().parse::<f64>().ok().map(|v| (v * 1024.0 * 1024.0) as u64)
    } else if let Some(k) = tok.strip_suffix('K') {
        k.trim().parse::<f64>().ok().map(|v| (v * 1024.0) as u64)
    } else {
        tok.parse::<u64>().ok()
    }
}

/// Keep `!Sample_*` metadata rows (renamed `ATTR_Sample_*`) plus the
/// expression matrix between the table markers; drop `!Series_*` bookkeeping.
pub fn clean_series_matrix(raw: &str) -> String {
    clean_series_matrix_with_stats(raw).0
}

/// [`clean_series_matrix`] plus usability counts: the number of
/// `ATTR_Sample_*` metadata rows and the number of expression data rows
/// (table lines after the `ID_REF` header).
pub fn clean_series_matrix_with_stats(raw: &str) -> (String, MatrixStats) {
    let mut out = String::new();
    let mut in_table = false;
    let mut attr_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut stats = MatrixStats::default();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("!series_matrix_table_begin") {
            in_table = true;
            continue;
        }
        if line.starts_with("!series_matrix_table_end") {
            in_table = false;
            continue;
        }
        if in_table {
            if line.starts_with("ID_REF") || line.starts_with("\"ID_REF") {
                stats.has_header = true;
            } else if !line.trim().is_empty() {
                stats.expr_rows += 1;
            }
            out.push_str(line);
            out.push('\n');
        } else if let Some(rest) = line.strip_prefix("!Sample_") {
            // `!Sample_title\tA\tB` → `ATTR_Sample_title\tA\tB`. Repeated
            // keys (characteristics_ch1) get a stable numeric suffix so
            // rows remain distinguishable after renaming.
            let (key_part, value_part) = rest.split_once('\t').unwrap_or((rest, ""));
            let key = format!("ATTR_Sample_{key_part}");
            let n = attr_counts.entry(key.clone()).or_insert(0);
            *n += 1;
            let final_key = if *n > 1 {
                format!("{key}__{}", *n - 1)
            } else {
                key
            };
            stats.attr_rows += 1;
            out.push_str(&final_key);
            out.push('\t');
            out.push_str(value_part);
            out.push('\n');
        }
    }
    (out, stats)
}

/// Usability counts of a cleaned series-matrix TSV.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MatrixStats {
    /// Number of `ATTR_Sample_*` metadata rows written.
    pub attr_rows: usize,
    /// Number of expression data rows (table lines after the `ID_REF` header).
    pub expr_rows: usize,
    /// Whether an `ID_REF` expression-table header was present.
    pub has_header: bool,
}

/// Gate a cleaned TSV before feeding it to downstream analysis: a matrix
/// without sample metadata or without expression data produces broken
/// analyses that only fail mid-run (observed live: scripts raising
/// "No sample attributes parsed" / "expression rows = 0"). Returns a
/// descriptive error naming exactly what is missing.
pub fn validate_cleaned_tsv(tsv: &str) -> Result<(), String> {
    let trimmed = tsv.trim();
    if trimmed.is_empty() {
        return Err(
            "series matrix contains no expression table (RNA-seq counts-only series often \
             lack one — see the GEO page)"
                .to_string(),
        );
    }
    if !trimmed.lines().any(|l| l.starts_with("ATTR_Sample_")) {
        return Err("series matrix has no per-sample attribute rows (ATTR_Sample_*) — \
                    cohorts cannot be defined"
            .to_string());
    }
    let header_cols = trimmed
        .lines()
        .find(|l| l.starts_with("ID_REF") || l.starts_with("\"ID_REF"))
        .map(|l| l.split('\t').count())
        .unwrap_or(0);
    if !trimmed
        .lines()
        .any(|l| l.starts_with("ID_REF") || l.starts_with("\"ID_REF"))
    {
        return Err("series matrix has no ID_REF expression-table header".to_string());
    }
    if header_cols < 2 {
        return Err("expression table header has no sample columns".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_derivation() {
        assert_eq!(geo_bucket("GSE12345").as_deref(), Some("GSE12nnn"));
        assert_eq!(geo_bucket("GSE120003").as_deref(), Some("GSE120nnn"));
        assert_eq!(geo_bucket("GSE7").as_deref(), Some("GSE0nnn"));
        assert_eq!(geo_bucket("gse529").as_deref(), Some("GSE0nnn"));
        assert_eq!(geo_bucket("GPL123"), None);
        assert_eq!(geo_bucket("GSE12x"), None);
    }

    #[test]
    fn cleaning_keeps_attrs_and_matrix() {
        let raw = "!Series_title\tFoo\n\
                   !Sample_title\tA\tB\n\
                   !Sample_characteristics_ch1\tcontrol\tdisease\n\
                   !Sample_characteristics_ch1\tage50\tage60\n\
                   !series_matrix_table_begin\n\
                   ID_REF\tA\tB\n\
                   1007_s_at\t3.1\t4.2\n\
                   !series_matrix_table_end\n\
                   !Series_end\n";
        let tsv = clean_series_matrix(raw);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(
            lines,
            vec![
                "ATTR_Sample_title\tA\tB",
                "ATTR_Sample_characteristics_ch1\tcontrol\tdisease",
                "ATTR_Sample_characteristics_ch1__1\tage50\tage60",
                "ID_REF\tA\tB",
                "1007_s_at\t3.1\t4.2",
            ]
        );
        assert!(!tsv.contains("!Series"));
    }

    #[test]
    fn empty_matrix_is_empty() {
        assert!(clean_series_matrix("!Series_title\tfoo\n").trim().is_empty());
    }

    #[test]
    fn usability_gate_rejects_unusable_matrices() {
        // No ATTR rows at all.
        let no_attrs = "ID_REF\tGSM1\n1007_s_at\t3.1\n";
        assert!(validate_cleaned_tsv(no_attrs).is_err());
        // No expression table.
        let no_table = "ATTR_Sample_title\tA\tB\n";
        assert!(validate_cleaned_tsv(no_table).is_err());
        // Header without sample columns.
        let no_cols = "ATTR_Sample_title\nID_REF\n1.0\n";
        assert!(validate_cleaned_tsv(no_cols).is_err());
        // Fully usable (quoted header, as GEO writes it).
        let good = "ATTR_Sample_title\t\"AD\"\t\"CTL\"\n\"ID_REF\"\tGSM1\tGSM2\n1007_s_at\t3.1\t4.2\n";
        assert!(validate_cleaned_tsv(good).is_ok());
    }

    #[test]
    fn stats_count_attr_and_expr_rows() {
        let raw = "!Sample_title\tA\tB\n\
                   !Sample_characteristics_ch1\tdx\n\
                   !series_matrix_table_begin\n\
                   ID_REF\tGSM1\tGSM2\n\
                   probe1\t1\t2\n\
                   probe2\t3\t4\n\
                   !series_matrix_table_end\n";
        let (_, stats) = clean_series_matrix_with_stats(raw);
        assert_eq!(stats.attr_rows, 2);
        assert_eq!(stats.expr_rows, 2);
        assert!(stats.has_header);
    }

    #[test]
    fn summarize_reports_attrs_header_and_dims() {
        let dir = std::env::temp_dir().join("miniagent_geo_summarize_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("GSEtest_series_matrix.tsv");
        let body = "ATTR_Sample_title\tctl_1\tctl_2\tad_1\n\
                    ATTR_Sample_characteristics_ch1\tcontrol\tcontrol\tAlzheimer\n\
                    ID_REF\tGSM1\tGSM2\tGSM3\n\
                    1007_s_at\t3.1\t4.2\t5.0\n\
                    1130_s_at\t1.0\t2.0\t3.0\n";
        std::fs::write(&path, body).unwrap();
        let summary = summarize_series_matrix(&path).expect("should summarize");
        assert!(summary.contains("3)"), "sample count: {summary}");
        assert!(summary.contains("ATTR_Sample_title: ctl_1"));
        assert!(summary.contains("Alzheimer"));
        assert!(summary.contains("ID_REF"), "header listed: {summary}");
        assert!(summary.contains("1007_s_at"), "first row ids: {summary}");
        assert!(summary.contains("2 gene/probe rows"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn summarize_non_geo_file_is_none() {
        let dir = std::env::temp_dir().join("miniagent_geo_summarize_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.csv");
        std::fs::write(&path, "a,b\n1,2\n").unwrap();
        assert!(summarize_series_matrix(&path).is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn platform_listing_picks_largest_matrix() {
        // Shape captured from the live GSE63063 matrix/ directory listing.
        let listing = "<pre>Name                         Last modified  Size  <hr>\n\
                       <a href=\"GSE63063-GPL10558_series_matrix.txt.gz\">GSE63063-GPL10558_series_matrix.txt.gz</a> 2026-05-28 01:30   61M\n\
                       <a href=\"GSE63063-GPL6947_series_matrix.txt.gz\">GSE63063-GPL6947_series_matrix.txt.gz</a> 2026-05-28 01:31   60M\n\
                       <a href=\"GSE63063-GPL96_supplementary.txt.gz\">GSE63063-GPL96_supplementary.txt.gz</a> 2026-05-28 01:31   2M\n";
        assert_eq!(
            pick_platform_from_listing(listing, "GSE63063").as_deref(),
            Some("GSE63063-GPL10558_series_matrix.txt.gz")
        );
        // Non-matrix and supplementary files are never picked.
        assert_eq!(pick_platform_from_listing("no matching files", "GSE63063"), None);
    }

    #[test]
    fn size_tokens_parse() {
        assert_eq!(parse_size("34M"), Some(34 * 1024 * 1024));
        assert_eq!(parse_size("512K"), Some(512 * 1024));
        assert_eq!(parse_size("1048576"), Some(1048576));
        assert_eq!(parse_size("-"), None);
    }
}

#[cfg(test)]
mod geo_live_tests {
    use super::*;

    /// Live-network end-to-end check of the multi-platform fallback. Ignored
    /// in normal test runs: NCBI throttles large transfers (a 61 MB platform
    /// matrix can exceed the 10-minute timeout on a throttled link), so this
    /// only passes on a healthy connection. The listing parser itself is
    /// covered by `platform_listing_picks_largest_matrix`.
    #[tokio::test]
    #[ignore = "live network; NCBI throughput-dependent (61 MB download)"]
    async fn multi_platform_series_falls_back_to_largest_platform_matrix() {
        let dir = std::env::temp_dir().join("miniagent_geo_live");
        let _ = std::fs::remove_dir_all(&dir);
        match download_geo_series_matrix(
            "GSE63063",
            &dir,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        {
            Ok(p) => {
                assert!(p.exists());
                let summary = summarize_series_matrix(&p).expect("summary");
                assert!(summary.contains("samples are COLUMNS"));
            }
            Err(e) => panic!("multi-platform download should fall back: {e}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
