---
name: batch-correction
description: >
  Batch-effect detection and correction for bulk expression data: when to
  correct, ComBat via pycombat/harmonypy, and the classic mistake of
  correcting out the biology.
triggers:
  - batch effect
  - ComBat
  - Harmony
  - batch correction
  - confounding
tools_needed: []
version: "1.0.0"
priority: 8
---

# 批次效应检测与校正协议

## 先检测，再决定

1. PCA 前两个主成分按批次/组着色——批次解释的方差用
   `PC1/PC2 方差占比 × 组间 PC 均值距离` 粗估。
2. **混杂检查**：批次与疾病分组的列联表。完全混杂（每批只有一种组）
   时**数学上无法解耦**——必须直接写明"批次与分组完全混杂，任何批次
   校正都会移除生物学信号，仅做描述性分析"，不要硬算。
3. 校正：`pycombat`（pip 可装）或 `harmonypy`；两者都装不上时用
   **分位数归一 + 以批次为协变量的线性模型残差**（statsmodels OLS）。

## 正确姿势

```python
# 把组/协变量保留进模型，只移除批次方向
# ComBat: combat(data, batch_labels, covariates=cov_df, ...)
```

- 校正**只用于可视化和聚类**；下游差异表达建议在"批次作协变量"的
  线性模型里做（比先校正后检验更少失真）。
- 校正前后各画一次 PCA 图作为交付物。

## 陷阱

- 对已 log 归一的数据再 ComBat 不指定 parametric → 输出溢出。
- 把疾病组差异当批次校掉 → 假说分析得出"无差异"的假阴性。
- 单批单组数据（常见于合并多个 GSE）→ 如上第 2 点，直接声明局限。
