---
name: survival-analysis
description: >
  Survival / time-to-event analysis in Python: Cox PH with lifelines,
  Kaplan-Meier, proportional hazards check, and the endpoint traps that
  make AD and other slow-disease analyses invalid.
triggers:
  - survival
  - Cox
  - Kaplan-Meier
  - hazard ratio
  - time to event
tools_needed: []
version: "1.0.0"
priority: 8
---

# 生存/时间-事件分析协议（lifelines）

## 环境与回退

- `lifelines`（pip 可装，miniagent 自动补装机制覆盖）；不可用时 KM 曲线可
  手写乘积极限估计，但 **Cox 回退到 `statsmodels.PHReg`**，再不行则如实
  报告无法执行并 raise。

## 流程

1. 数据必需三列：`duration`、`event`（1=事件，0=删失）、协变量。
   GEO/TCGA 元数据里生存字段常叫 `OS`、`OS.time`、`vital_status`——先
   枚举列名再映射，缺失时长列时**立即 raise**（不要把随访开始日当时长）。
2. KM：`KaplanMeierFitter` 按组画曲线 + log-rank 检验
   （`multivariate_logrank_test`）。
3. Cox：`CoxPHFitter().fit(df, 'duration', 'event')`；报告 HR、95%CI、p。
4. PH 假定：`cph.check_assumptions(df)`；违反时改用带时间交互的模型或
   分层，并在结果中说明。
5. 多变量嵌套模型比较用似然比检验或 C-index 增量。

## 输出契约

- `km_curves.png`、`cox_summary.csv`（covariate, HR, CI_low, CI_high, p）。
- `RESULT = {"logrank_p": …, "hr_primary": …, "n_events": …}`。

## 陷阱（生物医学审稿最常抓的点）

- 把诊断时点当随访起点 → immortal time bias。
- 事件率 <10% 时 HR 极不稳定，必须报 n_events 并限制结论强度。
- 删失机制非随机（如死亡即删失）→ 结论限定为"证据一致但不作因果解释"。
- 治疗变量在基线后变化 → 需要时变协变量，普通 Cox 会高估效应。
