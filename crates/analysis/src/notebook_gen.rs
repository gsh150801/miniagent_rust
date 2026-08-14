//! Build Jupyter notebooks (nbformat 4.5) for executed data analyses.
//!
//! The [`AnalysisRunner`](crate::runner::AnalysisRunner) generates a Python
//! script; this module turns that script into a structured `.ipynb` with a
//! markdown header (objective, dataset, cohort, variables, statistical method,
//! expected outcome, deliverable) and one or more code cells, so the analysis
//! is viewable and re-runnable in Jupyter. When Jupyter is available the
//! runner executes the notebook in place and the cell outputs (including
//! figures as base64) are embedded into the saved `.ipynb`.

use miniagent_core::error::AgentError;
use miniagent_hypothesis::{DataAnalysisTask, DatasetSource};
use serde_json::{json, Value};
use std::path::Path;

/// Maximum number of code cells; further top-level blocks are folded into the
/// last cell so very long scripts don't explode the notebook.
const MAX_CODE_CELLS: usize = 40;

/// Build an nbformat-4.5 notebook JSON value for a data-analysis task.
///
/// `hypothesis_ref` is included in the header so the notebook traces back to
/// the hypothesis it was generated to validate.
pub fn build_notebook(
    task: &DataAnalysisTask,
    hypothesis_ref: Option<uuid::Uuid>,
    code: &str,
) -> Value {
    let mut cells: Vec<Value> = Vec::new();

    cells.push(markdown_cell(&header_markdown(task, hypothesis_ref)));

    for block in split_code_into_cells(code) {
        cells.push(code_cell(&block));
    }
    // Guarantee at least one code cell even if the script body was empty.
    if cells.len() == 1 {
        cells.push(code_cell(""));
    }

    cells.push(markdown_cell(
        "## 结果 / Results\n\nOutputs (tables, figures, printed summaries) appear in the cells above once executed.",
    ));

    json!({
        "cells": cells,
        "metadata": {
            "kernelspec": {
                "display_name": "Python 3",
                "language": "python",
                "name": "python3"
            },
            "language_info": { "name": "python" }
        },
        "nbformat": 4,
        "nbformat_minor": 5
    })
}

/// Write a notebook JSON value to disk as pretty JSON.
pub fn write_notebook(nb: &Value, path: &Path) -> Result<(), AgentError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AgentError::Checkpoint(format!("create notebook dir: {e}")))?;
    }
    let pretty = serde_json::to_string_pretty(nb)
        .map_err(|e| AgentError::Checkpoint(format!("serialize notebook: {e}")))?;
    std::fs::write(path, pretty)
        .map_err(|e| AgentError::Checkpoint(format!("write notebook: {e}")))?;
    Ok(())
}

/// Split a Python script into code-cell-sized blocks.
///
/// A new cell begins at a **top-level** boundary: a blank line whose next
/// non-blank line starts at column 0 (no leading whitespace). This avoids
/// breaking indented blocks (function/class/loop bodies), whose internal blank
/// lines are always followed by indented lines. Empty/whitespace-only scripts
/// yield a single empty cell so the notebook stays structurally valid.
pub fn split_code_into_cells(code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    if lines.is_empty() {
        return vec![String::new()];
    }

    let mut cells: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    // `true` while we are inside an indented (nested) block.
    let mut in_block = false;

    for (i, line) in lines.iter().enumerate() {
        let is_blank = line.trim().is_empty();
        let indented = line.chars().next().map(|c| c.is_whitespace()).unwrap_or(false);

        // Update block state based on the current line.
        if !is_blank {
            in_block = indented;
        }

        let prev_blank = i > 0 && lines[i - 1].trim().is_empty();
        // Decide whether to start a fresh cell *before* appending this line.
        if prev_blank && !is_blank && !indented {
            // Top-level boundary: flush the previous cell (trim trailing blanks).
            if !current.is_empty() {
                while current.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                    current.pop();
                }
                if !current.is_empty() {
                    cells.push(current.join("\n"));
                }
                current.clear();
            }
        }

        current.push(line);

        // `in_block` is only consulted to avoid false splits; the boundary
        // condition above already requires `!indented`, so nested blank lines
        // never trigger a split. Keep the variable referenced for clarity.
        let _ = in_block;
    }

    if !current.is_empty() {
        while current.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            current.pop();
        }
        if !current.is_empty() {
            cells.push(current.join("\n"));
        }
    }

    if cells.is_empty() {
        cells.push(String::new());
    }

    // Fold excess cells into the last one to keep notebooks readable.
    while cells.len() > MAX_CODE_CELLS {
        let extra = cells.pop().unwrap();
        if let Some(last) = cells.last_mut() {
            last.push_str("\n\n");
            last.push_str(&extra);
        }
    }

    cells
}

// ───────────────────────────── cell builders ─────────────────────────────

fn markdown_cell(body: &str) -> Value {
    json!({
        "cell_type": "markdown",
        "id": cell_id(),
        "metadata": {},
        "source": lines_to_source(body)
    })
}

fn code_cell(body: &str) -> Value {
    json!({
        "cell_type": "code",
        "id": cell_id(),
        "execution_count": Value::Null,
        "metadata": {},
        "outputs": [],
        "source": lines_to_source(body)
    })
}

/// nbformat >= 4.5 requires a unique cell `id` (8-char alnum by convention).
fn cell_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cell-{n:04}")
}

/// Convert a string into nbformat `source` (array of lines, each keeping its
/// trailing `\n` except the last).
fn lines_to_source(text: &str) -> Vec<Value> {
    if text.is_empty() {
        return vec![Value::String(String::new())];
    }
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut acc = String::new();
    while let Some(c) = chars.next() {
        acc.push(c);
        if c == '\n' {
            out.push(Value::String(acc.clone()));
            acc.clear();
        }
    }
    if !acc.is_empty() {
        out.push(Value::String(acc));
    }
    if out.is_empty() {
        out.push(Value::String(String::new()));
    }
    out
}

fn header_markdown(task: &DataAnalysisTask, hypothesis_ref: Option<uuid::Uuid>) -> String {
    let source = match &task.dataset_source {
        DatasetSource::Geo => "GEO".to_string(),
        DatasetSource::Tcga => "TCGA".to_string(),
        DatasetSource::ArrayExpress => "ArrayExpress".to_string(),
        DatasetSource::Local(p) => format!("local ({p})"),
        DatasetSource::CustomUrl(u) => format!("URL ({u})"),
    };
    let accession = task
        .dataset_accession
        .as_deref()
        .unwrap_or("(unspecified)");
    let hyp = hypothesis_ref
        .map(|u| format!("`{u}`"))
        .unwrap_or_else(|| "_(none)_".into());

    format!(
        "# 数据分析任务：{id}

- **Objective:** {objective}
- **Hypothesis ref:** {hyp}
- **Dataset:** {source} — `{accession}`
- **Cohort / comparison:** {cohort}
- **Variables:**
  - independent: {ind}
  - dependent: {dep}
  - covariates: {cov}
- **Statistical method:** {method}
- **Expected outcome:** {expected}
- **Deliverable:** {deliverable}
",
        id = task.id,
        objective = task.objective,
        hyp = hyp,
        source = source,
        accession = accession,
        cohort = task.cohort_definition,
        ind = task.variables.independent.join(", "),
        dep = task.variables.dependent.join(", "),
        cov = task.variables.covariates.join(", "),
        method = task.statistical_method,
        expected = task.expected_outcome,
        deliverable = task.deliverable,
    )
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use miniagent_hypothesis::{AnalysisVariables, DatasetSource};

    fn task() -> DataAnalysisTask {
        DataAnalysisTask {
            id: "DA-1".into(),
            objective: "Measure BRCA1 differential expression".into(),
            dataset_source: DatasetSource::Geo,
            dataset_accession: Some("GSE12345".into()),
            cohort_definition: "tumor vs normal".into(),
            variables: AnalysisVariables {
                independent: vec!["BRCA1".into()],
                dependent: vec!["status".into()],
                covariates: vec!["age".into()],
            },
            statistical_method: "limma DE".into(),
            expected_outcome: "BRCA1 downregulated in tumor".into(),
            deliverable: "volcano + CSV".into(),
            priority: 0.9,
        }
    }

    #[test]
    fn build_notebook_has_valid_structure() {
        let nb = build_notebook(&task(), Some(uuid::Uuid::new_v4()), "import numpy as np\nprint('hi')\n");
        assert_eq!(nb["nbformat"], 4);
        assert_eq!(nb["nbformat_minor"], 5);
        let cells = nb["cells"].as_array().unwrap();
        // header markdown + >=1 code cell + results markdown
        assert!(cells.len() >= 3);
        assert_eq!(cells[0]["cell_type"], "markdown");
        assert_eq!(cells[cells.len() - 1]["cell_type"], "markdown");
        let code_cells: Vec<&Value> = cells.iter().filter(|c| c["cell_type"] == "code").collect();
        assert!(!code_cells.is_empty());
        // code cells carry execution_count=null and empty outputs.
        assert!(code_cells[0]["execution_count"].is_null());
        assert!(code_cells[0]["outputs"].as_array().unwrap().is_empty());
        // header carries task metadata.
        let header = source_to_string(&cells[0]["source"]);
        assert!(header.contains("DA-1"));
        assert!(header.contains("GSE12345"));
        assert!(header.contains("limma DE"));
    }

    #[test]
    fn split_does_not_break_indented_blocks() {
        let code = "import numpy as np\n\n\ndef f():\n    x = 1\n\n    y = 2\n\n    return x + y\n\nz = f()\n";
        let cells = split_code_into_cells(code);
        // Expect: [import], [def f() ... return], [z = f()]
        assert_eq!(cells.len(), 3, "cells: {cells:?}");
        assert!(cells[1].contains("def f():"));
        assert!(cells[1].contains("return x + y"), "indented body stays together: {cells:?}");
        assert!(cells[2].trim() == "z = f()");
    }

    #[test]
    fn split_single_block_returns_one_cell() {
        let cells = split_code_into_cells("x = 1\ny = 2\n");
        assert_eq!(cells.len(), 1);
    }

    #[test]
    fn split_empty_code_yields_one_empty_cell() {
        let cells = split_code_into_cells("");
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0], "");
    }

    #[test]
    fn write_notebook_roundtrips_as_json() {
        let dir = std::env::temp_dir().join("miniagent_notebook_gen_test");
        let path = dir.join("analysis.ipynb");
        let nb = build_notebook(&task(), None, "print(1)\n");
        write_notebook(&nb, &path).unwrap();
        let txt = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["nbformat"], 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn source_to_string(v: &Value) -> String {
        v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<String>())
            .unwrap_or_default()
    }
}
