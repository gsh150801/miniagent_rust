# 11 · 架构评审与功能状态 / Architecture Review & Feature Status

> 更新日期：2026-09-06。基于三轮端到端真实运行（ALS/TDP-43、帕金森/α-突触核蛋白×2、
> 睡眠-AD）与全代码审计。图例：✅ 已实现且实测 · ⚠️ 部分实现 · ❌ 未实现。

## 一、架构总览

```
17 crates | Rust 2024 | Tokio | axum(WS) | SQLite/FTS5 | calamine | 156 skills
```

三套执行模式共享同一 `agent` 运行时与 `tool`/`skill`/`memory` 基础设施：

| 模式 | 编排 | 适用 |
|------|------|------|
| workflow | DAG 引擎，LLM 规划阶段 | 通用任务 |
| loop | Explore→Plan→Dispatch(并行波次)→Evaluate→Repair | 长程多子任务 |
| research | 9 阶段科研管线，每阶段套 loop 语义（含三方裁决） | 四目标科学发现 |

## 二、优势 / Strengths

1. **审计深度是全框架第一优先级**。`project.json` append-only 事件日志贯穿所有阶段；
   每个分析任务有 `provenance.json`（脚本 SHA-256、输入输出哈希、conda 包版本、git
   commit、seed、repair_history）；KG 边可经 `kg_sources.json` 解析回 PMID；报告引用有
   `citation_check.json` 逐条核验；审核结论写回报告本体。任何结论都能回溯到
   文献 → 图谱边 → 假说 → 分析脚本 → 执行环境的完整链条。
2. **fail-closed 门 + 诚实降级并存**。语料一致性门、KG 空结果门、幽灵成功检测
   （"成功"任务产物缺失即打回）、引用标题错配 → fail；同时所有可降级处（辩论单假说
   失败、merge 结构化失败、notebook 执行失败回落 .py）都降级不丢主流程。
3. **真实事故驱动的韧性**。多轮实测撞出的故障都有针对性修复：efetch 文本格式摘要错位
   （P0）→ 改 XML 解析；provider 欠费/限流 → 跨族回退 + 401/402/403 熔断；conda 环境
   与系统 jupyter 脱节 → 环境内核优先 + 依赖自动补装；推理模型空响应 → 预算升级 +
   跨族重试 + 空脚本记为诚实失败。
4. **辩论机制完整**。四角色（立论/驳论/rebuttal/裁决）+ 外部证据抓取全文注入 +
   跨假说矛盾对比 + 精炼 + **merge/drop/revise 操作真正执行**（置信度只降不升、
   证据并集、操作数上限、集合不清空）。实测一次运行把 4 个冗余假说收敛为 2 个。
5. **多智能体 loop 是真并行**。Kahn 波次内 tokio spawn + 信号量限流，依赖任务直接注入
   上游输出摘要（不再依赖共享目录的隐性契约），critic/judge 按难度分层，完成前三方裁决。
6. **前端可读性投入充分**。多标签预览（md/json/表格/ipynb 含输出/Excel 多 sheet/图片）、
   结构化假说卡片（置信度变化、rebuttal、矛盾）、provenance 面板、澄清问答卡，
   全部支持历史重绘。
7. **测试覆盖关键回归**：37 个测试套件，含本 session 修的每个 bug 的回归用例
   （efetch 配对、SECTION→markdown cell、merge ops 语义与防护、引用解析、review 窗口）。

## 三、不足与风险 / Weaknesses & Risks

| # | 问题 | 证据/位置 | 影响 |
|---|------|----------|------|
| 1 | **非 GEO 数据源无下载器**：ENA（ERP*）、PPMI、TCGA、custom_url 声明了 DatasetSource 但无实现 → dry-run，只出脚本不执行 | pipeline.rs 只对 GEO 做 accession 落地与下载 | 宏基因组/临床队列类假说的分析任务（实测 3/8）无法端到端 |
| 2 | **loop 模式无自动续跑**：服务重启后 checkpoint 只在同一任务 follow-up 时间接触发；loop 内（dispatch 半途）崩溃丢整轮 | loop-pipeline 无磁盘级 per-wave checkpoint | 长任务对进程存活的依赖较高 |
| 3 | **串行审查瓶颈**：critic→judge 对成功任务逐个串行；repair 对失败任务逐个串行 | dispatch.rs / repair.rs | 大计划下阶段尾延迟线性增长 |
| 4 | **KG 抽取解析失败即丢文献**：LLM 输出 JSON 解析失败仅重试一次，失败论文静默缺图（实测 2/24 丢失） | pipeline.rs extraction | 图谱覆盖度打折，无显式审计入口 |
| 5 | **湿实验方案无执行/校验回路**：纯文本方案，步骤/试剂不做可行性校验 | hypothesis/validation.rs | 目标 3 的湿实验半边只有"计划" |
| 6 | **KG 质量评估默认关闭**：TransE hold-out 评估仅 opt-in 环境变量 | pipeline.rs kg_eval | 链路预测质量无持续度量 |
| 7 | **workflow 模式功能滞后**：澄清答案历史为空操作、记忆注入缺失、非增量路径单 stage 失败整体报错 | routes.rs / engine.rs | 三模式体验不齐 |
| 8 | **引用核验只做 PMID 级**：DOI/URL 在线核验能力在 citation_check 工具里但未入管线；语料外引用只告警不溯源（可能来自辩论 web 证据） | review.rs | 引用审计覆盖面有限 |
| 9 | **Excel 预览上限 500×60 无分页**；csv 预览受 200KB 文本截断 | routes.rs | 大表浏览受限（可下载） |
| 10 | **无 CI/集成测试**：全流程验证靠手工端到端跑，单测不覆盖阶段衔接 | 仓库无 CI 配置 | 回归风险靠纪律 |
| 11 | **单用户本地设计**：无鉴权、单 WS 连接单任务 | server | 不能多人/远程暴露 |

## 四、功能状态矩阵 / Feature Status

### 四大科研目标

| 目标 | 状态 | 明细 |
|------|------|------|
| G1 性能/可追溯/长程 | ✅（边界见不足 2/3） | 审计链完整 ✅ · 断点 resume（research）✅ · loop 自动续跑 ❌ · 供应商熔断 ✅ · 超时可配（3h）✅ |
| G2 假说提出与辩论完善 | ✅ | KG+链路预测 ✅ · 4 角色辩论含 rebuttal ✅ · 矛盾对比 ✅ · merge 执行 ✅ · 证据引用强制 ✅ · 精炼回写 ✅ |
| G3 可执行验证计划 | ⚠️ | 数据分析任务 ✅（GEO 落地）· 湿实验方案 ✅（生成）· 非 GEO 数据源 ❌ · 湿实验执行 ❌ |
| G4 端到端数据分析 notebook | ⚠️ | GEO 任务 ✅（.ipynb 带叙述 cell+真实输出+修复回路+provenance）· dry-run 降级 ✅ · 非 GEO 下载器 ❌ |

### 横切能力

| 能力 | 状态 | 能力 | 状态 |
|------|------|------|------|
| 跨供应商回退 | ✅ | 账户级熔断（401/402/403） | ✅ |
| 搜索后端健康探针 | ✅ | 语料一致性门 | ✅ |
| 技能运行时发现（156） | ✅ | 技能链子智能体核验 | ✅ |
| 澄清问答（双向） | ✅ | 运行中转向 | ✅ |
| goal_state 跨轮约束 | ✅ | 跨会话记忆（loop/workflow） | ✅ |
| 幽灵成功检测 | ✅ | 三方裁决 | ✅ |
| 分节报告审核 | ✅ | 引用逐条核验（PMID 级） | ✅ |
| ipynb 预览（含输出） | ✅ | Excel 预览（多 sheet） | ✅ |
| 图片/CSV(RFC4180) 预览 | ✅ | 假说结构化卡片 | ✅ |
| provenance 面板 | ✅ | trace 事件审计 | ✅ |
| notebook conda 内核执行 | ✅ | SECTION→叙述 cell | ✅ |
| DOI/URL 在线核验 | ❌ | 多用户鉴权 | ❌ |
| KG 质量评估默认开启 | ❌ | loop 自动续跑 | ❌ |

## 五、下一步建议（按收益/成本排序）

1. **ENA 下载器**（低成本高收益）：ERP* accession 经 ENA API 拉元数据+数据，直接消灭
   宏基因组类任务的 dry-run。
2. **PPMI/TCGA 替代检索**：需注册的库自动降级为"检索其公开衍生数据集"而非 dry-run。
3. **critic/judge 与 repair 并行化**：tokio join_all 化，收益随任务数线性。
4. **KG 抽取解析失败重试升级**：失败论文走跨族回退二次抽取 + 缺失清单落盘审计。
5. **CI + 冒烟集成测试**：mock LLM server（scripts/mock_llm_server.py 已有）驱动
   research 管线 4 篇论文全流程，防阶段衔接回归。
6. **loop per-wave checkpoint**：波次完成后落盘，服务重启可续。
