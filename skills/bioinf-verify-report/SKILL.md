---
name: bioinf-verify-report
description: >
  Report verification skill (dsh-bioinfo verify-report style): check a
  generated report's citations, references, file references, and claims
  against real sources; produce a structured mismatch list so a writer
  agent knows exactly what to fix. Use before delivering any report, or
  when asked to 校验/核实/verify a report.
triggers:
  - 校验引用
  - verify citations
  - 核实报告
  - check references
  - 引用核验
tools_needed:
  - citation_check
  - web_fetch
  - pubmed_search
version: "1.0.0"
priority: 10
---

# 报告核验技能（citation/fact verification）

## 工作流

1. **引用核验**：对报告全文调用 `citation_check` 工具——自动解析
   正文 [n] 标记与 References 段，逐条核对：
   - PMID → PubMed E-Summary 标题/期刊/年份比对
   - DOI → doi.org 解析
   - URL → 可达性
   - 正文引用索引 vs References 覆盖（双向缺失检查）
2. **文件引用核验**：报告中提到的每个产物文件（图表/CSV）用 glob/read
   确认真实存在。
3. **事实抽查**：对关键数值/结论（药物名+效果、里程碑+年份）用
   pubmed_search / web_search 交叉验证，不一致即为 mismatch。
4. **产出修复清单**：按严重度排序的 `mismatch_list`（每条含位置、
   问题、建议修正），交给写作方针对性修改后重新核验，直到全部 ✅。

## 输出格式（固定）

```
## 核验结论：PASS / FAIL
- 引用：N 条，verified X / mismatch Y / not_found Z / unverifiable W
- 文件引用：全部存在 / 缺失 [...]
- 事实抽查：N 条，一致 X / 不一致 [...]
## mismatch_list
1. [high] 位置：...；问题：...；建议：...
```

## 红线

- PubMed 查无此 PMID ⇒ not_found（虚构引用），必须删除或替换为
  真实检索到的文献，不允许"改个相近的 PMID"。
- 报告主题与文献主题错配（如 AD 报告引用植物病理文献）⇒ [high]。
- 核验工具不可用时明确标注"未能核验"，不得假装已核验。
