---
name: mixed-effects-models
description: >
  Mixed-effects (multilevel) modeling protocol with statsmodels MixedLM:
  when random effects are needed, specification, and convergence handling.
triggers:
  - mixed model
  - mixed effects
  - lme4
  - random effect
  - repeated measures
  - longitudinal
tools_needed: []
version: "1.0.0"
priority: 7
---

# 混合效应模型协议（statsmodels.MixedLM）

## 什么时候必须用

- 同一受试者多个观测（纵向随访、多区域测量、多细胞/多探针嵌套于供体）。
- 任务文本里的 `lme4`、`(1|subject)`、`random effect`、`nested`。
- 普通回归在这些数据上会**低估标准误** → 假阳性。

## 规范

```python
import statsmodels.formula.api as smf
# 随机截距：结果 ~ 固定效应 + (1|组)
md = smf.mixedlm("y ~ stage + age + sex", data=df, groups=df["subject"])
res = md.fit(method="lbfgs")
# 随机斜率：smf.mixedlm("y ~ time + age", df, groups=df["subject"],
#                      re_formula="~time")
```

## 报告

- 固定效应：coef、SE、p（res.summary() 转存 CSV）。
- 组内相关 ICC = `re_var / (re_var + resid_var)`。
- 随机斜率不收敛时：先简化 re_formula，再报告"仅随机截距模型"。

## 陷阱

- 组数 <5 时随机效应方差不可估 → 改用 GEE 或聚类稳健 SE
  （`statsmodels.OLS(...).fit(cov_type="cluster", cov_kwds=...)`）。
- 完全收敛失败时循环降级：`["lbfgs", "bfgs", "powell"]` 逐个试。
- 缺失值要 dropna 于公式涉及的列，并把 n、n_groups 写进 RESULT。
