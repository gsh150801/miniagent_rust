use miniagent_tool::registry::ToolRegistry;

/// Generate a comprehensive tool usage guide for any agent that has tool access.
/// This is the SINGLE source of truth for tool instructions in the entire pipeline.
pub fn tool_usage_guide(registry: &ToolRegistry) -> String {
    let defs = registry.get_definitions();
    let mut guide = String::from(
        "## Available Tools\n\
         You have the following tools at your disposal. USE THEM — do not simulate results.\n\n"
    );

    for def in &defs {
        guide.push_str(&format!("- **{}**: {}  \n", def.name, def.description));
    }

    guide
}

/// Get a role-filtered subset of tool names.
pub fn tools_for_role(role: &str) -> &'static [&'static str] {
    match role {
        "researcher" => &["web_search", "web_fetch", "pubmed_search", "patent_search", "clinical_trials_search", "read"],
        "explorer"   => &["web_search", "web_fetch", "pubmed_search", "patent_search", "clinical_trials_search"],
        "executor"   => &["bash", "read", "write", "edit", "glob", "grep", "git", "conda"],
        "writer"     => &["read", "write", "edit"],
        "critic"     => &["read", "web_search", "web_fetch"],
        "synthesizer"=> &["read"],
        "analyst"    => &["read", "grep", "glob"],
        _            => &["web_search", "web_fetch", "bash", "read", "write",
                          "edit", "glob", "grep", "pubmed_search",
                          "patent_search", "clinical_trials_search", "git", "conda"],
    }
}

/// Build a role-specific system prompt with tool guidance.
///
/// 参考 cc-python-claude 的分层提示词工程，system prompt 由以下段落组成：
/// 1. 角色定义（You are a {ROLE}）
/// 2. 任务执行原则（先读再改、不过度工程、安全优先）
/// 3. 工具使用偏好（专用工具优先于 Bash）
/// 4. 风险评估（可逆操作自由，不可逆操作确认）
/// 5. 输出效率（简洁直接，先结论后推理）
/// 6. 具体任务（task_desc + expected_output）
/// 7. 角色特定工具指南
/// 8. 关键规则（USE tools、不模拟、引用来源）
pub fn role_system_prompt(role: &str, task_desc: &str, expected_output: &str) -> String {
    let role_guide = match role {
        "researcher" =>
            "Use **web_search** and **web_fetch** to gather online information. \
             Use **pubmed_search** for scientific literature. \
             Use **patent_search** to search patents (Google Patents, USPTO). \
             Use **clinical_trials_search** to find clinical studies. \
             Use **read** to examine local files. \
             Cite all sources with URLs, PMIDs, or patent numbers.",
        "explorer" =>
            "Use **web_search** and **web_fetch** to research the task requirements. \
             Use **pubmed_search** if the task involves scientific topics. \
             Use **patent_search** and **clinical_trials_search** when the task involves \
             patents or clinical studies. \
             Gather real information to inform planning.",
        "executor" =>
            "Use **bash** to execute commands. \
             Use **read** to inspect files, **write** to create files, **edit** to modify files. \
             Use **glob** and **grep** to find and search code. \
             Use **git** for version control (clone, commit, push, pull, etc.). \
             Use **conda** to create/manage Python environments (use 'mn_' prefix for new envs). \
             Report the actual results of each action.",
        "writer" =>
            "Use **read** to review existing content. \
             Use **write** to produce polished output files. \
             Use **edit** to refine drafts.",
        "critic" =>
            "Use **read** to review outputs and research. \
             Use **web_search** and **web_fetch** to verify claims. \
             Focus on identifying gaps, overstatements, and unsupported claims.",
        "synthesizer" =>
            "Use **read** to gather all available outputs from previous tasks. \
             Integrate findings into a coherent whole.",
        "analyst" =>
            "Use **read** to examine data, **grep** to find patterns, \
             **glob** to discover relevant files. \
             Be thorough and precise.",
        _ =>
            "Use available tools (web_search, web_fetch, bash, read, write, \
             edit, glob, grep, pubmed_search, patent_search, \
             clinical_trials_search, git, conda) to complete your task.",
    };

    let env_info = format!(
        "{}{}",
        miniagent_core::context_info::env_block("."),
        miniagent_core::context_info::project_md_block(".").unwrap_or_default()
    );

    format!(
        r#"You are a {role_uppercase}.

## Task Execution Principles
 - **Read before modifying.** Do not propose changes to code or files you haven't read. Understand existing content before suggesting modifications.
 - **Don't over-engineer.** Don't add features, error handling, or abstractions beyond what the task requires. Do the minimum needed to complete the task well — no more, no less.
 - **Don't gold-plate.** A bug fix doesn't need surrounding code cleaned up. A simple feature doesn't need extra configurability. Don't add comments/docstrings to code you didn't change.
 - **Be careful with security.** Avoid command injection, XSS, SQL injection. If you notice insecure code, fix it immediately.
 - **If an approach fails, diagnose why before switching.** Read the error, check assumptions, try a focused fix. Don't retry blindly.

## Tool Usage Preferences
 - Use **read** instead of cat/head/tail to read files.
 - Use **edit** instead of sed/awk to modify files.
 - Use **write** instead of echo redirection to create files.
 - Use **glob** instead of find/ls to search for files.
 - Use **grep** instead of shell grep/rg to search file contents.
 - Reserve **bash** for system commands that require shell execution.
 - Call multiple independent tools in parallel for efficiency.

## Risk Assessment
 - Local, reversible actions (editing files, running tests) are fine to do freely.
 - For hard-to-reverse or destructive actions (deleting files, force push, dropping tables), verify before executing.
 - When uncertain, describe the action and its risks rather than executing blindly.

## Output Efficiency
 - Go straight to the point. Lead with the answer or result, not the reasoning.
 - Skip filler words, preamble, and unnecessary transitions.
 - If you can say it in one sentence, don't use three.
 - Focus output on: decisions, status updates, errors, and key findings.

{env_info}

## Your Task
{task_desc}

## Expected Output
{expected_output}

## Tool Usage
{role_guide}

## Critical Rules
1. **USE tools — do not simulate.** Always call a tool rather than fabricating results.
2. Report actual findings, not invented information.
3. If a tool returns an error, describe the error honestly — do not guess.
4. When citing sources, include URLs, PMIDs, or file paths.
5. Complete the task thoroughly before reporting."#,
        role_uppercase = capitalize_role(role),
        task_desc = task_desc,
        expected_output = expected_output,
        role_guide = role_guide,
        env_info = env_info,
    )
}

/// Generic instruction to append to any user prompt that has tool access.
///
/// 参考 cc-python-claude 的环境信息注入和 worker prompt 自包含原则。
pub fn tool_instruction_block() -> &'static str {
    r#"## Instructions
1. Use available tools to gather information and produce results.
2. DO NOT just describe what you plan to do — actually use the tools now.
3. Report your findings in a clear, structured format.
4. If you have already completed the task, summarize the findings.
5. Never fabricate tool output. If a tool call fails, report the error.
6. Your output should be self-contained — include all file paths, line numbers, and details needed to understand the result.
7. Record important information from tool results in your response, as original tool results may be cleared from context later."#
}

/// 生成环境信息段落（参考 cc-python-claude 的 compute_env_info）。
///
/// 让智能体了解运行环境：工作目录、平台、日期。帮助生成适合当前环境的命令和建议。
pub fn env_info_block(working_dir: &str) -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d");
    let platform = std::env::consts::OS;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".into());
    let shell_name = if shell.contains("zsh") { "zsh" }
        else if shell.contains("bash") { "bash" }
        else if shell.contains("fish") { "fish" }
        else { "sh" };
    let is_git = std::path::Path::new(working_dir).join(".git").exists();

    format!(
        "## Environment\n\
         - Working directory: {working_dir}\n\
         - Platform: {platform}\n\
         - Shell: {shell_name}\n\
         - Is a git repository: {is_git}\n\
         - Date: {today}"
    )
}

fn capitalize_role(role: &str) -> String {
    let mut chars = role.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capitalize() {
        assert_eq!(capitalize_role("researcher"), "Researcher");
        assert_eq!(capitalize_role("executor"), "Executor");
        assert_eq!(capitalize_role(""), "");
    }

    #[test]
    fn test_role_tools_inclusion() {
        for &role in &["researcher", "executor", "writer", "critic", "synthesizer", "analyst", "explorer"] {
            let tools = tools_for_role(role);
            assert!(!tools.is_empty(), "role {} should have tools", role);
        }
    }

    #[test]
    fn test_role_system_prompt_mentions_tools() {
        for &role in &["researcher", "executor", "writer", "critic", "synthesizer", "analyst", "explorer"] {
            let prompt = role_system_prompt(role, "test task", "test output");
            assert!(prompt.contains("Use"), "role {} prompt should mention tool usage", role);
            assert!(prompt.contains("test task"), "role {} prompt should contain task desc", role);
            assert!(prompt.contains("test output"), "role {} prompt should contain expected output", role);
        }
    }

    #[test]
    fn test_researcher_prompt_mentions_web_search() {
        let prompt = role_system_prompt("researcher", "Find papers on AI", "List of papers");
        assert!(prompt.contains("web_search"), "researcher prompt should mention web_search");
        assert!(prompt.contains("pubmed_search"), "researcher prompt should mention pubmed_search");
    }

    #[test]
    fn test_executor_prompt_mentions_bash() {
        let prompt = role_system_prompt("executor", "Run tests", "Test results");
        assert!(prompt.contains("bash"), "executor prompt should mention bash");
        assert!(prompt.contains("write"), "executor prompt should mention write");
        assert!(prompt.contains("edit"), "executor prompt should mention edit");
    }

    #[test]
    fn test_tool_instruction_block_has_use_tools() {
        let block = tool_instruction_block();
        assert!(block.contains("Use available tools"));
        assert!(block.contains("DO NOT just describe"));
    }

    #[test]
    fn test_explorer_tools() {
        let tools = tools_for_role("explorer");
        assert!(tools.contains(&"web_search"));
        assert!(tools.contains(&"web_fetch"));
        assert!(tools.contains(&"pubmed_search"));
    }

    #[test]
    fn test_all_roles_have_unique_tools() {
        // Each role should have at least one tool
        for &role in &["researcher", "executor", "writer", "critic", "synthesizer", "analyst", "explorer"] {
            let tools = tools_for_role(role);
            assert!(!tools.is_empty(), "{} has no tools", role);
        }
    }
}
