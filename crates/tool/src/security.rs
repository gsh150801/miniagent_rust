use std::path::{Path, PathBuf};

/// Check if `child_path` is strictly within `base_dir`.
/// Both are canonicalized (resolved to absolute, symlink-resolved paths) before comparison.
pub fn is_path_within_base(child_path: &Path, base_dir: &Path) -> bool {
    let Ok(canon_child) = child_path.canonicalize() else {
        return false;
    };
    let Ok(canon_base) = base_dir.canonicalize() else {
        return false;
    };
    canon_child.starts_with(&canon_base)
}

/// Resolve a potentially non-canonical path relative to a base, then check it's within base.
/// If `path` is relative, it's resolved relative to `base_dir`.
/// If `path` is absolute, it must be within `base_dir`.
/// Returns the resolved path if it's safe, or an error message otherwise.
pub fn resolve_safe_path(path_str: &str, base_dir: &Path) -> Result<PathBuf, String> {
    let base_canon = base_dir.canonicalize().map_err(|e| format!("Cannot resolve base dir: {e}"))?;

    let target = PathBuf::from(path_str);
    let target = if target.is_relative() {
        base_canon.join(&target)
    } else {
        target
    };

    let Ok(target_canon) = target.canonicalize() else {
        // Target doesn't exist yet (writing a new file).
        // For non-existent paths, check the parent directory instead.
        if let Some(parent) = target.parent() {
            let Ok(parent_canon) = parent.canonicalize() else {
                return Err(format!("Parent directory '{}' does not exist", parent.display()));
            };
            if !parent_canon.starts_with(&base_canon) {
                return Err(format!(
                    "Path '{}' is outside the working directory '{}'",
                    path_str, base_canon.display()
                ));
            }
            return Ok(target);
        }
        return Err(format!("Invalid path '{path_str}'"));
    };

    if target_canon.starts_with(&base_canon) {
        Ok(target_canon)
    } else {
        Err(format!(
            "Path '{}' resolves to '{}' which is outside working directory '{}'",
            path_str, target_canon.display(), base_canon.display()
        ))
    }
}

/// Check if a path string looks like it belongs to the system conda environment.
/// System conda is typically at paths like: /opt/conda, /usr/local/conda, ~/miniconda3, ~/anaconda3
pub fn is_system_conda_path(env_path: &str) -> bool {
    let env_path_lower = env_path.to_lowercase();
    // Check for known system conda prefixes
    if env_path.starts_with('/') || env_path.starts_with("~/") {
        let indicators = [
            "miniconda", "anaconda", "miniforge", "mambaforge",
            "/opt/conda", "/usr/local/conda",
        ];
        indicators.iter().any(|&i| env_path_lower.contains(i))
    } else {
        // Environment name only — check if it exists in system conda
        // This is a best-effort check; the approval handler will do the final enforcement
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_path_within_base() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("subdir").join("file.txt");
        fs::create_dir_all(child.parent().unwrap()).unwrap();
        fs::write(&child, "content").unwrap();
        assert!(is_path_within_base(&child, dir.path()));
    }

    #[test]
    fn test_path_outside_base() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        assert!(!is_path_within_base(dir2.path(), dir1.path()));
    }

    #[test]
    fn test_resolve_safe_relative() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        let f = nested.join("f.txt");
        fs::write(&f, "").unwrap();

        let result = resolve_safe_path("nested/f.txt", dir.path());
        assert!(result.is_ok(), "Should resolve relative path within base");
    }

    #[test]
    fn test_resolve_safe_absolute_ok() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f.txt");
        fs::write(&f, "").unwrap();
        let result = resolve_safe_path(&f.to_string_lossy(), dir.path());
        assert!(result.is_ok(), "Should accept absolute path within base");
    }

    #[test]
    fn test_resolve_safe_outside() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let result = resolve_safe_path(&outside.path().to_string_lossy(), dir.path());
        assert!(result.is_err(), "Should reject path outside base");
    }

    #[test]
    fn test_resolve_safe_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let new_file = dir.path().join("new_file.txt");
        // File doesn't exist yet — should still succeed if parent is within base
        let result = resolve_safe_path(&new_file.to_string_lossy(), dir.path());
        assert!(result.is_ok(), "Should allow creating new file within base");
    }

    #[test]
    fn test_system_conda_detection() {
        assert!(is_system_conda_path("/opt/conda/envs/myenv"));
        assert!(is_system_conda_path("/home/user/miniconda3/envs/myenv"));
        assert!(is_system_conda_path("~/anaconda3/envs/myenv"));
        assert!(!is_system_conda_path("./my_local_env"));
        assert!(!is_system_conda_path("myenv_name_only"));
    }
}
