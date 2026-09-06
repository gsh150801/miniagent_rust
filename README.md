# miniagent

> 高性能 Rust AI Agent 框架，专为长程科研任务设计——从海量文献到致病机理假说、
> 到可执行验证计划、再到端到端数据分析 notebook。
> High-performance AI agent framework in Rust for long-running scientific research tasks.

## 核心能力 / Core Capabilities

- **科研发现管线 / Research Pipeline**（四目标全流程，✅ 已实测）：PubMed 检索（efetch XML
  解析）→ 语料一致性门 → 相关性过滤 → 知识图谱（PMID 级溯源）→ TransE+GIVE 链路预测 →
  假说生成排序 → 对抗辩论（立论→驳论→rebuttal→裁决→跨假说对比→**假说合并执行**）→
  验证计划（GEO 落地 + 湿实验方案）→ 端到端数据分析（.ipynb 带 cell 叙述与真实输出）→
  报告审核（机械校验 + 分节 LLM 审核 + 引用逐条核验）
- **多智能体循环 / Loop Pipeline**（✅）：Explore→Clarify→Plan→Dispatch→Evaluate→Repair，
  Kahn 波次并行 Dispatch、依赖任务注入上游输出、critic/judge 分层审查、三方裁决
  （advocate→challenger→arbiter）、幽灵成功检测、执行中转向（steer）、跨轮约束继承（goal_state）
- **Web UI**（✅）：ChatGPT 风格三栏界面，实时阶段 pills、工具调用操作卡、流式输出、
  澄清问答、**结构化假说/辩论卡片**（支持/反驳/rebuttal/置信度变化/跨假说矛盾）、
  **多标签预览面板**（Markdown/JSON/CSV/TSV/`.ipynb` 含输出/**Excel 多 sheet**/图片）、
  **Provenance 溯源面板**、Trace 事件审计、任务历史/搜索/删除
- **自定义智能体 / Custom Agent Roles**（✅）：⚙️ 设置页新建/编辑/删除智能体角色
  （角色名、Persona、工具白名单、配套技能，存 `agents.json`）；**生成器**按
  base_url / 鉴权 / API key / 文档一键生成 Python 工具脚本 + SKILL.md（key 只落
  `.miniagent/secrets/`，0600）；技能面板支持导入/查看/删除（用户目录
  `.miniagent/skills/`）；Loop 规划阶段可见自定义角色目录并自主分配子任务；
  输入栏可强制指定"某智能体 + 某技能"执行任务（loop 确定性覆盖 + workflow
  prompt 注入）
- **跨供应商容灾 / Cross-Vendor Resilience**（✅）：DeepSeek / StepFun / MiniMax 自动回退，
  401/402/403 账户级错误**进程内熔断**该厂商；搜索后端健康探针 + 熔断
- **可扩展技能 / Extensible Skills**（✅）：156 个科学技能，drop SKILL.md 自动发现注册；
  Web UI 导入/查看/删除用户技能
- **四层记忆 / 4-Layer Memory**（✅）：L0 工作记忆 → L1 情景记忆 (SQLite FTS5) →
  L2 语义记忆 (向量) → L3 技能记忆；loop/workflow 模式注入跨会话经验
- **自改进 / Self-Improvement**（✅）：在线 Step-Reflection + Q-Router；离线 Experience Graph + Skill 生命周期

功能实现状态明细见 **[docs/11-architecture-review.md](docs/11-architecture-review.md)**（含已实现/部分/未实现矩阵）。

## 架构 / Architecture

```
17 crates | Rust 2024 | Tokio async | DeepSeek / StepFun / MiniMax | 156 skills
```

```
┌──────────────────────────────────────────────────────────┐
│  Web UI ── CLI ── REST API ── WebSocket                   │  ← 接口层
├──────────────────────────────────────────────────────────┤
│  Workflow Planner ── DAG Engine ── Loop Pipeline ── Skill │  ← 编排层
├──────────────────────────────────────────────────────────┤
│  Stateless Agent Loop                                     │
│  Context Assembly → LLM Call → Tool Dispatch              │  ← 运行时
│  + Self-Improvement (online + offline)                    │
├──────────────────────────────────────────────────────────┤
│  4-Layer Memory ── Telemetry ── Provenance                │  ← 基础设施
│  Knowledge Graph ── Hypothesis ── Analysis Notebook       │
└──────────────────────────────────────────────────────────┘
```

### Crate 清单 / Crate Map（17 crates）

| 层 Layer | Crate | 用途 Purpose |
|----------|-------|-------------|
| 基础 Foundation | `core` | 共享类型、预算、事件、错误、Kahn 调度、JSON 修复 |
| LLM | `provider` | DeepSeek/StepFun/MiniMax 三族客户端 + 流式 + 深度推理 + 跨族回退 + **账户级熔断**（401/402/403 进程内禁用） |
| 运行时 Runtime | `agent` | 无状态 Agent Loop + 工具循环 + 历史压缩 + 技能注入 |
| 工具 Tools | `tool` | read/write/edit/glob/grep/bash/fetch/search/pubmed/notebook_edit/citation_check + 多后端搜索回退（健康探针 + 熔断） |
| 记忆 Memory | `memory` | 4 层: L0 工作 → L1 情景 (SQLite FTS5) → L2 语义 (向量) → L3 技能 |
| 工作流 Workflow | `workflow` | DAG 引擎 + 动态规划器 + 重试 + Mermaid 可视化 |
| 循环 Loop | `loop-pipeline` | Explore→Clarify→Plan→Dispatch(并行波次)→Evaluate→Repair + critic/judge/adjudicate |
| 规划 Planning | `planning` | 任务分解 + StateGraph 动态调度 + AgentRole 多角色编排 + 辩论执行器（CLI `plan`/`team`/`debate`） |
| 知识图谱 KG | `kg` | 实体/关系抽取、canonical 别名合并、TransE 嵌入、GIVE 链路预测、跨项目 KG store |
| 假说 Hypothesis | `hypothesis` | 假说生成/验证/排序 + **对抗辩论**（含 rebuttal 轮与 merge/drop/revise 操作执行）+ 验证计划 |
| 科研管线 Research | `research` | 9 阶段科研发现管线（详见下文）+ project.json 审计 + 报告生成 + 审核 |
| 数据分析 Analysis | `analysis` | LLM 脚本生成 → 执行 → 修复回路 → conda 环境管理 → notebook 生成（SECTION 叙述 cell）→ provenance |
| 技能 Skills | `skill` | 运行时 SKILL.md 发现、触发器匹配、技能链 |
| 自改进 Self-Improve | `self-improve` | 在线 Step-Reflection + Q-Router + Lifecycle Guard；离线 Experience Graph + Skill Manager |
| 遥测 Telemetry | `telemetry` | 结构化 JSON 日志、原子指标计数器 |
| 服务 Server | `server` | axum 0.8 REST API + WebSocket + 内嵌前端（多标签预览/假说卡片/溯源面板/Excel·ipynb·图片预览） |
| CLI | `cli` | 所有命令入口 |

> 曾有 `checkpoint`/`evolution`/`python`/`sandbox` crate 已在架构演进中移除/合并
> （checkpoint 语义由 manifest resume + metadata.json 承担）。

## 功能状态 / Feature Status

图例：✅ 已实现且经端到端实测 · ⚠️ 部分实现（有已知边界） · ❌ 未实现（规划中）

| 功能 | 状态 | 说明 |
|------|------|------|
| 文献检索 → 摘要结构化（efetch XML） | ✅ | 修复了文本格式摘要错位的 P0 缺陷；Title/Year/Abstract 逐篇配对 |
| 语料一致性门 + 相关性过滤 | ✅ | fail-closed：语料不可判/不相关即中止，不毒化下游 |
| KG 构建 + PMID 级溯源 | ✅ | `kg_sources.json` 把每条关系的 source id 解析回 PMID+标题 |
| 跨项目 KG store 累积 | ✅ | `kg_store.json` 合并历史实体/关系扩大链路预测候选 |
| 链路预测（TransE + 路径 + GIVE） | ✅ | 疾病锚定过滤候选 |
| 假说生成 + 启发式排序 | ✅ | LLM 评估 plausibility，空响应/不可信自动跳过 |
| 对抗辩论（4 角色含 rebuttal） | ✅ | 正方立论 → 反方驳论 → 正方 rebuttal → 裁决；证据要点强制带 URL/PMID；反方独立意见与已执行合并操作前端可见 |
| 跨假说对比 + 精炼 | ✅ | 矛盾对比、排序理由、merge 建议 |
| **假说合并执行**（merge/drop/revise） | ✅ | 裁判建议结构化为操作并真正应用；置信度只降不升、证据并集、≤3 操作 |
| 验证计划生成 + GEO 落地 | ⚠️ | GEO accession 自动校验落地；**TCGA/ArrayExpress/ENA/PPMI/custom_url 无下载器 → dry-run** |
| 湿实验方案生成 | ✅ | 含对照组设计/时间线/步骤；❌ 无执行/校验回路（纯方案） |
| 端到端数据分析 + notebook | ⚠️ | GEO 任务全流程 ✅（脚本生成→conda 环境执行→.ipynb 带输出→修复回路）；非 GEO 数据源 ❌ |
| 引用逐条核验 | ✅ | 链接抽取 → 双向语料比对 → esummary 标题比对；标题错配 → fail |
| 报告分节 LLM 审核 | ✅ | 按 `##` 分节窗口审核，全报告覆盖，节选不再误报截断 |
| 报告章节过渡 | ✅ | 各节承接叙述（含动态数量），去机械拼装感 |
| Loop 多智能体并行 | ✅ | 波次并行 + 上游输出注入 + critic/judge（审查结果事件化，前端审查区块）+ 三方裁决（评估·裁决卡） |
| 服务重启自动续跑 | ⚠️ | research 模式支持 manifest resume（同一 project-dir 重跑）；loop 模式仅 follow-up 间接触发，**无自动续跑** |
| 预览：Markdown/JSON/CSV/TSV | ✅ | CSV/TSV 为 RFC4180 感知解析（引号字段/多行字段/截断半行丢弃） |
| 预览：`.ipynb`（含输出/图表） | ✅ | 单元格级渲染：markdown/code/输出/错误回溯 |
| 预览：Excel（.xlsx/.xls/.xlsm/.xlsb） | ✅ | calamine 服务端解析，多 sheet 切换，500×60 截断 |
| 预览：图片（png/jpg/svg/gif/webp） | ✅ | raw 路由内联渲染 |
| 结构化假说/辩论卡片 | ✅ | 专用 WS 事件 + 历史重绘；置信度变化、rebuttal、跨假说矛盾、已执行合并操作、反方独立意见 |
| 数据分析结果卡 | ✅ | 每 DA 任务执行结局（成功/dry-run/失败）+ notebook/provenance 一键预览 + 自修复轮数 + 输入警告（合成数据演示显式降级） |
| Provenance 溯源面板 | ✅ | 脚本哈希/seed/conda 包版本/git commit/repair 历史 |
| 跨供应商回退 + 账户熔断 | ✅ | DeepSeek→StepFun→MiniMax；401/402/403 进程内禁用 |
| 跨会话记忆注入 | ⚠️ | loop/workflow 模式 ✅；research 模式刻意不注入（防查询污染） |
| KG 质量评估（TransE hold-out） | ❌ | 仅 opt-in 环境变量，默认关闭 |
| 多用户/鉴权（Web UI） | ❌ | 设计为单用户本地部署 |
| DOI/URL 逐条在线核验 | ❌ | citation_check 工具具备该能力，主管线仅做 PMID 级 |

## 快速开始 / Quick Start

### 环境要求 / Prerequisites

- Rust 1.85+
- 任一 LLM API key（DeepSeek / StepFun / MiniMax——三族互为回退，保留几家的 key 就有容灾；
  401/402/403 账户级错误会自动熔断该家族）
- 数据分析需 conda 或 micromamba（自动创建环境并补装依赖）；Jupyter 可选（有则直接以
  conda 环境为内核执行 notebook）

### 安装 / Setup

```bash
git clone https://github.com/gsh150801/miniagent_rust.git
cd miniagent_rust

# 配置 API 密钥
cp .env.example .env
# 编辑 .env 填入至少一家 LLM key（PROVIDER 指定默认厂商）

cargo build --release
```

### 启动 Web UI / Start Web UI

```bash
cargo run -p miniagent-server
# 浏览器打开 http://localhost:3002 （SERVER_PORT 可改）
```

Web UI 功能：
- **任务规划可视化**: 阶段 pills + 计划卡片 + 每个子任务执行卡（角色/耗时/token/复用标记）
- **实时执行追踪**: 工具调用逐条操作卡（默认收起，展开看参数与结果）
- **结构化假说卡片**: 研究管线完成后推送假说卡（陈述/机制/置信度变化/支持与反驳要点/rebuttal/跨假说矛盾）
- **多标签预览面板**: Files 树内浏览器式多标签——Markdown 渲染、JSON、RFC4180 表格、
  `.ipynb` 单元格（含图表输出）、Excel（多 sheet 切换）、图片内联
- **Provenance 溯源**: 一键查看每个分析任务的脚本哈希、输入输出、conda 包版本、修复历史
- **澄清问答 + 运行中转向**: 双向 `ask` 协议；任务运行中插入指令在阶段边界生效
- **任务管理**: 历史任务列表 + 搜索 + 删除 + Trace 事件审计

### CLI 使用 / CLI Usage

```bash
# ── 基础对话 / Basic Chat ──────────────────────────────
miniagent run -p "什么是 CRISPR" -P flash
miniagent run -p "深度分析..." -P pro -c complex

# ── 科研管线 / Research Pipeline（四目标全流程，推荐）───
# 文献 → KG → 链路预测 → 假说 → 辩论(含假说合并) → 验证计划 → 数据分析 → 审核
miniagent research -q "Alzheimer's disease pathogenesis mechanism" \
  -n 20 --validate --analyze --use-store --top-n 3 --project-dir result/my_run
# 常用旗标：
#   --validate      生成验证计划（默认开启辩论）
#   --analyze       端到端执行数据分析任务（GEO 自动下载）
#   --use-store     用跨项目 KG store 扩大链路预测候选
#   --kg-only       只构建知识图谱
#   --enrich-file   用 DisGeNET/OMIM 三元组富化图谱
#   --project-dir   指定审计目录（重跑同一目录即断点 resume）

# ── 多智能体循环 / Loop Pipeline ───────────────────────
miniagent loop -q "调研近两年 ALS 新靶标并写综述" -n 10

# ── 科学辩论 / Scientific Debate ──────────────────────
miniagent debate -q "CRISPR off-target risk" -r 3

# ── 技能管理 / Skill Management ───────────────────────
miniagent skill list / search "meta analysis" / show <name>

# ── 遥测 / 配置 ───────────────────────────────────────
miniagent metrics
miniagent config
```

## 科研发现管线 / Research Pipeline（9 阶段）

```
Phase 1  文献检索     查询翻译(LLM) → PubMed 检索
Phase 2  摘要获取     efetch XML 解析（Title/Year/Abstract 逐篇配对）
Phase 2a 语料一致性门 fail-closed：语料与研究问题不符即中止
Phase 2b 相关性过滤   LLM 逐篇打分，拒绝项落盘审计
Phase 3  KG 抽取      逐篇并发抽取实体/关系 → canonical 别名合并 → PMID 溯源 (kg_sources.json)
Phase 3b 图谱富化     DisGeNET/OMIM 三元组合并 + 跨项目 KG store
Phase 4  链路预测     TransE 128 维 + 路径 + GIVE 评分 → 疾病锚定候选
Phase 5  假说生成     候选 → LLM 机制解释 + 实验设计 → plausibility 验证 → 排序
Phase 6b 对抗辩论     web 证据检索(抓全文) → 正方立论 → 反方驳论 → 正方 rebuttal →
                     裁决 → 跨假说对比 → 精炼 → merge/drop/revise 操作执行
Phase 7  验证计划     数据分析任务（GEO accession 校验落地）+ 湿实验方案
Phase 8  数据分析     脚本生成(技能提示) → conda 环境执行 → .ipynb 带叙述 cell 与输出
                     → 失败自修复(≤3 轮) → provenance.json
Phase 9  报告审核     机械数字校验 → 分节 LLM 审核 → 引用逐条核验 → 结论写回报告
```

审计产物（`--project-dir` 下）：`project.json`（append-only 事件日志）、`papers.json`、
`kg.json` + `kg_sources.json`、`hypotheses_full/refined_full.json`、`debate_report.json`
（含 `merge_ops_applied`）、`plans/validation_plan_*.json`、`analysis/**/provenance.json`、
`citation_check.json`、`report_review.json`、`run_report.md`。

## 核心设计 / Key Design

### 无状态 Agent Loop / Stateless Agent Loop

调用者拥有消息历史，Agent 只返回增量 (delta)。天然支持 fork/retry/resume/audit。
历史超长时自动压缩：保留 prompt + LLM 摘要 + 最近若干轮，摘要持久化。

### 数据分析自修复 / Analysis Self-Repair

每个数据分析任务运行 **生成 → 执行 → 修复** 回路（最多 3 次尝试）：

- **GEO 数据 schema 摘要**：清洗后的 series matrix 以结构化摘要喂给脚本生成；
- **依赖自动补装**：执行前解析脚本 import，对每个候选解释器探测并安装缺失模块；
- **conda 环境内核优先**：notebook 优先用 conda 环境解释器执行（系统 jupyter 二进制
  损坏/缺失时自动降级），notebook 产出真实输出与图表；
- **cell 级叙述**：生成脚本带 `# == SECTION: … ==` 标记，转换为 notebook 叙述 markdown cell；
- **审计**：每轮修复记录在 `provenance.json` 的 `repair_history`。

### 跨供应商容灾 / Cross-Vendor Resilience

- 只要配置了不止一家的 key，长文本生成类调用按 **DeepSeek → StepFun → MiniMax** 逐家回退
  （跳过当前活跃家族）；
- **401/402/403 账户级错误触发进程内熔断**：该厂商家族在本次运行中不再进入回退链
  （欠费/坏 key 不会中途恢复），429 等瞬态错误仍正常重试；
- 搜索后端同样有启动健康探针 + 失败临时禁用；
- 全部供应商不可用时管线诚实熔断，同一 `--project-dir` 重跑即断点恢复。

### 四层记忆 / 4-Layer Memory

| 层 Layer | 存储 Storage | 用途 Purpose |
|----------|-------------|-------------|
| L0 工作 Working | 上下文窗口 | 当前任务 + 索引指针 |
| L1 情景 Episodic | SQLite FTS5 | 结构化摘要 + 关系图 |
| L2 语义 Semantic | 向量 | 全文嵌入 |
| L3 技能 Procedural | SKILL.md 仓库 | 可复用技能模板 |

### 可扩展技能 / Extensible Skills

放置 SKILL.md 文件，Agent 运行时自动发现（triggers/priority/tools_needed 元数据）。
已集成 **156 个科学技能**（来自 [K-Dense-AI/scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills)）。

## 配置 / Configuration

通过 `.env` 文件配置（参考 `.env.example`），常用项：

| 变量 Variable | 默认 Default | 说明 Description |
|--------------|-------------|----------------|
| `PROVIDER` | `deepseek` | 启动时激活的内置模型档案（deepseek/stepfun/minimax），Web UI ⚙ 可热切换 |
| `DEEPSEEK_API_KEY` / `STEPFUN_API_KEY` / `MINIMAX_API_KEY` | — | 保留即加入跨厂商回退链（账户级错误自动熔断） |
| `RESEARCH_TIMEOUT_SECS` | `10800` | research 模式总超时（默认 3h，重语料可调大） |
| `MAX_ITERATIONS` | `35` | Agent 最大工具循环次数 |
| `SERVER_PORT` | `3002` | Web UI 端口 |
| `PUBMED_API_KEY` | 可选 | 提高 PubMed 速率上限 |
| `LOOP_MAX_LOOPS` / `LOOP_DISPATCH_WAVE_CONCURRENCY` 等 | 见 `.env.example` | Loop 管线参数 |

## 项目结构 / Project Structure

```
miniagent/
├── crates/              # 17 Rust crates（见上方 Crate Map）
├── skills/              # 156 技能定义 / Skill Definitions
├── docs/                # 设计文档 + 架构评审 / Design Docs + Architecture Review
├── scripts/             # 辅助脚本 (mock LLM server)
└── .env.example         # 配置模板 / Config Template
```

## 设计文档 / Design Documents

| 文档 Doc | 内容 Content |
|---------|-------------|
| [架构评审与功能状态](docs/11-architecture-review.md) | 优势/不足分析、已实现/未实现矩阵、下一步建议 |
| [总体架构](docs/00-overall-architecture.md) | 技术选型、Crate 拆分、路线图 |
| [无状态 Agent Loop](docs/01-stateless-agent-loop.md) | 六大需求分析、核心 API |
| [记忆系统](docs/02-memory-system.md) | 四层架构、遗忘曲线、Consolidation |
| [自改进](docs/03-self-improvement.md) | 双层架构、Q-Router、Lifecycle Guard |
| [知识图谱与假设](docs/04-knowledge-graph-hypothesis.md) | KG 构建、GIVE、LLM 验证 |
| [多智能体对比/重设计](docs/05-multi-agent-comparison.md) · [06-multi-agent-redesign](docs/06-multi-agent-redesign.md) | Loop 管线设计 |
| [MLEvolve 集成](docs/07-mlevolve-integration.md) · [前端重设计](docs/08-frontend-redesign.md) | 进化机制 / UI 演进 |
| [优化路线/变更记录](docs/optimization-roadmap.md) · [changelog](docs/optimization-changelog.md) | 优化过程记录 |

## License

MIT
