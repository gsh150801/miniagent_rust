---
name: cited-review-report
description: >
  Biomedical literature-review report format with verifiable citations:
  in-text [index] markers plus a References section with complete,
  machine-checkable entries (PMID/DOI/URL). Use for 综述分析/文献综述/
  research survey reports. Pairs with the citation_check tool.
triggers:
  - 综述
  - literature review
  - survey report
  - review report
  - 文献综述报告
tools_needed:
  - citation_check
  - pubmed_search
version: "1.0.0"
priority: 10
---

# 综述分析报告格式（可核验引用版）

## 硬性格式要求

1. **正文引用**：每个事实性陈述（数据、结论、机制、年份）必须紧跟
   `[index]` 标记，index 对应文末 References 的序号。
   示例：`1988 年的一篇文献指出 IL-8 是重要的炎症介质[12]。`
   - 同一支撑文献多处使用同一序号；多个来源用 `[1,3]` 或 `[4-6]`。
   - **禁止**无标记的事实性陈述——没有引用支撑的论断会被核验工具标红。

2. **References 段**（文末，标题固定 `## References`）：
   每行一条，格式 `[n]\t完整引用信息`，必须包含以下标识符之一
   （否则无法通过核验）：
   - 期刊文献：`PMID: <数字>`（来自 PubMed，必须真实存在）
   - 任何文献：`doi: <DOI>`（必须可在 doi.org 解析）
   - 网页：完整 URL（必须可访问）
   
   示例：
   `[1]	IRAK4 Signaling Drives Resistance to Checkpoint Immunotherapy in Pancreatic Ductal Adenocarcinoma. Gastroenterology. 2022 Jun;162(7):2047-2062. doi: 10.1053/j.gastro.2022.02.035. PMID: 35271824.`

3. **来源真实性**：每条文献必须来自你实际检索到的结果
   （pubmed_search / web_fetch 的返回），凭记忆生成的 PMID/DOI 会被
   核验工具判为 not_found/mismatch。引用格式示例（期刊、年份、卷期页、
   DOI、PMID）尽量完整，便于人工快速校验。

## 结构

```
# 标题
## 1. 背景/引言（含引用）
## 2..N. 主题段落（每段一个机制/主题，段落内 [index] 引用）
## 结论（含引用）
## References
[1]	...
[2]	...
```

## 自检（提交前）

用 citation_check 工具核验一遍，修复所有 mismatch/not_found；
正文引用索引必须全部能在 References 中找到，反之 References 中
不应有正文从未引用的条目。
