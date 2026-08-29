---
name: gwas-gene-prioritization
description: >
  Gene prioritization for disease hypotheses (biomni-style gene
  prioritization task): combine OpenTargets association scores, GWAS
  loci, and literature evidence into a transparent ranked shortlist.
triggers:
  - gene prioritization
  - target ranking
  - GWAS
  - risk gene
  - candidate gene
tools_needed:
  - opentargets
  - pubmed_search
version: "1.0.0"
priority: 8
---

# 疾病基因优先级排序协议

## 证据通道（每个通道透明打分，最终加权合成）

1. **人类遗传学**（权重 0.4）：OpenTargets `opentargets` 工具按 disease EFO id
   查关联基因 → `associationScore` 及分项 `geneticAssociation`（GWAS/L2E）。
2. **体细胞/表达证据**（0.2）：`differentialExpression`、`rnaExpression` 分项。
3. **已知药物可干预性**（0.2）：`knownDrug`、`safety` 分项 —— 对"可执行
   验证任务"是加分项（已有工具分子）。
4. **文献窗口**（0.2）：PubMed 近 5 年命中数（`pubmed_search`），log1p 归一。

## 协议

1. 解析任务的疾病名 → EFO id（OpenTargets `search` 查询，取第一个精确/前缀命中）。
2. 拉取 top 50 关联基因 + 分项分数。
3. 与本管线 KG/假说中的基因取交集：**交集基因是假说的独立外证**，
   在辩论/审核中按"人类遗传学支持"引用（附 URL）。
4. 输出 `prioritized_genes.csv`（symbol, total, genetic, expression, drug,
   pubmed_hits, in_hypothesis_kg）+ top10 条形图。
5. `RESULT = {"disease_efo": …, "n_genes": …, "top10": [...],
   "overlap_with_hypothesis": [...]}`。

## 陷阱

- EFO id 猜错 → 全表是无关疾病：先打印 search 命中的疾病名再往下走。
- 把 associationScore 当因果证据：它是加权汇总，引用时要带分项。
- 忽略负向证据：pGWAS 反向（保护性变异）也要标注。
