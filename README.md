# miniagent

> 高性能 Rust AI Agent 框架，专为长程科研任务设计。
> High-performance AI agent framework in Rust for long-running scientific research tasks.

## 核心能力 / Core Capabilities

- **动态工作流规划 / Dynamic Workflow Planning**: LLM 分析用户意图 → 自动规划多阶段工作流 (研究→批判→综合) → DAG 并行执行
- **Web UI**: 内嵌 ChatGPT 风格界面，WebSocket 流式输出，文件上传/下载
- **文献综述 / Literature Review**: 搜索 PubMed 等数据库 → 抓取摘要 → 知识图谱构建 → 链路预测 → 假设生成
- **知识图谱 / Knowledge Graph**: LLM 实体关系抽取 → TransE 嵌入 → 混合评分链路预测 → GIVE 可信度外推
- **假设生成 / Hypothesis Generation**: 图谱候选 → DeepSeek Pro 深度推理验证 → 机制解释 + 实验设计 → 多维排序
- **可扩展技能 / Extensible Skills**: 142 个科学技能，drop SKILL.md 自动发现注册
- **自改进 / Self-Improvement**: 在线 Step-Reflection + Q-Learning 路由；离线 Experience Graph + Skill 生命周期
- **四层记忆 / 4-Layer Memory**: L0 工作记忆 → L1 情景记忆 (SQLite FTS5) → L2 语义记忆 (向量) → L3 技能记忆

## 架构 / Architecture

```
18 crates | Rust 2024 | Tokio async | DeepSeek Flash/Pro | 142 skills
```

```
┌─────────────────────────────────────────────────┐
│  Web UI ── CLI ── REST API ── WebSocket          │  ← 接口层
├─────────────────────────────────────────────────┤
│  Workflow Planner ── DAG Engine ── Skill System  │  ← 编排层
├─────────────────────────────────────────────────┤
│  Stateless Agent Loop                            │
│  Context Assembly → LLM Call → Tool Dispatch      │  ← 运行时
│  + Self-Improvement (online + offline)           │
├─────────────────────────────────────────────────┤
│  4-Layer Memory ── Checkpoint ── Telemetry       │  ← 基础设施
│  Knowledge Graph ── Hypothesis ── Sandbox        │
└─────────────────────────────────────────────────┘
```

### Crate 清单 / Crate Map

| 层 Layer | Crate | 用途 Purpose |
|----------|-------|-------------|
| 基础 Foundation | `core` | 共享类型、预算、事件、错误 |
| LLM | `provider` | DeepSeek Flash/Pro + 深度推理 + 多供应商路由 |
| 运行时 Runtime | `agent` | 无状态 Agent Loop + 工具循环 + 历史压缩 |
| 工具 Tools | `tool` | 9 个内置工具 (read/write/edit/glob/grep/bash/fetch/search/pubmed) + 多后端搜索回退 |
| 记忆 Memory | `memory` | 4 层: L0 工作 → L1 情景 (SQLite FTS5) → L2 语义 (向量) → L3 技能 |
| 持久化 Persistence | `checkpoint` | SQLite 检查点 + 恢复 |
| 工作流 Workflow | `workflow` | DAG 引擎 + 动态规划器 + 6 Stage + 重试 + Mermaid 可视化 |
| 规划 Planning | `planning` | 任务分解 + 角色 + 黑板 + Hook + StateGraph + 控制壳 |
| 知识图谱 KG | `kg` | 实体抽取、TransE/RotatE 嵌入、GIVE 链路预测 |
| 假设 Hypothesis | `hypothesis` | LLM 验证假设 + 实验设计 |
| 自改进 Self-Improve | `self-improve` | 在线: Step-Reflection + Q-Router + Lifecycle Guard。离线: Experience Graph + Skill Manager + Sleeptime |
| 技能 Skills | `skill` | 运行时 SKILL.md 发现、触发器匹配、技能链 |
| Python | `python` | PyO3 科学计算桥接 (接口占位) |
| 沙箱 Sandbox | `sandbox` | WASM 沙箱 deny-by-default (接口占位) |
| 遥测 Telemetry | `telemetry` | 结构化 JSON 日志、原子指标计数器 |
| 服务 Server | `server` | axum REST API + WebSocket + 内嵌前端 |
| CLI | `cli` | 所有命令入口 |

## 快速开始 / Quick Start

### 环境要求 / Prerequisites

- Rust 1.85+
- DeepSeek API key

### 安装 / Setup

```bash
git clone <repo>
cd miniagent_rust

# 配置 API 密钥
cp .env.example .env
# 编辑 .env 填入 DEEPSEEK_API_KEY

# 编译
cargo build --release
```

### 启动 Web UI / Start Web UI

```bash
cargo run -p miniagent-server
# 浏览器打开 http://localhost:3000
```

功能：WebSocket 流式输出、文件上传/解析、结果文件下载、任务历史管理。

### CLI 使用 / CLI Usage

```bash
# ── 基础对话 / Basic Chat ──────────────────────────────
miniagent run -p "什么是 CRISPR" -P flash
miniagent run -p "深度分析..." -P pro -c complex

# ── 深度研究 / Deep Research ──────────────────────────
miniagent run -p "分析 CRISPR 基因编辑安全性" -c deep-research

# ── 科研管线 / Research Pipeline ──────────────────────
# 搜索文献 → 构建知识图谱 → 生成假设 (推荐！)
miniagent research -q "BRCA1 DNA repair breast cancer" -n 10

# 仅构建知识图谱，不生成假设
miniagent research -q "gene editing off-target" -n 5 --kg-only

# ── 技能管理 / Skill Management ───────────────────────
miniagent skill list              # 列出所有技能
miniagent skill search "meta analysis"  # 语义搜索
miniagent skill show crispr-review     # 技能详情

# ── 工作流 / Workflow ─────────────────────────────────
miniagent literature-review -q "gene editing safety" -n 10

# ── 科学辩论 / Scientific Debate ──────────────────────
miniagent debate -q "CRISPR off-target risk" -r 3

# ── 自改进演示 / Self-Improvement Demo ────────────────
miniagent self-improve

# ── 遥测 / Telemetry ──────────────────────────────────
miniagent metrics

# ── 配置 / Configuration ──────────────────────────────
miniagent config
```

## 动态工作流规划 / Dynamic Workflow Planning

用户输入不匹配任何预设模板时，系统自动调用 LLM 分析任务并生成自定义工作流：

```
用户 Prompt
    ↓
PlannerStage (LLM 分析)
    ↓
WorkflowSpec JSON (自定义阶段 + 边)
    ↓
WorkflowBuilder → DAG
    ↓
Workflow.run() → 结果
```

内置工作流模板（由 LLM 参考，非关键词匹配）：
- **single_agent**: 单 Agent 工具循环
- **deep_research**: 研究 → 批判 → 综合 (3 阶段)
- **writing**: 调研 → 写作 (2 阶段)

## 核心设计 / Key Design

### 无状态 Agent Loop / Stateless Agent Loop

调用者拥有消息历史，Agent 只返回增量 (delta)。天然支持 fork/retry/resume/audit。

```
Agent::run(history, context, cancel) → AgentDelta { new_messages, stop_reason, usage }
Agent::run_with_loop(history, context, cancel) → AgentDelta  // 多轮工具循环
```

历史超长时自动压缩：保留 prompt + LLM 摘要 + 最近 5 轮，摘要持久化到记忆库和磁盘。

### 四层记忆 / 4-Layer Memory

| 层 Layer | 存储 Storage | 用途 Purpose |
|----------|-------------|-------------|
| L0 工作 Working | 上下文窗口 Context window | 当前任务 + 索引指针 |
| L1 情景 Episodic | SQLite FTS5 | 结构化摘要 + 关系图 |
| L2 语义 Semantic | LanceDB (向量) | 全文嵌入 + 知识图谱 |
| L3 技能 Procedural | SKILL.md 仓库 | 可复用技能模板 + 元技能 |

### 知识图谱 → 假设生成 / KG → Hypothesis

```
论文 → LLM 实体抽取 → KG (实体+关系)
  → TransE 嵌入 → 链路预测 (KGE + 路径 + GIVE)
    → DeepSeek Pro 验证 → 排序假设 + 实验方案
```

### 自改进 / Self-Improvement

| 层 Layer | 组件 Component | 用途 Purpose |
|----------|---------------|-------------|
| 在线 Online | Step Reflection | 每步即时诊断 |
| 在线 Online | Q-Learning Router | 学习最优策略选择 |
| 在线 Online | Lifecycle Guard | 防止性能退化 (Ratchet 定理) |
| 离线 Offline | Experience Graph | 结构化成功/失败模式 |
| 离线 Offline | Skill Manager | Draft → Active → Deprecated 生命周期 |
| 离线 Offline | Sleeptime Consolidation | 衰减→去重→聚类→重建 |

### 可扩展技能 / Extensible Skills

放置 SKILL.md 文件，Agent 运行时自动发现：

```markdown
---
name: my-skill
description: 技能描述
triggers:
  - 何时触发
tools_needed:
  - pubmed_search
version: "0.1.0"
priority: 5
---

# 技能正文
... LLM 指令 ...
```

已集成 142 个科学技能 (来自 [K-Dense-AI/scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills))。

## 配置 / Configuration

通过 `.env` 文件配置（参考 `.env.example`）：

| 变量 Variable | 默认 Default | 说明 Description |
|--------------|-------------|----------------|
| `DEEPSEEK_API_KEY` | 必填 | DeepSeek API 密钥 |
| `DEEPSEEK_BASE_URL` | `https://api.deepseek.com` | API 基础 URL |
| `BOCHA_API_KEY` | 可选 | Bocha 搜索 API |
| `TAVILY_API_KEY` | 可选 | Tavily 搜索 API |
| `SERPAPI_API_KEY` | 可选 | SerpAPI 搜索 API |
| `SERPER_API_KEY` | 可选 | Serper 搜索 API |
| `PUBMED_API_KEY` | 可选 | PubMed API |
| `MAX_ITERATIONS` | `35` | Agent 最大工具循环次数 |
| `MAX_TOKENS` | `393216` | 最大输出 token 数 |

## 项目结构 / Project Structure

```
miniagent/
├── crates/              # 18 Rust crates
│   ├── core/            # 基础类型 / Foundation
│   ├── provider/        # LLM 抽象 / Abstraction
│   ├── agent/           # Agent 运行时 / Runtime
│   ├── tool/            # 工具系统 / Tool System
│   ├── memory/          # 记忆系统 / Memory
│   ├── checkpoint/      # 持久化 / Persistence
│   ├── workflow/        # DAG 引擎 + 动态规划器 / Engine + Planner
│   ├── planning/        # 任务规划 + 角色 + Hook / Planning
│   ├── kg/              # 知识图谱 / Knowledge Graph
│   ├── hypothesis/      # 假设生成 / Hypothesis
│   ├── self-improve/    # 自改进 / Self-Improvement
│   ├── skill/           # 技能子系统 / Skill Subsystem
│   ├── python/          # Python 桥接 / Bridge
│   ├── sandbox/         # WASM 沙箱 / Sandbox
│   ├── telemetry/       # 可观测性 / Observability
│   ├── server/          # Web 服务器 + 前端 / Server + Frontend
│   └── cli/             # CLI 入口 / Entry Point
├── skills/              # 142 技能定义 / Skill Definitions
├── docs/                # 设计文档 / Design Docs
├── scripts/             # 迁移脚本 / Migration Scripts
└── .env.example         # 配置模板 / Config Template
```

## 设计文档 / Design Documents

| 文档 Doc | 内容 Content |
|---------|-------------|
| [总体架构](docs/00-overall-architecture.md) | 技术选型、Crate 拆分、路线图 |
| [无状态 Agent Loop](docs/01-stateless-agent-loop.md) | 六大需求分析、核心 API |
| [记忆系统](docs/02-memory-system.md) | 四层架构、遗忘曲线、Consolidation |
| [自改进](docs/03-self-improvement.md) | 双层架构、Q-Router、Lifecycle Guard |
| [知识图谱与假设](docs/04-knowledge-graph-hypothesis.md) | KG 构建、GIVE、LLM 验证 |

## License

MIT
