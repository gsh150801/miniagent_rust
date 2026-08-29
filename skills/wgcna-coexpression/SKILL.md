---
name: wgcna-coexpression
description: >
  Co-expression module analysis (WGCNA) in Python: pywgcna or a transparent
  hand-rolled fallback (correlation → adjacency → hierarchical clustering →
  dynamic tree cut), module-trait correlation, and package traps.
triggers:
  - WGCNA
  - co-expression
  - module
  - hub gene
tools_needed: []
version: "1.0.0"
priority: 7
---

# 共表达模块分析协议（WGCNA 的 Python 实现）

## 包现实

R 的 WGCNA 不可用。优先 `pywgcna`（pip）；装不上时**手写透明版**：

```python
corr = np.corrcoef(X.T)                       # 基因 x 基因
soft = np.abs(corr) ** beta                   # soft threshold，beta 取
                                              # scale-free 拟合最好的 3..20
adj = soft
dist = 1 - adj
link = scipy.cluster.hierarchy.linkage(
    scipy.spatial.distance.squareform(((dist + dist.T) / 2)[np.triu_indices_from(dist, 1)], checks=False),
    method="average")
modules = scipy.cluster.hierarchy.fcluster(link, t=cut_height, criterion="distance")
```

## 模块-性状关联

- 模块特征向量 = 模块内基因表达的第一主成分（PCA）。
- `spearmanr(ME, trait)` 逐性状；报告 r、p、模块大小、top hub（模块内
  kME 最高 5 个基因）。
- 目标基因（如 NLRP3/IL1B）所在模块要单独点名。

## 输出契约

`modules.csv`（gene, module, kME）、`module_trait.csv`、热力图 PNG、
`RESULT = {"n_modules": …, "target_module_size": …}`。

## 陷阱

- 样本数 <15 时共表达矩阵噪声主导 → 换成"目标基因与全基因组的相关排名"
  并明说样本不足。
- 忘记转置（必须基因 x 样本再 `np.corrcoef`，X 是样本 x 基因时要 `X.T`）。
- beta 过大导致邻接矩阵全零 → 打印每档 beta 的连接密度，选非退化的。
