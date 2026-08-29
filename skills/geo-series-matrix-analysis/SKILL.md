---
name: geo-series-matrix-analysis
description: >
  End-to-end protocol for analysing NCBI GEO series-matrix TSV files as
  produced by miniagent's downloader: transposed expression matrix +
  ATTR_Sample_* metadata. Covers group parsing, transposition, and the
  three failure modes that break naive pandas code.
triggers:
  - geo
  - GSE
  - series matrix
  - series_matrix.tsv
  - expression dataset
  - public dataset
tools_needed:
  - geo_search
version: "1.0.0"
priority: 9
---

# GEO series-matrix 分析协议（miniagent 清洗格式）

## 数据布局

miniagent 下载并清洗后的 `{GSE}_series_matrix.tsv` 是 **一张表**：

1. 前若干行：`ATTR_Sample_*` 行 —— 每行一个属性，**列为样本**（`ATTR_Sample_title`、
   `ATTR_Sample_characteristics_ch1`、`ATTR_Sample_characteristics_ch1__1`…）。
2. 之后：表达矩阵，首列为 `ID_REF`（探针/基因），其余列为样本（GSM 编号）。
3. **样本是列，不是行** —— 与大多数 scRNA 处理代码的假设相反。

## 标准加载模板

```python
df = pd.read_csv(PATH, sep="\t", index_col=0)          # 表达矩阵 + ATTR 行混在一张表
expr = df[~df.index.astype(str).str.startswith("ATTR_")]
expr = expr.apply(pd.to_numeric, errors="coerce").dropna(how="all")
attrs = df[df.index.astype(str).str.startswith("ATTR_")].apply(
    lambda r: r.str.strip('"').str.strip())
assert expr.shape[1] == attrs.shape[1], "列错位：ATTR 行与矩阵列数不一致"
# 转置成"样本 x 基因"，后续 sklearn/statsmodels 都用这个方向
X = expr.T
X.index = X.index.astype(str).str.strip('"')
```

## 组标签解析（陷阱最多的一步）

- 疾病/分组几乎总在 `ATTR_Sample_characteristics_ch1*` 行里，值形如
  `"disease state: Alzheimer disease"` 或 `"disease: AD"`。
- 先枚举每个 characteristics 行的唯一值，再挑含预期关键词（disease/control/normal/ad…）
  的那一行做分组；不要硬编码行名。
- 样本列名（GSM…）与 `ATTR_Sample_geo_accession` 对齐；`ATTR_Sample_title`
  常带引号，务必 strip `" '` 再匹配。

## 三个必查的失败模式

1. **没有表达矩阵**：某些系列（scRNA/counts-only）矩阵只有 ATTR 行。
   加载后 `assert len(expr) > 0`，否则立即 raise ValueError 报
   "no expression table"——不要静默生成空结果。
2. **多平台系列**：每平台一张 `GSE-GPL…` 矩阵；若样本数远小于预期，说明只
   下到了一个平台子集，分析结论要限定该子集。
3. **探针非基因**：ID_REF 常是探针号；若任务要求基因水平结论，按平台注释
   映射或明确写"探针水平"结论。没有注释时用 `ATTR_Sample_supplementary_file`
   是可选路线，缺失则在报告中如实说明。

## 统计注意事项

- 芯片数据先 log2（值域 >100 时必做）；检查样本分布再选检验。
- 两个组的连续变量用 Welch t 检验 + Cohen's d；多组用 Kruskal-Wallis。
- 所有 p 值经 Benjamini-Hochberg 校正；报告效应量与 CI，不只报 p。
