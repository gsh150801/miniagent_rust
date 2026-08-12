//! Notebook 编辑工具（参考 cc-python-claude notebook_edit_tool.py）。
//!
//! 直接操作 .ipynb JSON 结构，无需 Jupyter 内核。支持：
//! - insert_cell：在指定位置插入 code/markdown cell
//! - replace_cell：替换指定 cell 内容
//! - delete_cell：删除指定 cell
//!
//! 自动创建符合 nbformat v4 的空 notebook，索引 clamp，cell 类型校验。

use async_trait::async_trait;
use miniagent_core::error::AgentError;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};
use crate::security::resolve_safe_path;
use tokio_util::sync::CancellationToken;

pub struct NotebookEditTool;

impl Default for NotebookEditTool { fn default() -> Self { Self } }
impl NotebookEditTool { pub fn new() -> Self { Self } }

#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &str { "notebook_edit" }

    fn description(&self) -> &str {
        "Edit a Jupyter notebook (.ipynb). Supports insert_cell, replace_cell, delete_cell. \
         Creates the notebook if it doesn't exist."
    }

    fn class(&self) -> ToolClass { ToolClass::Mutating }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .ipynb file" },
                "action": { "type": "string", "enum": ["insert_cell", "replace_cell", "delete_cell"] },
                "cell_index": { "type": "integer", "description": "Cell index (0-based)" },
                "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Cell type for insert/replace" },
                "source": { "type": "string", "description": "Cell source content for insert/replace" }
            },
            "required": ["path", "action"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let path_str = input["path"].as_str()
            .ok_or_else(|| AgentError::tool("notebook_edit", "missing 'path'"))?;
        let action = input["action"].as_str()
            .ok_or_else(|| AgentError::tool("notebook_edit", "missing 'action'"))?;

        // 安全校验
        let path = resolve_safe_path(path_str, std::path::Path::new(&ctx.working_dir))
            .map_err(|e| AgentError::tool("notebook_edit", e))?;

        // 加载或创建 notebook
        let mut nb = load_or_create(&path)?;

        // 先执行修改（结果文本），修改完释放 cells 借用，再序列化
        let result_text: Result<String, AgentError> = {
            let cells = nb["cells"].as_array_mut()
                .ok_or_else(|| AgentError::tool("notebook_edit", "invalid notebook: no cells array"))?;

            match action {
                "insert_cell" => {
                    let cell_type = input["cell_type"].as_str().unwrap_or("code");
                    let source = input["source"].as_str().unwrap_or("");
                    let mut idx = input["cell_index"].as_u64()
                        .unwrap_or(cells.len() as u64) as usize;
                    idx = idx.min(cells.len());
                    let new_cell = make_cell(cell_type, source);
                    cells.insert(idx, new_cell);
                    Ok(format!("Inserted {} cell at index {} (total: {})", cell_type, idx, cells.len()))
                }
                "replace_cell" => {
                    let idx = input["cell_index"].as_u64()
                        .ok_or_else(|| AgentError::tool("notebook_edit", "replace_cell requires 'cell_index'"))? as usize;
                    if idx >= cells.len() {
                        return Err(AgentError::tool("notebook_edit",
                            format!("cell_index {} out of range (0-{})", idx, cells.len().saturating_sub(1))));
                    }
                    let cell_type = input["cell_type"].as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            cells[idx]["cell_type"].as_str().unwrap_or("code").to_string()
                        });
                    let source = input["source"].as_str().unwrap_or("");
                    cells[idx] = make_cell(&cell_type, source);
                    Ok(format!("Replaced cell {} (type: {})", idx, cell_type))
                }
                "delete_cell" => {
                    let idx = input["cell_index"].as_u64()
                        .ok_or_else(|| AgentError::tool("notebook_edit", "delete_cell requires 'cell_index'"))? as usize;
                    if idx >= cells.len() {
                        return Err(AgentError::tool("notebook_edit",
                            format!("cell_index {} out of range (0-{})", idx, cells.len().saturating_sub(1))));
                    }
                    cells.remove(idx);
                    Ok(format!("Deleted cell {} (remaining: {})", idx, cells.len()))
                }
                _ => Err(AgentError::tool("notebook_edit", format!("unknown action: {action}"))),
            }
        };

        let result_text = result_text?;

        // 序列化保存（cells 借用已释放）
        let json = serde_json::to_string_pretty(&nb)
            .map_err(|e| AgentError::tool("notebook_edit", format!("serialize: {e}")))?;
        std::fs::write(&path, &json)
            .map_err(|e| AgentError::tool("notebook_edit", format!("write '{}': {e}", path.display())))?;

        Ok(ToolOutput {
            content: result_text,
            metadata: None,
        })
    }
}

/// 加载 .ipynb 文件，不存在则创建空 notebook（nbformat v4）。
fn load_or_create(path: &std::path::Path) -> Result<serde_json::Value, AgentError> {
    if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::tool("notebook_edit", format!("read '{}': {e}", path.display())))?;
        serde_json::from_str(&content)
            .map_err(|e| AgentError::tool("notebook_edit", format!("parse notebook: {e}")))
    } else {
        // 创建空 notebook
        Ok(serde_json::json!({
            "cells": [],
            "metadata": {
                "kernelspec": { "display_name": "Python 3", "language": "python", "name": "python3" },
                "language_info": { "name": "python", "version": "3.0.0" }
            },
            "nbformat": 4,
            "nbformat_minor": 5
        }))
    }
}

/// 构造一个 notebook cell（nbformat v4 格式）。
fn make_cell(cell_type: &str, source: &str) -> serde_json::Value {
    // source 按 \n 分割为数组（nbformat 要求 source 是字符串数组）
    let source_lines: Vec<&str> = source.lines().collect();
    let source_array: Vec<String> = source_lines.iter().enumerate()
        .map(|(i, line)| {
            if i < source_lines.len() - 1 {
                format!("{line}\n")
            } else {
                line.to_string()
            }
        })
        // 如果 source 以 \n 结尾，最后补一个空行
        .chain(if source.ends_with('\n') { vec![String::new()] } else { vec![] })
        .collect();

    match cell_type {
        "markdown" => serde_json::json!({
            "cell_type": "markdown",
            "metadata": {},
            "source": source_array
        }),
        _ => serde_json::json!({
            "cell_type": "code",
            "execution_count": serde_json::Value::Null,
            "metadata": {},
            "outputs": [],
            "source": source_array
        }),
    }
}
