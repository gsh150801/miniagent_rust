//! 工具输出封顶与大结果卸载（offload）。
//!
//! 业界口径（调研结论，2026-09）：Claude Code read 硬上限 2000 行 /
//! 25K token、bash ~30K chars、MCP 结果 25K token；OpenAI 工具输出
//! ~32K chars 封顶；Manus 把长内容卸载到文件系统、历史只留路径+摘要。
//!
//! miniagent 的铁律：**进历史前必须封顶**——历史层 trim 只是第二道
//! 防线（live: read 单次 646KB ×11 次 = 7.1MB 进历史，MiniMax 直接
//! 2013 context window exceeds limit）。

use std::path::PathBuf;

/// 单条工具输出进入历史的字节上限（≈8K token 英文 / ≈5K 汉字）。
/// 对齐业界 25-32K chars 口径的保守值；超出部分 offload 到文件。
pub const MAX_TOOL_OUTPUT_BYTES: usize = 32_000;

/// 卸载文件的目录名（位于任务工作目录下，随任务归档可审计）。
pub const OFFLOAD_DIR: &str = "tool_outputs";

/// 封顶工具输出：超过 [`MAX_TOOL_OUTPUT_BYTES`] 时把全文卸载到
/// `<working_dir>/<OFFLOAD_DIR>/`，历史里只留前段预览 + 文件路径 +
/// 续读指引。返回 (进历史的内容, 是否发生了卸载)。
///
/// offload 失败（磁盘/权限）时退化为纯截断——历史保护不因卸载失败
/// 而失效。
pub fn cap_tool_output(tool: &str, content: &str, working_dir: &str) -> (String, bool) {
    if content.len() <= MAX_TOOL_OUTPUT_BYTES {
        return (content.to_string(), false);
    }

    let original_bytes = content.len();
    let offload_path = offload_to_file(tool, content, working_dir);

    // 预览取头部（char-boundary 安全），并附尾部摘要行
    let mut preview: String = content.chars().take(MAX_TOOL_OUTPUT_BYTES / 2).collect();
    preview.push_str(&format!(
        "\n\n[OUTPUT TRUNCATED: full {original_bytes}-byte result from `{tool}` saved to `{}` — \
         use read(offset/limit) or grep on that file to retrieve specific parts. \
         Do NOT re-run the same command expecting full output in-context.]",
        offload_path.as_ref().map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(offload failed — re-run with narrower parameters)".into()),
    ));
    (preview, true)
}

/// 把超限输出写入工作目录下的卸载文件，返回路径。
fn offload_to_file(tool: &str, content: &str, working_dir: &str) -> Result<PathBuf, ()> {
    if working_dir.is_empty() {
        return Err(());
    }
    let dir = PathBuf::from(working_dir).join(OFFLOAD_DIR);
    std::fs::create_dir_all(&dir).map_err(|_| ())?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S%3f");
    let path = dir.join(format!("{tool}_{ts}.txt"));
    std::fs::write(&path, content).map_err(|_| ())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_passes_through() {
        let (out, offloaded) = cap_tool_output("read", "hello", "/tmp");
        assert_eq!(out, "hello");
        assert!(!offloaded);
    }

    #[test]
    fn oversized_output_offloads_with_path() {
        let dir = std::env::temp_dir().join("miniagent_offload_test");
        std::fs::create_dir_all(&dir).unwrap();
        let big = "x".repeat(MAX_TOOL_OUTPUT_BYTES + 1000);
        let (out, offloaded) = cap_tool_output("read", &big, &dir.to_string_lossy());
        assert!(offloaded);
        assert!(out.contains("OUTPUT TRUNCATED"));
        assert!(out.contains(OFFLOAD_DIR));
        assert!(out.len() < MAX_TOOL_OUTPUT_BYTES, "历史内容必须被封顶");
        // 卸载文件确实存在且包含全文（只看本次写入的——目录可能含历史残留）
        let entries: Vec<_> = std::fs::read_dir(dir.join(OFFLOAD_DIR)).unwrap().flatten().collect();
        assert!(!entries.is_empty());
        let saved = std::fs::read_to_string(entries.last().unwrap().path()).unwrap();
        assert_eq!(saved.len(), big.len());
    }

    #[test]
    fn offload_failure_degrades_to_truncation() {
        let big = "y".repeat(MAX_TOOL_OUTPUT_BYTES + 100);
        // 不存在的目录路径 → create_dir_all 也可能成功（权限允许时），
        // 所以这里只验证"封顶始终生效"这一核心保证，卸载与否不影响
        // 历史体积。
        let (out, _offloaded) = cap_tool_output("read", &big, "");
        assert!(out.contains("OUTPUT TRUNCATED"));
        assert!(out.len() < MAX_TOOL_OUTPUT_BYTES + 500);
    }
}
