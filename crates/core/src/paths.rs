//! Centralised path anchoring for everything the harness writes.
//!
//! Historical defect: every crate resolved its output location relative to
//! the process CWD (`./result`, `./miniagent_context`, `./miniagent_workspace`,
//! `models.json`). When the server/CLI binary was launched from another
//! directory, artifacts silently scattered outside the repo's `result/` tree
//! (the `.worktrees/result/...` escape class). This module is the single
//! source of truth: resolve once, anchor absolutely, canonicalize.
//!
//! Override hierarchy (highest wins):
//! 1. `MINIAGENT_RESULT_DIR` — absolute path used directly as the result root.
//! 2. `MINIAGENT_ROOT` — pins the workspace root; results go to `<root>/result`.
//! 3. Walk up from the process CWD (then the executable) to the first
//!    directory that looks like the workspace root (a `Cargo.toml`
//!    containing `[workspace]`, or a `.miniagent-root` marker file).
//! 4. Fall back to the process CWD (previous behaviour, never worse).

use std::path::{Path, PathBuf};

/// Env var pinning the workspace root (absolute path wins over walk-up).
pub const ROOT_ENV: &str = "MINIAGENT_ROOT";
/// Env var overriding the result root outright (absolute path).
pub const RESULT_DIR_ENV: &str = "MINIAGENT_RESULT_DIR";

fn env_abs_path(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn looks_like_workspace_root(dir: &Path) -> bool {
    if dir.join(".miniagent-root").is_file() {
        return true;
    }
    match std::fs::read_to_string(dir.join("Cargo.toml")) {
        Ok(txt) => txt.contains("[workspace]"),
        Err(_) => false,
    }
}

/// Walk up from `start` to the first ancestor that looks like the workspace
/// root. Returns `None` when no ancestor qualifies.
fn ancestor_root(start: Option<&Path>) -> Option<PathBuf> {
    let mut cur: PathBuf = start?.to_path_buf();
    loop {
        if looks_like_workspace_root(&cur) {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// The workspace root directory (see module docs for the hierarchy).
pub fn workspace_root() -> PathBuf {
    if let Some(root) = env_abs_path(ROOT_ENV) {
        return root;
    }
    let cwd = std::env::current_dir().ok();
    if let Some(root) = ancestor_root(cwd.as_deref()) {
        return root;
    }
    // Binary may live in target/debug while launched from anywhere.
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
    if let Some(root) = ancestor_root(exe.as_deref()) {
        return root;
    }
    cwd.unwrap_or_else(|| PathBuf::from("."))
}

/// The root directory holding every run result: `result/{id}_{brief}`.
/// Created if missing, canonicalized so downstream joins stay absolute.
pub fn result_root() -> PathBuf {
    if let Some(dir) = env_abs_path(RESULT_DIR_ENV) {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "failed to create MINIAGENT_RESULT_DIR");
            return dir;
        }
        return dir.canonicalize().unwrap_or(dir);
    }
    let dir = workspace_root().join("result");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(path = %dir.display(), error = %e, "failed to create result root");
        return dir;
    }
    dir.canonicalize().unwrap_or(dir)
}

/// Location of the runtime model registry file (workspace root, CWD-independent).
pub fn models_file() -> PathBuf {
    workspace_root().join("models.json")
}

/// Human- and filesystem-safe task brief derived from the user prompt.
/// Shared by server and CLI so both produce identical `{id}_{brief}` dirs
/// and the server restart scan picks CLI runs up.
pub fn sanitize_task_brief(prompt: &str) -> String {
    let brief: String = prompt
        .chars()
        .take(30)
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let brief = brief.trim_end_matches('_');
    if brief.is_empty() {
        "task".into()
    } else {
        brief.into()
    }
}

/// Canonical run-directory name: `{8-char-id}_{brief}`.
pub fn task_dir_name(task_id: &str, brief: &str) -> String {
    format!("{task_id}_{brief}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_sanitizes_whitespace_and_punctuation() {
        assert_eq!(sanitize_task_brief("总结15-26年的ALS疾病 相关文献!"), "总结15-26年的ALS疾病_相关文献");
        assert_eq!(sanitize_task_brief("   "), "task");
        assert_eq!(sanitize_task_brief("hello world foo bar baz qux quux"), "hello_world_foo_bar_baz_qux_qu");
    }

    #[test]
    fn task_dir_name_joins_id_and_brief() {
        assert_eq!(task_dir_name("1c822612", "总结文献"), "1c822612_总结文献");
    }

    #[test]
    fn workspace_root_found_from_crate_subdir() {
        // Test CWD is the crate dir; the walk-up must reach the workspace root.
        let root = workspace_root();
        assert!(root.join("Cargo.toml").exists(), "root={}", root.display());
        assert!(looks_like_workspace_root(&root));
    }
}
