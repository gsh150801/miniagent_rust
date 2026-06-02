use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// TodoAttention manages the `todo.md` file as an attention anchor.
///
/// Inspired by Manus's context engineering:
/// - Rewrites the task list at the end of every context window
/// - Prevents "lost-in-the-middle" drift in long tasks
/// - Survives restarts via disk persistence
/// - Bounded size to prevent context bloat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoAttention {
    items: Vec<TodoItem>,
    work_dir: PathBuf,
    max_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub description: String,
    pub status: TodoStatus,
    pub priority: u8,
    pub assigned_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Skipped,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "PENDING"),
            TodoStatus::InProgress => write!(f, "IN_PROGRESS"),
            TodoStatus::Completed => write!(f, "COMPLETED"),
            TodoStatus::Blocked => write!(f, "BLOCKED"),
            TodoStatus::Skipped => write!(f, "SKIPPED"),
        }
    }
}

impl TodoAttention {
    pub fn new(work_dir: &Path) -> Self {
        let mut todo = Self {
            items: Vec::new(),
            work_dir: work_dir.to_path_buf(),
            max_items: 20,
        };
        todo.load_from_disk();
        todo
    }

    pub fn with_max_items(mut self, max: usize) -> Self {
        self.max_items = max;
        self
    }

    /// Add a new task item.
    pub fn add(&mut self, description: impl Into<String>, agent: Option<&str>, priority: u8) -> &TodoItem {
        let id = format!("t{}", self.items.len() + 1);
        self.items.push(TodoItem {
            id,
            description: description.into(),
            status: TodoStatus::Pending,
            priority,
            assigned_agent: agent.map(|a| a.to_string()),
        });
        // Sort by priority (highest first)
        self.items.sort_by_key(|b| std::cmp::Reverse(b.priority));
        // Trim if over max
        if self.items.len() > self.max_items {
            // Keep completed items for reference, trim oldest pending
            let pending_count = self.items.iter().filter(|i| i.status == TodoStatus::Pending).count();
            if pending_count > self.max_items / 2 {
                self.items.retain(|i| i.status != TodoStatus::Pending || i.priority >= 5);
            }
        }
        self.save_to_disk();
        self.items.last().unwrap()
    }

    /// Mark an item as in progress.
    pub fn start(&mut self, id: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = TodoStatus::InProgress;
        }
        self.save_to_disk();
    }

    /// Mark an item as completed.
    pub fn complete(&mut self, id: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = TodoStatus::Completed;
        }
        self.save_to_disk();
    }

    /// Mark an item as blocked.
    pub fn block(&mut self, id: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = TodoStatus::Blocked;
        }
        self.save_to_disk();
    }

    /// Refresh: rewrite the todo.md file (called every iteration).
    /// Returns the formatted todo text for inclusion in prompts.
    /// This is the core "attention anchor" mechanism.
    pub fn refresh(&mut self) -> String {
        let text = self.format_todo();
        self.save_to_disk();
        text
    }

    /// Format the current todo list as Markdown.
    pub fn format_todo(&self) -> String {
        let mut md = String::from("# Current Objectives\n\n");

        let active: Vec<&TodoItem> = self.items.iter()
            .filter(|i| matches!(i.status, TodoStatus::Pending | TodoStatus::InProgress))
            .collect();
        let completed: Vec<&TodoItem> = self.items.iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .collect();

        if active.is_empty() && completed.is_empty() {
            md.push_str("(no active tasks)\n");
            return md;
        }

        if !active.is_empty() {
            md.push_str("## Active\n");
            for item in &active {
                let check = match item.status {
                    TodoStatus::InProgress => "[>]",
                    _ => "[ ]",
                };
                let agent = item.assigned_agent.as_deref().unwrap_or("unassigned");
                md.push_str(&format!(
                    "- {check} **{}** (p{}, @{}) — {}\n",
                    item.id, item.priority, agent, item.description
                ));
            }
            md.push('\n');
        }

        // Only show last 5 completed items
        if !completed.is_empty() {
            md.push_str("## Completed\n");
            for item in completed.iter().rev().take(5) {
                md.push_str(&format!("- [x] {} — {}\n", item.id, item.description));
            }
            if completed.len() > 5 {
                md.push_str(&format!("  ... and {} more completed\n", completed.len() - 5));
            }
            md.push('\n');
        }

        let done = self.items.iter().filter(|i| i.status == TodoStatus::Completed).count();
        md.push_str(&format!("**Progress: {}/{} tasks done**\n", done, self.items.len()));

        md
    }

    /// Get pending items.
    pub fn pending(&self) -> Vec<&TodoItem> {
        self.items.iter().filter(|i| i.status == TodoStatus::Pending).collect()
    }

    /// Get items assigned to a specific agent.
    pub fn for_agent(&self, agent: &str) -> Vec<&TodoItem> {
        self.items.iter()
            .filter(|i| i.assigned_agent.as_deref() == Some(agent))
            .collect()
    }

    /// Overall progress percentage.
    pub fn progress_pct(&self) -> f64 {
        if self.items.is_empty() { return 0.0; }
        let done = self.items.iter().filter(|i| i.status == TodoStatus::Completed).count();
        (done as f64 / self.items.len() as f64) * 100.0
    }

    fn save_to_disk(&self) {
        let dir = &self.work_dir;
        std::fs::create_dir_all(dir).ok();

        // Save markdown version (for prompts)
        let md = self.format_todo();
        std::fs::write(dir.join("todo.md"), &md).ok();

        // Save structured JSON (for programmatic access)
        let json = serde_json::to_string_pretty(&self.items).unwrap_or_default();
        std::fs::write(dir.join("todo.json"), &json).ok();
    }

    fn load_from_disk(&mut self) {
        let json_path = self.work_dir.join("todo.json");
        if let Ok(content) = std::fs::read_to_string(&json_path)
            && let Ok(items) = serde_json::from_str::<Vec<TodoItem>>(&content) {
                self.items = items;
            }
    }
}
