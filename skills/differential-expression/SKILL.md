---
name: differential-expression
description: >
  Best-practice protocol for differential expression analysis in Python on
  bulk data: test selection, multiple testing, effect sizes, and the
  package traps (no DESeq2/limma in pure pip installs).
triggers:
  - differential expression
  - DE gene
  - DESeq2
  - limma
  - fold change
  - volcano
tools_needed: []
version: "1.0.0"
priority: 9
---

# 差异表达分析协议（Python 端）

## 检验选择

| 数据 | 方法 |
|---|---|
| 两组、近正态 | Welch t 检验（`stats.ttest_ind(equal_var=False)`） |
| 两组、非正态/小样本 | Mann-Whitney U（`stats.mannwhitneyu`） |
| 多组 | Kruskal-Wallis + Dunn 事后（或 ANOVA，检查残差） |
| 配对/纵向 | `statsmodels.MixedLM` 或 Wilcoxon signed-rank |
| 计数矩阵（raw counts） | 不用 DESeq2（R 专属）；做 log2(x+1) 后按上表处理 |

**微型环境注意**：没有 R。遇到任务写 "DESeq2/limma/edgeR" 时，在纯 Python
中实现等价物（log 变换 + Welch/秩检验 + BH 校正），并在输出中注明方法
替代。scipy/statsmodels 覆盖绝大多数情形。

## 流程

1. log2 变换（芯片数据或 CPM>0 的数据）。
2. 每个基因/特征做检验，收集 `log2FC`、`p`、方向。
3. BH 校正（`statsmodels.stats.multitest.multipletests(method="fdr_bh")`）。
4. 显著集定义：`p_adj < 0.05` 且 `|log2FC| >= 0.58`（约 1.5 倍）。
5. 效应量：Cohen's d 或秩相关；报告 CI。
6. 图：volcano（matplotlib，`fig.savefig`，`plt.close()`，不用 plt.show）。

## 输出契约

- `de_results.csv`：全基因表（含校正前后 p）。
- `sig_genes.csv`：显著集。
- 最后一行打印 `RESULT = {"n_sig_up": …, "n_sig_down": …, "top_genes": [...]}`。

## 常见错误

- 对未 log2 的跨 4 个数量级数据直接 t 检验 → 假阳性暴增。
- 把列当行（GEO 矩阵是"样本为列"，转置后才能按基因算）。
- 只报最小 p 值不报校正结果。
- 生成图片后不 `plt.close()` → 长跑中内存泄漏。
