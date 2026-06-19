use std::fs;
use std::path::Path;

use crate::bundle::{self, SkillBundle};

/// Scans a directory tree for SKILL.md files and parses them into SkillBundles.
pub struct SkillDiscovery {
    skill_dirs: Vec<String>,
}

impl SkillDiscovery {
    pub fn new() -> Self {
        Self {
            skill_dirs: vec![
                "skills".to_string(),
                ".miniagent/skills".to_string(),
            ],
        }
    }

    pub fn with_dir(mut self, dir: impl Into<String>) -> Self {
        self.skill_dirs.push(dir.into());
        self
    }

    /// Scan all configured directories for SKILL.md files.
    pub fn discover(&self) -> Vec<SkillBundle> {
        let mut bundles = Vec::new();

        for dir in &self.skill_dirs {
            let path = Path::new(dir);
            if !path.exists() || !path.is_dir() {
                continue;
            }

            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        // Look for SKILL.md inside each subdirectory
                        let skill_file = entry_path.join("SKILL.md");
                        if skill_file.exists()
                            && let Some(bundle) = self.load_skill_file(&skill_file) {
                                bundles.push(bundle);
                            }
                    } else if entry_path.is_file() {
                        let name = entry_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if (name.ends_with(".skill.md") || name == "SKILL.md")
                            && let Some(bundle) = self.load_skill_file(&entry_path) {
                                bundles.push(bundle);
                            }
                    }
                }
            }
        }

        bundles.sort_by_key(|b| std::cmp::Reverse(b.metadata.priority));
        bundles
    }

    fn load_skill_file(&self, path: &Path) -> Option<SkillBundle> {
        let content = fs::read_to_string(path).ok()?;
        let path_str = path.display().to_string();
        match bundle::parse_skill_file(&path_str, &content) {
            Ok(bundle) => {
                tracing::debug!("Discovered skill: {} ({})", bundle.metadata.name, path_str);
                Some(bundle)
            }
            Err(e) => {
                tracing::warn!("Failed to parse skill file {path_str}: {e}");
                None
            }
        }
    }

    /// Reload a specific skill file.
    pub fn reload(&self, path: &str) -> Option<SkillBundle> {
        self.load_skill_file(Path::new(path))
    }

    /// List all discovered skill files without parsing them.
    pub fn list_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for dir in &self.skill_dirs {
            let path = Path::new(dir);
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        let skill_file = entry_path.join("SKILL.md");
                        if skill_file.exists() {
                            files.push(skill_file.display().to_string());
                        }
                    }
                }
            }
        }
        files
    }
}

impl Default for SkillDiscovery {
    fn default() -> Self { Self::new() }
}
