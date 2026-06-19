// Simple glob matching without external crate dependency.
// Used by both the glob tool and the grep tool.

pub fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" || pattern == "*.*" {
        return true;
    }
    if pattern.starts_with("*.") {
        return name.ends_with(&pattern[1..]);
    }
    if pattern.starts_with('*') && pattern.ends_with('*') {
        return name.contains(&pattern[1..pattern.len()-1]);
    }
    if let Some(star_pos) = pattern.find('*') {
        let prefix = &pattern[..star_pos];
        let suffix = &pattern[star_pos+1..];
        return name.starts_with(prefix) && name.ends_with(suffix);
    }
    pattern == name
}

pub fn glob_files(pattern: &str) -> std::io::Result<Vec<std::path::PathBuf>> {
    let (dir, file_pattern) = if pattern.contains("**") {
        let parts: Vec<&str> = pattern.split("**").collect();
        let dir = if parts[0].is_empty() { "." } else { parts[0].trim_end_matches('/') };
        let rest = if parts.len() > 1 { parts[1].trim_start_matches('/') } else { "*" };
        (dir, rest)
    } else if let Some(slash) = pattern.rfind('/') {
        (&pattern[..slash], &pattern[slash+1..])
    } else {
        (".", pattern)
    };

    let mut results = Vec::new();
    collect_files(std::path::Path::new(dir), file_pattern, &mut results, 0)?;
    Ok(results)
}

fn collect_files(
    dir: &std::path::Path,
    pattern: &str,
    results: &mut Vec<std::path::PathBuf>,
    depth: usize,
) -> std::io::Result<()> {
    if depth > 20 { return Ok(()); }
    if !dir.is_dir() { return Ok(()); }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_files(&path, pattern, results, depth + 1)?;
            }
        } else if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if glob_match(pattern, name) || pattern == "*" {
                results.push(path);
            }
        }
    }
    Ok(())
}
