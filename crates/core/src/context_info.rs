//! 统一上下文信息注入（参考 cc-python-claude compute_env_info + build_system_prompt）。
//!
//! 所有智能体的 prompt 都应注入以下信息，让模型了解运行环境：
//! 1. 当前日期和时间（关键：让模型理解"今年"、"最近"等相对时间词）
//! 2. 工作目录、平台、shell（帮助生成正确的命令）
//! 3. 可用工具提示（引导模型使用工具而非猜测）
//!
//! 使用方式：
//! ```ignore
//! let system = format!("You are an AI agent.\n{}", context_info::env_block(&working_dir));
//! ```

/// 生成完整的环境信息段落（注入 system prompt 末尾）。
///
/// 参考 cc-python-claude 的 `compute_env_info`：
/// - 当前日期（UTC + 本地时区）
/// - 工作目录 + git 仓库状态
/// - 平台 + shell
/// - 语言提示（"用与用户相同的语言回答"）
pub fn env_block(working_dir: &str) -> String {
    let now = chrono::Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let year = now.format("%Y").to_string();
    let platform = std::env::consts::OS;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".into());
    let shell_name = if shell.contains("zsh") { "zsh" }
        else if shell.contains("bash") { "bash" }
        else if shell.contains("fish") { "fish" }
        else { "sh" };
    let is_git = std::path::Path::new(working_dir).join(".git").exists();

    format!(
        "## Environment\n\
         - Current date: {today} (UTC). The current year is {year}.\n\
         - When the user says \"this year\", \"today\", \"recent\", or \"latest\", \
           they mean {year}, not a past year.\n\
         - Working directory: {working_dir}\n\
         - Platform: {platform} | Shell: {shell_name} | Git repo: {is_git}\n\
         - Respond in the same language the user uses."
    )
}

/// 生成简短的日期提示（注入需要轻量日期感知的 prompt，如 evaluator/validator）。
///
/// 比 `env_block` 更短，适合不需要完整环境信息的辅助角色。
pub fn date_hint() -> String {
    let now = chrono::Utc::now();
    let year = now.format("%Y").to_string();
    let today = now.format("%Y-%m-%d").to_string();
    format!("Today is {today} (UTC). Current year: {year}.")
}

/// 生成用户上下文段落（注入 user prompt 开头）。
///
/// 包含原始用户输入 + 环境信息，让 LLM 有完整上下文。
pub fn user_context_block(user_input: &str, working_dir: &str) -> String {
    format!(
        "{env}\n\n\
         ## User Request\n{user_input}",
        env = env_block(working_dir),
        user_input = user_input,
    )
}

// ── project.md 项目级指令加载（参考 cc-python-claude claudemd.py）─────────
//
// 用户在工作目录放一个 project.md，Agent 自动读取并注入到 system prompt。
// 这比隐藏文件（.miniagent.md）更直观——用户能直接在 IDE 中看到。
//
// 搜索路径（从低到高优先级，后者覆盖前者）：
// 1. 从工作目录向上遍历到根目录：每层的 project.md
// 2. 工作目录下的 project.local.md（私有本地指令，不提交版本控制）
//
// 支持 @path 语法引用其他文件（有循环引用检测）。

/// 加载并合并项目级指令文件（project.md）。
///
/// 从工作目录向上遍历，合并所有找到的 project.md 内容。
/// 越靠近工作目录的文件优先级越高（排在拼接结果末尾）。
/// 返回合并后的文本，或 None（无文件）。
pub fn load_project_md(working_dir: &str) -> Option<String> {
    let mut contents: Vec<String> = Vec::new();
    let cwd = std::path::Path::new(working_dir);

    // 从 cwd 向上遍历到根目录，收集所有 project.md
    let mut ancestors: Vec<std::path::PathBuf> = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        ancestors.push(current.clone());
        let parent = current.parent();
        match parent {
            Some(p) if p != current => current = p.to_path_buf(),
            _ => break,
        }
    }

    // 从根到 cwd 的顺序处理（reversed），确保 cwd 的 project.md 最后加载
    for ancestor in ancestors.iter().rev() {
        let project_md = ancestor.join("project.md");
        if project_md.is_file()
            && let Ok(text) = std::fs::read_to_string(&project_md) {
                let expanded = expand_includes(&text, project_md.parent().unwrap_or(ancestor), &mut std::collections::HashSet::new());
                if !expanded.is_empty() {
                    contents.push(expanded);
                }
            }
    }

    // cwd 下的 project.local.md（最高优先级）
    let local_md = cwd.join("project.local.md");
    if local_md.is_file()
        && let Ok(text) = std::fs::read_to_string(&local_md) {
            let expanded = expand_includes(&text, cwd, &mut std::collections::HashSet::new());
            if !expanded.is_empty() {
                contents.push(expanded);
            }
        }

    if contents.is_empty() {
        None
    } else {
        Some(contents.join("\n\n"))
    }
}

/// 展开 @path 包含指令（参考 cc-python-claude _read_and_expand）。
fn expand_includes(
    text: &str,
    base_dir: &std::path::Path,
    seen: &mut std::collections::HashSet<std::path::PathBuf>,
) -> String {
    // 移除 HTML 块注释
    let text = remove_html_comments(text);

    let mut result = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') && !trimmed.starts_with("@@") {
            let include_path_str = trimmed[1..].trim();
            let include_path = if include_path_str.starts_with('/') {
                std::path::PathBuf::from(include_path_str)
            } else {
                base_dir.join(include_path_str)
            };

            let resolved = match include_path.canonicalize() {
                Ok(p) => p,
                Err(_) => { result.push_str(line); result.push('\n'); continue; }
            };

            // 循环引用检测
            if seen.contains(&resolved) {
                result.push_str(line); result.push('\n'); continue;
            }
            seen.insert(resolved.clone());

            if let Ok(include_text) = std::fs::read_to_string(&resolved) {
                let expanded = expand_includes(
                    &include_text,
                    resolved.parent().unwrap_or(base_dir),
                    seen,
                );
                result.push_str(&expanded);
                result.push('\n');
            } else {
                result.push_str(line); result.push('\n');
            }
        } else {
            result.push_str(line); result.push('\n');
        }
    }

    result.trim().to_string()
}

/// 移除 HTML 块注释（<!-- ... -->）。
fn remove_html_comments(text: &str) -> String {
    let mut result = String::new();
    let mut in_comment = false;
    for line in text.lines() {
        if line.contains("<!--") { in_comment = true; }
        if !in_comment { result.push_str(line); result.push('\n'); }
        if line.contains("-->") { in_comment = false; }
    }
    result.trim().to_string()
}

/// 生成 project.md 注入段落（如果有内容）。
///
/// 注入到 system prompt 末尾，优先级最高（覆盖前面的指令）。
pub fn project_md_block(working_dir: &str) -> Option<String> {
    load_project_md(working_dir).map(|content| {
        format!(
            "## Project Instructions\n\
             The following project-level instructions override defaults. Adhere to them.\n\n\
             {content}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_block_contains_date() {
        let block = env_block("/tmp");
        assert!(block.contains("Current date:"), "should contain date");
        assert!(block.contains("current year is"), "should mention current year");
        assert!(block.contains("\"this year\""), "should have 'this year' hint");
    }

    #[test]
    fn env_block_contains_year() {
        let block = env_block("/tmp");
        let year = chrono::Utc::now().format("%Y").to_string();
        assert!(block.contains(&year), "should contain current year {year}");
    }

    #[test]
    fn date_hint_is_short() {
        let hint = date_hint();
        assert!(hint.contains("Today is"));
        assert!(hint.len() < 100, "date_hint should be short, got {} chars", hint.len());
    }

    #[test]
    fn user_context_block_combines_env_and_input() {
        let block = user_context_block("hello world", "/tmp");
        assert!(block.contains("Environment"));
        assert!(block.contains("hello world"));
        assert!(block.contains("/tmp"));
    }
}
