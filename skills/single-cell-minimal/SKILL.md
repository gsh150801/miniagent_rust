---
name: single-cell-minimal
description: >
  Reality check and minimal viable protocol for single-cell requests:
  what is possible with a GEO series matrix (usually nothing real), when
  to fall back to pseudobulk / marker analysis, and the synthetic-data
  honesty rule.
triggers:
  - single cell
  - scRNA
  - scanpy
  - Seurat
  - UMAP
  - cell type
tools_needed: []
version: "1.0.0"
priority: 9
---

# 单细胞请求的现实检验与最小可行方案

## 第一步：确认数据形态

scRNA 真数据是 **10x matrix.mtx / h5 / h5ad**（GEO 的 supplementary 文件），
**不在 series matrix 里**。拿到 series matrix 后：

- `len(expr) == 0` 或样本数远小于细胞数预期 → 这不是单细胞矩阵。
- 此时**绝不要**装 scanpy 跑流程，更**不要生成合成单细胞数据冒充结果**。

## 退化路线（按优先级）

1. **伪bulk 差异分析**：把 series matrix 当 bulk 表达，按任务指定的
   标记基因集（如 microglia: AIF1/TMEM119/P2RY12/TREM2）打 module score
   （均值 z-score），按组比较 —— 这是对"细胞类型特异性"问题的合法近似，
   结论措辞为"bulk 水平的细胞类型签名差异"。
2. **标记基因相关性**：签名基因间的 Spearman 相关 + 组间差异。
3. 如果任务确需真单细胞 → raise ValueError 明说
   "this task needs raw scRNA files (h5/h5ad), not a series matrix"，
   让修复回路/报告如实记录数据不可得。

## 若真拿到 h5ad/mtx（readthedocs 级协议）

scanpy 流程：QC（`n_genes_by_counts`、`pct_counts_mt` 过滤）→ 归一 →
HVG → PCA → neighbors → Leiden → `rank_genes_groups`。R 包（Seurat）不
可用，等价物全部用 scanpy 实现。

## 诚实性红线（审计重点）

- 合成/模拟数据**只能**在变量名前缀 `synthetic_`、输出目录写
  `SYNTHETIC_DATA_NOTICE.txt`、且 `RESULT` JSON 里带
  `"data": "synthetic"` 的前提下用于**管线自测**；科研结论不得基于合成数据。
- 数据不可得时，正确交付物是"数据需求说明 + 可执行的分析脚本 +
  dry-run 标记"，而不是编造数值。
