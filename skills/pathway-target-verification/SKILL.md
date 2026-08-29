---
name: pathway-target-verification
description: >
  Verify disease-gene/pathway hypotheses against structured public
  resources (OpenTargets, UniProt, PubMed) instead of parametric memory:
  the evidence-lookup protocol used by the debate and report-review stages.
triggers:
  - verify target
  - evidence check
  - disease gene association
  - drug target
  - protein function
tools_needed:
  - opentargets
  - uniprot
  - pubmed_search
version: "1.0.0"
priority: 9
---

# 假说靶点/通路证据核查协议（辩论与审核共用）

原则：**结构化数据库 > 检索片段 > 参数记忆**。断言一条
"基因 X 与疾病 Y 有关联/通路 P 参与 Z" 之前，至少走一遍下面的查询并把
证据 URL 写进输出。

## OpenTargets：基因-疾病关联与评分分解

```
opentargets(query={"operation":"search","query":"alzheimer"})       # 拿 EFO id
opentargets(query={"operation":"associated_targets","efo_id":"EFO_0000249","limit":30})
```

- 报告 `associationScore` + `geneticAssociation` 分项（GWAS 证据有
  studyId，可回溯）。
- 与假说方向一致性检查：遗传学证据支持"功能丧失致病"还是"功能获得致病"。

## UniProt：蛋白功能/定位/翻译后修饰

```
uniprot(query={"gene":"ZBP1","fields":"function,subcellular_location,ptm"})
```

- 用于核实酶活性域、细胞定位是否与机制故事自洽（例：假设"胞质内
  RNA 感受器"，但 UniProt 显示主要定位于核 → 机制要限定条件）。

## 输出规范（审计要求）

每条结论输出：`{claim, sources: [{db, id, url, field}], verdict:
supported | contradicted | not_found, detail}` —— 这正是
`report_review.json` 引用的格式。

## 红线

- 数据库查询失败时标注 `verdict: "not_found"` 并写明失败原因，不许
  回退到记忆填充。
- 同一事实两个来源冲突时，两条都保留并标 `detail`，不擅自取舍。
