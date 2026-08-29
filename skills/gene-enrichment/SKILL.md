---
name: gene-enrichment
description: >
  Gene-set enrichment analysis protocol: over-representation (Enrichr API,
  no key needed) and gseapy ranking; correct background sets and FDR.
triggers:
  - enrichment
  - GO
  - KEGG
  - pathway analysis
  - gene set
  - ORA
tools_needed:
  - enrichr
version: "1.0.0"
priority: 8
---

# 基因集富集分析协议

## 路线选择

1. **在线 Enrichr**（miniagent 内置 `enrichr` 工具可直接调用；或脚本里
   POST `https://maayanlab.cloud/Enrichr/addList`）——免 key、免装包，
   适合 10–2000 个基因的 ORA。
2. **离线超几何**：`scipy.stats.hypergeom` + 自带背景集，网络不可用时用，
   背景集取"检测到的全部基因"而非全基因组。

## Enrichr 调用模板

```python
import json, urllib.request
genes = ["TP53", "BRCA1", ...]            # 官方符号，HUGO 大写
body = urllib.parse.urlencode({
    "list": "\n".join(genes), "description": "sig genes"}).encode()
req = urllib.request.Request(
    "https://maayanlab.cloud/Enrichr/addList", data=body)
short = json.load(urllib.request.urlopen(req))["shortId"]
res = json.load(urllib.request.urlopen(
    f"https://maayanlab.cloud/Enrichr/enrich?userListId={short}&backgroundType=KEGG_2021_Human"))
# res["KEGG_2021_Human"]: [term, p, z, combined, genes, adj_p, ...]
```

## 协议要点

- 输入用官方基因符号；探针先映射，映射失败的基因单列计数并写入结果。
- 同时看 3 类库：GO Biological Process、KEGG_2021_Human、MSigDB_Hallmark_2020。
- 显著性用**校正后 p**（Enrichr 返回 adj p 值在第 6 列）；报告 top 10 条目 +
  命中基因列表。
- ORA 需要 ≥5 个显著基因才有意义；更少时改做单基因文献核查并在报告中说明。

## 输出契约

`enrichment.csv`（library, term, overlap, genes, p, adj_p）+ top 条目条形图。

## 陷阱

- 忘记设背景集 → hypergeom 分母错，全表虚假显著。
- 混用大小写/旧符号（如 TNF vs TNFA）→ 命中数虚低。
- 网络失败时静默输出空表 → 必须打印错误并 raise，让修复回路介入。
