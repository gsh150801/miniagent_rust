# miniagent 系统架构优化建议

> 基于对 19 个 crate 源码的全面审查，按严重程度排序。

## A. 严重问题（应优先修复）

### A1. API Key 以 `String` 到处传递 — 安全隐患 ✅ 已完成

**问题位置**：`loop-pipeline/src/stage.rs:13`、`pipeline.rs:25`、`explore.rs:93-94`、`dispatch.rs:381-382`、`workflow/src/builder.rs:51` 等几十处。

整个调用链都在以 `api_key: String`（或 `&str`）裸传递 DeepSeek API Key，每层都 `clone()`。任何一次 panic 的 backtrace 或日志误打印都可能泄露密钥。

**建议**：
```rust
#[derive(Clone)]
pub struct ApiKey(Arc<str>);
impl fmt::Debug for ApiKey { // always "ApiKey(****)" }
```
所有函数签名改为 `api_key: &ApiKey`，日志和 `Debug` 自动脱敏。

### A2. Loop Pipeline 每个 Task 重新构建 Agent/Provider/ToolExecutor — 严重性能浪费

**问题位置**：`dispatch.rs:379-385`、`explore.rs:93-97`

每个 wave 的每个 task 都重建一遍 HTTP Client、ToolRegistry、所有工具实例。在 5 轮 loop × 8 tasks 的场景下相当于重建 40+ 次。

**建议**：在 `StageContext` 中持有一个 `Arc<Agent>`，dispatch 时 `agent.clone()`（Arc 浅拷贝）。`DeepSeekClient` 内部复用 `reqwest::Client` 连接池。

### A3. `eprintln!` 作为日志系统 — 违反可观测性原则

**问题位置**：100+ 处 `eprintln!`，分布在几乎所有关键路径。项目已集成 `telemetry` crate（structured JSON tracing），但实际全部走 `eprintln!`。

**建议**：全部替换为 `tracing::info!/warn!/error!/debug!`。

### A4. Loop Pipeline 的"无进展检测"逻辑有 off-by-one 缺陷 ✅ 已完成

**问题位置**：`pipeline.rs:104-117`

用 `len()-3` 索引比较"3 轮前的进度"，硬编码 `7` 是 magic number，`max_loops` 设为 5 时永远触发不了。

**建议**：改为显式追踪 `no_progress_streak: usize` 计数器。

### A5. `Plan` 阶段的"强制拆分"用启发式字符串匹配 — 脆弱 ✅ 已完成

**问题位置**：`plan.rs:156-211`

用字符串包含判断 + 逗号分词来"补救" LLM 没拆分的情况。

**建议**：删除启发式补救。改为 prompt few-shot + JSON Schema 校验，不合法直接重试。

---

## B. 架构级问题

### B1. 三套互不相通的"规划/编排"子系统 — 重复且割裂

| 系统 | 位置 | 角色定义 | 编排模型 |
|------|------|---------|---------|
| Workflow DAG | `workflow/` | StageHandler（6种） | 拓扑排序 wave |
| Loop Pipeline | `loop-pipeline/` | AgentRoleType（7种） | 循环 E→P→D→E→R |
| Planning/StateGraph | `planning/` | AgentRole（13种）+ AgentProfile | 条件图 + ControlShell |

三套有各自的角色集合、上下文类型、输出类型。

**建议**：统一为"单一编排核心 + 多种执行模式"，以 `StateGraph` 为唯一图执行引擎。

### B2. `StageMessage` 是死代码 — Loop Pipeline 实际未使用消息路由 ✅ 已完成

每个 stage 构造 `StageMessage` 但主循环 `pipeline.rs` 从未读取。repair 阶段精心设计的路由消息全部无效。

**建议**：要么删除（YAGNI），要么真正实现消息分发。

### B3. Workflow Engine 不支持真正的并行 — 与文档承诺不符 ✅ 已完成

**问题位置**：`workflow/src/engine.rs:131-173`

`topological_order()` 返回扁平 `Vec<usize>`，不是分 wave。`Stage.parallel` 字段从未被读取。

**建议**：改为返回 `Vec<Vec<usize>>`（waves），每个 wave 内 `join_all` 并发执行。

### B4. 历史压缩的"摘要丢失"风险

**问题位置**：`agent/src/lib.rs:388-460`

工具调用结果被当纯文本摘要；摘要用 Flash 模型（高认知负荷任务）；`keep_recent=5` 硬编码。

**建议**：摘要前先结构化提取；摘要改用 Pro 模型；`keep_recent` 改为可配置。

### B5. `tools_for_role` 是"软约束" — 实际工具未按角色过滤 ✅ 已完成

**问题位置**：`loop-pipeline/src/prompts.rs:20-33` vs `dispatch.rs:383-385`

无论角色是什么，agent 都拿到全部工具。

**建议**：在 `Agent::new` 后用 `registry.retain()` 真正过滤。

---

## C. 工程质量问题

### C1. JSON 解析容错 — 重复实现 3 遍 ✅ 已完成

`planning/src/roles/mod.rs:258`、`workflow/src/stages.rs:1006`、`loop-pipeline/src/explore.rs:183` 三份独立实现。

**建议**：抽到 `core/src/json_util.rs` 统一实现。

### C2. 错误处理：`AgentError::provider(String)` 滥用 ✅ 已完成

大量业务错误塞进"Provider 错误"变体，破坏类型安全。

**建议**：新增 `InvalidState(String)` 变体，或统一用 `anyhow::Error`。

### C3. 测试与生产代码耦合 — 测试用 `eprintln!` 而非断言

`loop-pipeline/tests/integration_test.rs` 大量 `eprintln!("✅...")`，真正的 `assert!` 稀疏。

**建议**：删除所有 `eprintln!("✅...")`，只保留 `assert!`。

### C4. `Magic Number` 与配置缺失 ✅ 已完成

`96_000`、`12_000`、`5`、`7`、`500`、`3_000_000` 等散布在代码中。

**建议**：统一到 `RunContext` 或 `PipelineConfig`，支持 `.env` 覆盖。

### C5. `Workflow Builder` 的 `max_tokens` 上下限冲突 ✅ 已完成

`builder.rs:59` 设 `10_000_000`，但 `agent/lib.rs:118` 限制 `.min(393216)`。10M 永远被压缩。

**建议**：统一为单一来源。

---

## D. 设计改进建议

### D1. Loop Pipeline 缺少"早停成本控制" ✅ 已完成

只看"任务是否完成"，不看"投入产出比"。

**建议**：引入"成本-进度比"判断，单轮 token > 30K 且进度 < 10% 时提前终止。

### D2. Critic/Judge 的 3-Party Review 对所有任务一刀切 ✅ 已完成

简单任务被过度审查。

**建议**：根据 `difficulty` 分级——`simple` 跳过，`medium` 只跑 Critic，`hard` 才跑完整三方。

### D3. EventStream 的 `role_dependencies` 硬编码 ✅ 已完成

**问题位置**：`event_stream.rs:227-243`、`context_manager.rs:228-245`

两处手工维护 13 角色映射，且**实现不一致**（如 `context_manager` 的 planner 缺 `evaluator` 依赖、`evaluator` 缺 `synthesizer`、`opponent` 含 `judge` 而 event_stream 不含）。

**建议**：移到 `AgentProfile` 的 `depends_on_agents` 字段。

### D4. Knowledge Graph / Hypothesis 与 Agent 主循环脱节 ✅ 已完成

KG/Hypothesis 只在 CLI `research` 命令中独立运行。

**建议**：封装为 `Tool`（`kg_query`、`hypothesis_suggest`），让 Agent 主动调用。

### D5. 缺少 Agent 自我反思的"经验沉淀" ✅ 已完成

`self-improve` crate 接好了线但没通电——`run_with_loop` 中没有调用 SelfImprover。

**建议**：在工具执行后插入 `improver.reflect_on_step()`。

---

## E. 推荐的优化优先级

| 优先级 | 建议 | 预期收益 | 状态 |
|--------|------|---------|------|
| **P0** | A1-A3 (安全/性能/可观测性) | 基础 | ✅ 已完成 |
| **P1** | A4-A5, B3, B5 (容错/并行/过滤) | 鲁棒性/性能/质量 | ✅ 已完成 |
| **P2** | B2, C1-C2, C4-C5 (工程清理) | 代码质量 | ✅ 已完成 |
| **P3** | D1 (早停成本控制) | 成本节省 | ✅ 已完成 |
| **P3** | D2 (分级审查) | 效率/成本 | ✅ 已完成 |
| **P3** | D5 (SelfImprover 接通) | 自改进 | ✅ 已完成 |
| **P2** | B1 (统一编排核心) | 可维护性 | ⬜ 待实施（大重构，单独立项） |
| **P2** | C3 (测试 eprintln! 清理) | 代码质量 | ⬜ 低优先级 |
| **P3** | D3 (role_dependencies 数据驱动) | 可维护性 | ✅ 已完成（`context_dependencies_of` 数据驱动，profile 优先、静态表兜底） |
| **P3** | D4 (KG/Hypothesis 封装为 Tool) | 功能完善 | ✅ 已完成（`defaults_with_kg()` 接入 `kg_query`/`kg_add`/`hypothesis_suggest`） |

> 第二轮（2026-06-16）补充：核实发现 D3/D4 早在前序工作中已落地，此处状态据此更正。
> 详见 `optimization-changelog.md` 末尾「第二轮优化」与 `optimization-suggestions2.md` 的状态标记。
