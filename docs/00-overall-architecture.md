# Miniagent 总体架构设计

## 项目定位

Rust + Python 混合架构的 AI Agent 框架，专为长程科研任务设计。核心场景：
- 数百篇论文的阅读总结 → 知识图谱构建 → 链路预测 → 科学假设生成
- 数据集下载筛选 → 数据分析 → 统计检验 → 结论报告
- 软件工程任务（代码生成/审查/重构）

## 技术选型

| 维度 | 选择 | 理由 |
|------|------|------|
| 核心运行时 | Rust (Tokio) | 零成本抽象、内存安全、Cargo 生态 |
| 扩展层 | Python (PyO3) | 科学生态 (numpy/pandas/scipy/sklearn) |
| LLM 提供商 | DeepSeek Flash + Pro | Flash=简单任务, Pro=深度推理 |
| 异步运行时 | Tokio | 生态最丰富 |
| 数据库 | SQLite (rusqlite + WAL) | 嵌入式零配置 |
| 向量存储 | LanceDB + sqlite-vec | 本地优先 |
| 嵌入模型 | ONNX (ort) + BGE-m3 | 本地推理 |
| WASM 运行时 | Wasmtime | 安全审计 |
| 数据分析 | Polars + DataFusion | Rust 原生高性能 |
| 可观测性 | OpenTelemetry | 行业标准 |
| 部署形态 | Phase1: macOS CLI → Phase2: API Server → Phase3: Web UI |

## Crate 拆分

```
miniagent/
├── miniagent-core/           # 基础类型: ID, Budget(3维), Event, Error, Checkpoint
├── miniagent-provider/       # LLM Provider trait + DeepSeek Flash/Pro 实现
├── miniagent-agent/          # 无状态 Agent Loop
├── miniagent-tool/           # Tool trait + 内置工具集 (fs/shell/web/python)
├── miniagent-memory/         # 四层记忆系统
│   ├── working/              # L0: 工作记忆 (上下文窗口管理)
│   ├── episodic/             # L1: 情景记忆 (SQLite FTS5 结构化记忆)
│   ├── semantic/             # L2: 语义记忆 (LanceDB 向量库 + KG 存储)
│   └── procedural/           # L3: 技能记忆 (Skill Repo + Meta-Skill)
├── miniagent-workflow/       # DAG 工作流引擎
├── miniagent-checkpoint/     # 检查点/恢复
├── miniagent-kg/             # 知识图谱构建 + Embedding + 链路预测
├── miniagent-hypothesis/     # 假设生成 + 排序 + 实验设计
├── miniagent-self-improve/   # 双层自改进系统
│   ├── online/               # Step-Level Reflection + Q-Learning Router + Lifecycle Guard
│   └── offline/              # Experience Graph + Skill Lifecycle + Sleeptime Consolidation
├── miniagent-python/         # PyO3 桥接层
├── miniagent-sandbox/        # WASM 安全沙箱
├── miniagent-policy/         # 安全策略引擎
├── miniagent-telemetry/      # OpenTelemetry 可观测性
├── miniagent-cli/            # macOS CLI 入口
├── miniagent-server/         # (后期) API 服务器
└── miniagent-web/            # (后期) 前端页面
```

## 三层架构

```
┌─────────────────────────────────────────────────────────────────┐
│                    Orchestration Layer (编排层)                   │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐   │
│  │ Project  │  │ Workflow │  │ Scheduler│  │ SubAgent      │   │
│  │ Manager  │  │ Engine   │  │ (Cron)   │  │ Spawner       │   │
│  └──────────┘  └──────────┘  └──────────┘  └──────────────┘   │
├─────────────────────────────────────────────────────────────────┤
│                    Agent Runtime Layer (运行时层)                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │  Stateless Agent Loop                                       │ │
│  │  Context Assembly → LLM Call → Parse → Tool Dispatch         │ │
│  │  ↑ Checkpoint/Restore at every step boundary                 │ │
│  └────────────────────────────────────────────────────────────┘ │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐   │
│  │ Provider │  │ Tool     │  │ Policy   │  │ Memory       │   │
│  │ Router   │  │ Registry │  │ Engine   │  │ Manager      │   │
│  └──────────┘  └──────────┘  └──────────┘  └──────────────┘   │
├─────────────────────────────────────────────────────────────────┤
│                    Infrastructure Layer (基础设施层)              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐   │
│  │ Storage  │  │ Vector   │  │ WASM     │  │ Telemetry    │   │
│  │ (SQLite) │  │ (LanceDB)│  │ Sandbox  │  │ (OTEL)       │   │
│  └──────────┘  └──────────┘  └──────────┘  └──────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## 核心设计原则

1. **无状态 Agent Loop** — 调用者拥有历史，Agent 返回 delta。支持 fork/retry/resume 的天然组合
2. **最少机制 + 最大卫生** — 借鉴 Ratchet 的核心洞见：加机制不如管好已有的
3. **结构化优于自由文本** — 论文→KG 三元组，经验→Experience Graph，技能→SKILL.md
4. **双层学习** — Layer1 在线快速改进（Flash），Layer2 离线深度优化（Pro + Sleeptime）
5. **Rust 负责执行 + Python 负责科学** — 核心路径零开销，科学生态无缝集成

## 实现路线图

| 阶段 | 内容 | 关键产出 |
|------|------|---------|
| Phase 1 | core + provider + agent + cli | 可运行的基础 Agent CLI |
| Phase 2 | tool + memory + checkpoint | 工具执行 + 四层记忆 + 断点续传 |
| Phase 3 | workflow + kg + hypothesis | DAG 工作流 + KG 构建 + 链路预测 + 假设生成 |
| Phase 4 | self-improve + python + sandbox | 自改进 + Python 生态 + WASM 沙箱 |
| Phase 5 | telemetry + server | 可观测性 + API 服务器 |
| Phase 6 | web | 前端页面 |
