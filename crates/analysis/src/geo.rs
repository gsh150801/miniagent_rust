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

/// Download (with on-disk cache) the series matrix for `accession` and write
/// the cleaned TSV into `dest_dir`. Returns the TSV path.
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

    let url = format!(
        "https://ftp.ncbi.nlm.nih.gov/geo/series/{bucket}/{accession}/matrix/{accession}_series_matrix.txt.gz"
    );
    let client = reqwest::Client::builder()
        .user_agent("miniagent/0.1")
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let gz = tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled".into()),
        resp = client.get(&url).send() => resp
            .map_err(|e| format!("request {url}: {e}"))?
            .error_for_status()
            .map_err(|e| format!("download {url}: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("read body: {e}"))?,
    };

    // Decompress and clean in one pass.
    let mut decoder = flate2::read::GzDecoder::new(&gz[..]);
    let mut raw = String::new();
    decoder
        .read_to_string(&mut raw)
        .map_err(|e| format!("gunzip {accession}: {e}"))?;
    let tsv = clean_series_matrix(&raw);
    if tsv.trim().is_empty() {
        return Err(format!(
            "{accession}: series matrix contains no expression table \
             (RNA-seq counts-only series often lack one — see the GEO page)"
        ));
    }
    std::fs::write(&dest, tsv).map_err(|e| format!("write {}: {e}", dest.display()))?;
    tracing::info!(accession = %accession, path = %dest.display(), "GEO series matrix downloaded");
    Ok(dest)
}

/// Keep `!Sample_*` metadata rows (renamed `ATTR_Sample_*`) plus the
/// expression matrix between the table markers; drop `!Series_*` bookkeeping.
pub fn clean_series_matrix(raw: &str) -> String {
    let mut out = String::new();
    let mut in_table = false;
    let mut attr_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
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
            out.push_str(&final_key);
            out.push('\t');
            out.push_str(value_part);
            out.push('\n');
        }
    }
    out
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
}
