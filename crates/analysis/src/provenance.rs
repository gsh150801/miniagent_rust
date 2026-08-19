//! Reproducibility provenance for executed data analyses.
//!
//! A [`ProvenanceRecord`] captures everything needed to re-run and audit an
//! analysis: the exact script (with hash), input/output files (with content
//! hashes), the conda environment + pinned package versions, the random seed,
//! the git commit, timing, and stdout/stderr digests. This is the audit trail
//! that makes the data-analysis track of a validation plan trustworthy.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A file referenced by provenance, with its size and a content hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: PathBuf,
    pub size_bytes: u64,
    /// FNV-1a 64-bit content hash (hex). Non-cryptographic; for change detection.
    pub hash: String,
}

/// Complete provenance for one analysis run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub task_id: String,
    pub hypothesis_ref: Option<uuid::Uuid>,
    pub script_path: PathBuf,
    pub script_hash: String,
    pub inputs: Vec<FileRecord>,
    pub outputs: Vec<FileRecord>,
    pub params: serde_json::Value,
    pub conda_env: String,
    /// Whether conda was actually used (false ⇒ system python fallback).
    pub conda_used: bool,
    pub package_versions: Vec<String>,
    pub seed: u64,
    pub git_commit: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration: Duration,
    pub exit_code: Option<i32>,
    pub stdout_hash: String,
    pub stderr_hash: String,
    pub stdout_preview: String,
    pub stderr_preview: String,
    /// Path to the generated Jupyter notebook (`.ipynb`), when produced.
    #[serde(default)]
    pub notebook_path: Option<PathBuf>,
    /// Whether the `.ipynb` itself was executed in place (has outputs). False
    /// when only the `.py` script ran or it was a dry-run.
    #[serde(default)]
    pub notebook_executed: bool,
    /// `"jupyter"`, `"python"`, or `"dry_run"` — how the analysis was executed.
    #[serde(default)]
    pub execution_backend: String,
}

impl ProvenanceRecord {
    /// Serialize to pretty JSON for on-disk audit storage.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Build a [`FileRecord`] for a path if it exists.
pub fn record_file(path: &Path) -> Option<FileRecord> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(FileRecord {
        path: path.to_path_buf(),
        size_bytes: meta.len(),
        hash: sha256_hex(&bytes),
    })
}

/// Record every regular file under `dir` (non-recursive on top level + one
/// shallow level for typical `figures/` / `tables/` output layouts).
pub fn record_dir_shallow(dir: &Path) -> Vec<FileRecord> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&p) {
                for se in sub.flatten() {
                    if let Some(r) = record_file(&se.path()) {
                        out.push(r);
                    }
                }
            }
        } else if let Some(r) = record_file(&p) {
            out.push(r);
        }
    }
    out
}

/// Capture the current git commit of `working_dir` (best-effort).
pub fn current_git_commit(working_dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(working_dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// FNV-1a 64-bit hash of bytes, returned as lowercase hex.
///
/// Kept for legacy provenance records; new records use [`sha256_hex`]
/// (FNV-1a is non-cryptographic — collisions are trivially craftable, which
/// undermines "these bytes are provably the ones that ran").
pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// SHA-256 of bytes as lowercase hex — the provenance hash of record.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Truncate a potentially large buffer to a preview string (UTF-8 lossy, capped).
pub fn preview(bytes: &[u8], max_chars: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.chars().count() <= max_chars {
        s.into_owned()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...[+{} bytes truncated]", bytes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn fnv1a_is_deterministic() {
        assert_eq!(fnv1a_hex(b"hello"), fnv1a_hex(b"hello"));
        assert_ne!(fnv1a_hex(b"hello"), fnv1a_hex(b"world"));
        // Known FNV-1a 64 value for "" is the offset basis.
        assert_eq!(fnv1a_hex(b""), format!("{:016x}", 0xcbf29ce484222325u64));
    }

    #[test]
    fn sha256_known_vector() {
        // Well-known SHA-256 of the empty string and of "abc".
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn record_file_captures_hash_and_size() {
        let dir = std::env::temp_dir().join("miniagent_provenance_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.csv");
        let content = b"gene,log2fc\nBRCA1,-2.3\n";
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content).unwrap();
        let rec = record_file(&path).unwrap();
        assert_eq!(rec.size_bytes, content.len() as u64);
        assert_eq!(rec.hash.len(), 64);
        assert_eq!(rec.hash, sha256_hex(content));
        assert!(!rec.hash.contains(' '));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn preview_truncates_long_output() {
        let big = "a".repeat(5000).into_bytes();
        let p = preview(&big, 100);
        assert!(p.contains("truncated"));
        assert!(p.chars().count() < 200);
    }

    #[test]
    fn preview_keeps_short_output() {
        let p = preview(b"ok", 100);
        assert_eq!(p, "ok");
    }

    #[test]
    fn provenance_serializes_roundtrip() {
        let rec = ProvenanceRecord {
            task_id: "DA-1".into(),
            hypothesis_ref: Some(uuid::Uuid::new_v4()),
            script_path: PathBuf::from("script.py"),
            script_hash: fnv1a_hex(b"print(1)"),
            inputs: vec![],
            outputs: vec![],
            params: serde_json::json!({"alpha": 0.05}),
            conda_env: "mn_da1".into(),
            conda_used: true,
            package_versions: vec!["pandas==2.0".into()],
            seed: 42,
            git_commit: Some("abc123".into()),
            started_at: Utc::now(),
            duration: Duration::seconds(12),
            exit_code: Some(0),
            stdout_hash: fnv1a_hex(b"done"),
            stderr_hash: fnv1a_hex(b""),
            stdout_preview: "done".into(),
            stderr_preview: "".into(),
            notebook_path: Some(PathBuf::from("analysis.ipynb")),
            notebook_executed: true,
            execution_backend: "jupyter".into(),
        };
        let json = rec.to_json_pretty().unwrap();
        assert!(json.contains("\"task_id\": \"DA-1\""));
        assert!(json.contains("\"seed\": 42"));
        let _: ProvenanceRecord = serde_json::from_str(&json).unwrap();
    }
}
