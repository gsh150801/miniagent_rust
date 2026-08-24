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

    /// Overall progress percentage.
    pub fn progress_pct(&self) -> f64 {
        if self.items.is_empty() { return 0.0; }
        let done = self.items.iter().filter(|i| i.status == TodoStatus::Completed).count();
        (done as f64 / self.items.len() as f64) * 100.0
    }

    /// Merge state changes from a sibling `TodoAttention` produced in a parallel
    /// branch back into this one.
    ///
    /// 并行分支各持有一份 `TodoAttention` 的 clone，分支内的 `complete`/`block`/`start`
    /// 修改不会自动回传主实例。本方法按 `id` 做并集合并：
    /// - 对共有的 item：若 other 的状态更"靠后"（Completed/Blocked/Skipped 优先于
    ///   InProgress/Pending），则采用 other 的状态（并行分支的完成/阻塞判定优先）。
    /// - 对 other 独有的 item（分支内 `add` 的新任务）：追加进来。
    ///
    /// 这样并行 wave 中各节点的 todo 进度都能反映到主实例上。
    pub fn merge_from(&mut self, other: &TodoAttention) {
        for other_item in &other.items {
            if let Some(mine) = self.items.iter_mut().find(|i| i.id == other_item.id) {
                // 采用"更靠后"的状态
                if status_rank(other_item.status) > status_rank(mine.status) {
                    mine.status = other_item.status;
                }
            } else {
                // 分支内新增的任务，追加（不重复 save_to_disk，最后统一 refresh）
                self.items.push(other_item.clone());
            }
        }
        // 保持优先级排序与上限裁剪，与 add() 一致
        self.items.sort_by_key(|b| std::cmp::Reverse(b.priority));
        if self.items.len() > self.max_items {
            let pending_count = self.items.iter().filter(|i| i.status == TodoStatus::Pending).count();
            if pending_count > self.max_items / 2 {
                self.items.retain(|i| i.status != TodoStatus::Pending || i.priority >= 5);
            }
        }
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

/// 给 `TodoStatus` 一个单调"进度"序，用于 `merge_from` 判定哪个状态更靠后。
/// Pending < InProgress < Blocked < Skipped < Completed（终态优先）。
fn status_rank(s: TodoStatus) -> u8 {
    match s {
        TodoStatus::Pending => 0,
        TodoStatus::InProgress => 1,
        TodoStatus::Blocked => 2,
        TodoStatus::Skipped => 3,
        TodoStatus::Completed => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_todo(id: &str) -> TodoAttention {
        let dir = std::env::temp_dir().join(format!("miniagent_todo_test_{id}"));
        // 清理可能的残留，保证测试独立
        std::fs::remove_file(dir.join("todo.json")).ok();
        TodoAttention::new(&dir)
    }

    #[test]
    fn merge_from_adopts_more_advanced_status() {
        let mut main = tmp_todo("merge_main");
        main.add("task A", None, 5);
        main.add("task B", None, 5);

        // 分支 clone：完成 task A，标记 task B 进行中
        let mut branch = main.clone();
        let a_id = main.items[0].id.clone();
        let b_id = main.items[1].id.clone();
        branch.complete(&a_id);
        branch.start(&b_id);

        main.merge_from(&branch);

        let a = main.items.iter().find(|i| i.id == a_id).unwrap();
        let b = main.items.iter().find(|i| i.id == b_id).unwrap();
        assert_eq!(a.status, TodoStatus::Completed, "merge should adopt branch's completed status");
        assert_eq!(b.status, TodoStatus::InProgress, "merge should adopt branch's in-progress status");
    }

    #[test]
    fn merge_from_appends_new_items_from_branch() {
        let mut main = tmp_todo("merge_append_main");
        main.add("task A", None, 5);

        let mut branch = main.clone();
        branch.add("branch-only task", None, 3); // 分支内新增

        main.merge_from(&branch);

        let descs: Vec<&str> = main.items.iter().map(|i| i.description.as_str()).collect();
        assert!(descs.contains(&"branch-only task"), "merge should append branch-only items");
    }

    #[test]
    fn merge_from_does_not_downgrade_status() {
        let mut main = tmp_todo("merge_nodowngrade");
        main.add("task A", None, 5);
        let a_id = main.items[0].id.clone();
        main.complete(&a_id); // main 已完成

        // 分支是旧的 Pending 状态
        let mut branch = tmp_todo("merge_nodowngrade_branch");
        branch.add("task A", None, 5); // 不同实例，但同 id（t1）→ 模拟分支未推进

        main.merge_from(&branch);

        let a = main.items.iter().find(|i| i.id == a_id).unwrap();
        assert_eq!(a.status, TodoStatus::Completed, "merge must not downgrade a completed item");
    }
}
