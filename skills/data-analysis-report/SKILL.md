---
name: data-analysis-report
description: >
  Data-analysis report format: dataset provenance, methods, statistical
  results with effect sizes and CIs, figure/table references, and honest
  limitation notes. Use for 数据分析报告/统计结果报告. Pairs with the
  analysis runner's outputs (CSV/PNG/provenance).
triggers:
  - 数据分析报告
  - analysis report
  - statistical report
  - 结果报告
follow_ups:
  - bioinf-verify-report
tools_needed: []
version: "1.0.0"
priority: 9
---

# 数据分析报告格式

## 结构（固定章节）

1. **数据来源**：数据集（GSE/TCGA/文件名）、样本量、分组定义、获取日期。
   引用数据集 accession（如 `GSE157827`），不得凭空编造样本数。
2. **方法**：统计方法与软件版本（如 `scipy 1.x Welch t-test`）；
   显著性阈值（如 `BH FDR < 0.05`）；协变量。
3. **结果**：
   - 每个结果一句结论 + 关键统计量：`组间差异显著（t=3.2, df=28,
     p=0.003, Cohen's d=1.1, 95%CI [0.4,1.8]）`。
   - 表格/图以相对路径引用（如 `figures/volcano.png`、`tables/de.csv`），
     且文件必须真实存在于输出目录。
4. **结论与局限**：结论严格限于数据支持的范围内；局限（样本量、混杂、
   批次）如实列出。
5. **复现**：脚本文本 + 随机种子 + 运行命令。

## 硬性规则

- 所有 p 值必须同时给出效应量；只报 p 不报效应量为格式错误。
- 数字必须来自实际运行输出（RESULT JSON / CSV），禁止编造。
- 文件引用必须是真实存在的文件（核验时会检查路径存在性）。
