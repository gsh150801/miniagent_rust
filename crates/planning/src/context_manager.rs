use std::path::{Path, PathBuf};

use crate::event_stream::EventStream;
use crate::todo_attention::TodoAttention;

/// ContextManager handles incremental context loading and compression.
///
/// Key principles (from Manus):
/// 1. File paths replace long content — "See: researcher/findings.json (2.3KB)"
/// 2. Errors are always preserved — they are learning opportunities
/// 3. KV-cache friendly: stable prompt prefix, append-only context
/// 4. Compression only drops content, never file references
/// 5. Structured variation prevents few-shot pattern traps
pub struct ContextManager {
    work_dir: PathBuf,
    max_context_chars: usize,
    max_recent_events: usize,
}

/// A context block that can be either full content or a file reference.
#[derive(Debug, Clone)]
pub enum ContextBlock {
    /// Full text content
    Content(String),
    /// Compressed: reference to a file
    FileRef {
        path: String,
        size_bytes: usize,
        summary: String,
    },
    /// Error that must be preserved
    Error {
        agent: String,
        message: String,
        timestamp: String,
    },
}

impl ContextManager {
    pub fn new(work_dir: &Path) -> Self {
        Self {
            work_dir: work_dir.to_path_buf(),
            max_context_chars: 48_000, // ~16K tokens budget for context
            max_recent_events: 15,
        }
    }

    pub fn with_max_context(mut self, max_chars: usize) -> Self {
        self.max_context_chars = max_chars;
        self
    }

    /// Build the full context string for a given role.
    /// Reads from filesystem + event stream + todo — never from in-memory state.
    pub fn build_context(
        &self,
        role: &str,
        todo: &mut TodoAttention,
        events: &EventStream,
    ) -> String {
        let mut context = String::new();

        // 1. Todo attention anchor (refreshed every call)
        context.push_str(&todo.refresh());
        context.push_str("\n\n");

        // 2. Role-specific context
        let role_context = self.load_role_context(role);
        context.push_str(&role_context);

        // 3. Recent events relevant to this role
        let event_text = events.format_recent(self.max_recent_events, Some(role));
        if !event_text.contains("no recent events") {
            context.push_str(&format!("## Recent Activity\n{event_text}\n\n"));
        }

        // 4. Error log (always preserved — Manus principle)
        let errors = self.load_error_log();
        if !errors.is_empty() {
            context.push_str(&format!("## Errors (preserve these)\n{errors}\n\n"));
        }

        // 5. Truncate if over budget (preserve structure)
        if context.len() > self.max_context_chars {
            context = self.compress_context(&context);
        }

        context
    }

    /// Load context for a specific role from filesystem.
    fn load_role_context(&self, role: &str) -> String {
        let mut context = String::new();
        let role_dir = self.work_dir.join(role);

        // Read own last output if the role directory exists
        if role_dir.exists()
            && let Some(output) = self.read_file_block(role, "last_output.json") {
                context.push_str(&format!("## Your Last Output\n{}\n\n", output));
            }

        // Read inputs from dependency roles (even if own directory doesn't exist yet)
        let deps = role_dependencies(role);
        for dep in deps {
            let dep_dir = self.work_dir.join(dep);
            if !dep_dir.exists() { continue; }

            context.push_str(&format!("## {dep}'s Output\n"));

            // Find the most relevant output file (prefer .md over .json)
            let preferred_order = [
                "findings.md", "critique.md", "synthesis.md", "draft.md",
                "evaluation.md", "report.md", "review.md", "context.md",
                "plan_v1.md", "output.md",
            ];
            let mut found = false;
            for name in &preferred_order {
                if dep_dir.join(name).exists() {
                    context.push_str(&self.file_ref_or_content(dep, name));
                    found = true;
                    break;
                }
            }
            if !found {
                // Try any .json file (prefer specific names over generic)
                let json_preferred = [
                    "findings.json", "critique.json", "synthesis.json",
                    "hypothesis.json", "verdict.json", "evaluation.json",
                    "review.json", "scores.json", "output.json", "last_output.json",
                    "plan.json", "current_plan.json",
                ];
                for name in &json_preferred {
                    if dep_dir.join(name).exists() {
                        context.push_str(&self.file_ref_or_content(dep, name));
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                context.push_str(&format!("(see {dep}/ directory for details)"));
            }
            context.push_str("\n\n");
        }

        context
    }

    /// Read a file and return either its content or a reference if too large.
    fn file_ref_or_content(&self, role: &str, filename: &str) -> String {
        let path = self.work_dir.join(role).join(filename);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let size = content.len();
                if size > 2000 {
                    // Compress: keep first 500 chars + reference
                    let preview: String = content.chars().take(500).collect();
                    format!("{preview}\n...({size} bytes total, see {role}/{filename})", )
                } else {
                    content
                }
            }
            Err(_) => format!("(could not read {role}/{filename})"),
        }
    }

    /// Read a file and return its content as a string block.
    fn read_file_block(&self, role: &str, filename: &str) -> Option<String> {
        let path = self.work_dir.join(role).join(filename);
        std::fs::read_to_string(&path).ok()
    }

    /// Load error log — errors are ALWAYS preserved (Manus principle).
    fn load_error_log(&self) -> String {
        let log_path = self.work_dir.join("errors.log");
        if let Ok(content) = std::fs::read_to_string(&log_path) {
            // Keep last 500 chars of errors
            if content.len() > 500 {
                let truncated: String = content.chars().rev().take(500).collect();
                return truncated.chars().rev().collect();
            }
            return content;
        }
        String::new()
    }

    /// Compress context by replacing long sections with summaries.
    fn compress_context(&self, context: &str) -> String {
        let mut compressed = String::new();
        let mut total_len = 0;

        for section in context.split("## ") {
            let section_text = if section.len() > 1000 {
                // Keep first 300 chars + "..."
                let preview: String = section.chars().take(300).collect();
                format!("{preview}\n...(compressed)\n")
            } else {
                section.to_string()
            };

            if total_len + section_text.len() > self.max_context_chars {
                break;
            }
            if !compressed.is_empty() {
                compressed.push_str("## ");
            }
            compressed.push_str(&section_text);
            total_len += section_text.len();
        }

        compressed
    }

    /// Append an error to the persistent error log.
    pub fn log_error(&self, agent: &str, error: &str) {
        use std::io::Write;
        let log_path = self.work_dir.join("errors.log");
        let ts = chrono::Utc::now().format("%H:%M:%S");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true).append(true).open(&log_path)
        {
            let _ = writeln!(f, "[{ts}] [{agent}] {error}");
        }
    }
}

/// Define role dependencies for context loading.
fn role_dependencies(role: &str) -> Vec<&'static str> {
    match role {
        "supervisor" => vec!["planner", "evaluator"],
        "planner" => vec!["supervisor"],
        "researcher" => vec!["planner"],
        "critic" => vec!["researcher"],
        "synthesizer" => vec!["researcher", "critic"],
        "executor" => vec!["planner"],
        "writer" => vec!["researcher", "critic", "synthesizer", "reviewer"],
        "reviewer" => vec!["researcher", "critic", "synthesizer", "writer"],
        "evaluator" => vec!["reviewer", "writer"],
        "observer" => vec![],
        "proposer" => vec!["researcher", "opponent", "judge"],
        "opponent" => vec!["proposer"],
        "judge" => vec!["proposer", "opponent"],
        _ => vec![],
    }
}
