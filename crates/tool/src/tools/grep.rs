use async_trait::async_trait;
use miniagent_core::error::AgentError;
use regex::Regex;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

pub struct GrepTool;

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str { "grep" }
    fn description(&self) -> &str {
        "Search files for a regex pattern. Returns matching lines with context."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex pattern to search for"},
                "path": {"type": "string", "description": "File or directory to search"},
                "include": {"type": "string", "description": "File pattern to include (glob)"},
                "context_lines": {"type": "integer", "description": "Lines of context (default: 0)"}
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let pattern_str = input["pattern"].as_str()
            .ok_or_else(|| AgentError::tool("grep", "missing 'pattern'"))?;
        let path = input["path"].as_str().unwrap_or(".");
        let context_lines = input["context_lines"].as_u64().unwrap_or(0) as usize;

        let re = Regex::new(pattern_str)
            .map_err(|e| AgentError::tool("grep", format!("Invalid regex: {e}")))?;

        let mut results: Vec<String> = Vec::new();
        let mut match_count = 0;

        let target = std::path::Path::new(path);
        if target.is_file() {
            search_file(target, &re, context_lines, &mut results, &mut match_count);
        } else {
            let include = input["include"].as_str();
            walk_dir(target, include, &re, context_lines, &mut results, &mut match_count, 0)?;
        }

        if results.is_empty() {
            return Ok(ToolOutput {
                content: format!("No matches for '{pattern_str}'"),
                metadata: None,
            });
        }

        if results.len() > 500 {
            results.truncate(500);
            results.push(format!("... (truncated, {} total matches)", match_count));
        }

        Ok(ToolOutput {
            content: format!("{} matches:\n\n{}", match_count, results.join("\n")),
            metadata: None,
        })
    }
}

fn search_file(
    path: &std::path::Path,
    re: &Regex,
    context: usize,
    results: &mut Vec<String>,
    count: &mut usize,
) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if re.is_match(line) {
            *count += 1;
            let start = i.saturating_sub(context);
            let end = (i + context + 1).min(lines.len());
            let snippet: String = lines[start..end]
                .iter()
                .enumerate()
                .map(|(j, l)| {
                    let marker = if start + j == i { ">" } else { " " };
                    format!("{}:{} {} {}", path.display(), start + j + 1, marker, l)
                })
                .collect::<Vec<_>>()
                .join("\n");
            results.push(snippet);
            if results.len() >= 500 { break; }
        }
    }
}

fn walk_dir(
    dir: &std::path::Path,
    include: Option<&str>,
    re: &Regex,
    context: usize,
    results: &mut Vec<String>,
    count: &mut usize,
    depth: usize,
) -> Result<(), AgentError> {
    if depth > 10 || results.len() >= 500 { return Ok(()); }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            // skip hidden dirs and target
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            walk_dir(&path, include, re, context, results, count, depth + 1)?;
        } else if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(inc) = include
                && !crate::glob_util::glob_match(inc, name) { continue; }
            search_file(&path, re, context, results, count);
        }
    }
    Ok(())
}


