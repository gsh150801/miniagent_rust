use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A complete skill bundle: metadata + prompt body + optional script.
/// Parsed from SKILL.md files with YAML-like frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillBundle {
    pub id: SkillId,
    pub metadata: SkillMetadata,
    pub body: String,
    pub examples: Vec<SkillExample>,
    pub dependencies: Vec<String>,
    pub file_path: String,
    pub loaded_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(pub Uuid);

impl Default for SkillId {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Unique skill name (kebab-case, e.g. "crispr-review")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Trigger phrases that suggest this skill is relevant
    pub triggers: Vec<String>,
    /// Tool names this skill needs access to
    pub tools_needed: Vec<String>,
    /// Skill version
    pub version: String,
    /// Author (optional)
    pub author: Option<String>,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// Priority: higher = preferred when multiple skills match
    pub priority: i32,
    /// Whether this skill can be invoked as a tool-call action
    pub actionable: bool,
}

impl Default for SkillMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            triggers: vec![],
            tools_needed: vec![],
            version: "0.1.0".into(),
            author: None,
            tags: vec![],
            priority: 0,
            actionable: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExample {
    pub input: String,
    pub expected_output: String,
}

/// Parse a SKILL.md file into a SkillBundle.
/// Format:
/// ```markdown
/// ---
/// name: crispr-review
/// description: Systematic review of CRISPR papers
/// triggers:
///   - review CRISPR
///   - CRISPR off-target
/// tools_needed:
///   - pubmed_search
///   - web_fetch
/// version: "0.1.0"
/// priority: 10
/// ---
///
/// # CRISPR Review Skill
///
/// You are a systematic review expert...
/// ```
pub fn parse_skill_file(path: &str, content: &str) -> Result<SkillBundle, String> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let meta = parse_frontmatter(&frontmatter)?;
    let examples = parse_examples(&body);

    Ok(SkillBundle {
        id: SkillId::new(),
        metadata: meta,
        body: body.trim().to_string(),
        examples,
        dependencies: vec![],
        file_path: path.to_string(),
        loaded_at: chrono::Utc::now(),
    })
}

fn split_frontmatter(content: &str) -> Result<(String, String), String> {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return Err("SKILL.md must start with '---' frontmatter block".into());
    }

    let after_first = &trimmed[3..];
    let end = after_first.find("\n---")
        .or_else(|| after_first.find("\r\n---"))
        .ok_or("Missing closing '---' in SKILL.md frontmatter")?;

    let frontmatter = after_first[..end].trim().to_string();
    let body = after_first[end + 4..].trim().to_string();
    Ok((frontmatter, body))
}

fn parse_frontmatter(yaml_like: &str) -> Result<SkillMetadata, String> {
    let mut meta = SkillMetadata::default();
    let mut current_array: Option<(&str, Vec<String>)> = None;

    for line in yaml_like.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Check if we're continuing an array
        if let Some(stripped) = trimmed.strip_prefix("- ") {
            if let Some((_field_name, ref mut values)) = current_array {
                let value = stripped.trim().trim_matches('"').to_string();
                values.push(value);
                continue;
            }
        } else {
            // Flush previous array
            if let Some((field_name, values)) = current_array.take() {
                set_meta_field(&mut meta, field_name, &serde_json::Value::Array(
                    values.into_iter().map(serde_json::Value::String).collect()
                ))?;
            }
        }

        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');

            if value.is_empty() {
                // Start of an array
                current_array = Some((key, Vec::new()));
            } else {
                set_meta_field(&mut meta, key, &serde_json::Value::String(value.to_string()))?;
            }
        }
    }

    // Flush last array
    if let Some((field_name, values)) = current_array {
        set_meta_field(&mut meta, field_name, &serde_json::Value::Array(
            values.into_iter().map(serde_json::Value::String).collect()
        ))?;
    }

    if meta.name.is_empty() {
        return Err("Skill 'name' is required in frontmatter".into());
    }

    Ok(meta)
}

fn set_meta_field(meta: &mut SkillMetadata, key: &str, value: &serde_json::Value) -> Result<(), String> {
    match key {
        "name" => meta.name = value.as_str().unwrap_or("").to_string(),
        "description" => meta.description = value.as_str().unwrap_or("").to_string(),
        "triggers" => {
            if let Some(arr) = value.as_array() {
                meta.triggers = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
            }
        }
        "tools_needed" => {
            if let Some(arr) = value.as_array() {
                meta.tools_needed = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
            }
        }
        "version" => meta.version = value.as_str().unwrap_or("0.1.0").to_string(),
        "author" => meta.author = Some(value.as_str().unwrap_or("").to_string()),
        "tags" => {
            if let Some(arr) = value.as_array() {
                meta.tags = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
            }
        }
        "priority" => {
            if let Some(s) = value.as_str() {
                meta.priority = s.parse().unwrap_or(0);
            }
        }
        "actionable" => {
            if let Some(s) = value.as_str() {
                meta.actionable = s.parse().unwrap_or(true);
            }
        }
        _ => {} // Ignore unknown fields
    }
    Ok(())
}

fn parse_examples(body: &str) -> Vec<SkillExample> {
    let mut examples = Vec::new();
    let mut in_example = false;
    let mut current_input = String::new();
    let mut current_output = String::new();

    for line in body.lines() {
        if line.trim() == "## Examples" {
            in_example = true;
            continue;
        }
        if !in_example { continue; }

        if line.trim().starts_with("### Input") {
            if !current_input.is_empty() {
                examples.push(SkillExample {
                    input: current_input.trim().to_string(),
                    expected_output: current_output.trim().to_string(),
                });
                current_input = String::new();
                current_output = String::new();
            }
        } else if line.trim().starts_with("### Output") {
            // Switch to output collection
        } else if line.trim().starts_with("### ") || line.trim().starts_with("```") {
            continue;
        } else if line.contains("Input:") {
            current_input = line.split_once("Input:").map(|x| x.1).unwrap_or("").trim().to_string();
        } else if line.contains("Expected:") {
            current_output = line.split_once("Expected:").map(|x| x.1).unwrap_or("").trim().to_string();
        } else if !line.trim().is_empty() {
            if current_output.is_empty() {
                current_input.push_str(line);
                current_input.push('\n');
            } else {
                current_output.push_str(line);
                current_output.push('\n');
            }
        }
    }

    if !current_input.is_empty() {
        examples.push(SkillExample {
            input: current_input.trim().to_string(),
            expected_output: current_output.trim().to_string(),
        });
    }

    examples
}
