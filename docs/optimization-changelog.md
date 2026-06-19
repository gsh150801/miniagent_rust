# miniagent 优化改动记录

> 按"安全、确保性能、融合"原则持续推进。每项改动记录实施细节、影响范围和验证结果。

---

## A1. ApiKey 统一配置（✅ 已完成）

**日期**：2026-06-15

### 目标
- API Key 不再以 `String` 到处传递，防止日志/panic 泄露
- 所有 API Key 和设置参数统一从 `.env` 文件读取

### 新增文件
| 文件 | 用途 |
|------|------|
| `crates/core/src/secrets.rs` | `ApiKey` 类型：`Debug`/`Display` 自动脱敏，基于 `Arc<str>` cheap clone，唯一访问点 `as_str()` |
| `crates/core/src/settings.rs` | `AppConfig` 统一配置：`load()` 调用 `dotenvy::dotenv()` 后一次性读取所有参数；`require_deepseek_key()` fail-fast |

### 修改文件（15 个）

| 层 | 文件 | 改动 |
|----|------|------|
| Core | `Cargo.toml` / `lib.rs` | 新增 `dotenvy` 依赖 + 导出 `secrets` / `settings` 模块 |
| Provider | `deepseek.rs` | `DeepSeekClient::new` / `DeepSeekFlash::new` / `DeepSeekPro::new` 签名 `impl Into<String>` → `&ApiKey` |
| Loop Pipeline | `stage.rs` / `pipeline.rs` | `StageContext.api_key: String` → `config: Arc<AppConfig>`；`LoopPipeline::run` 参数改为 `Arc<AppConfig>` |
| | `explore.rs` / `plan.rs` / `dispatch.rs` / `evaluate.rs` / `repair.rs` | 所有 `ctx.api_key` → `ctx.config.require_deepseek_key()` |
| Workflow | `builder.rs` | `WorkflowBuilder` 从 `api_key + max_iterations + max_tokens` 改为 `config: Arc<AppConfig>` |
| Server | `lib.rs` / `state.rs` / `routes.rs` / `bin/miniagent-server.rs` | `ServerConfig` 改为持有 `Arc<AppConfig>` |
| CLI | `main.rs` | 删除 `EnvConfig` 结构体，完全由 `AppConfig` 替代；所有命令函数接收 `&Arc<AppConfig>` |
| Config | `.env.example` | 完整列出所有可配置参数 |

### 验证
- `cargo build --bins` ✅ 通过
- `cargo test -p miniagent-core` ✅ 10/10 通过（7 个 ApiKey 测试 + 3 个 AppConfig 测试）
- `cargo test -p miniagent-loop-pipeline --lib` ✅ 14/14 通过

### 数据流
```
.env → AppConfig::load() → Arc<AppConfig>
                              ↓
         ┌────────────────────┼────────────────────┐
         ▼                    ▼                    ▼
   CLI commands        Server AppState      LoopPipeline
   WorkflowBuilder     routes.rs            StageContext
```

---

## A2. Agent/Provider 复用（✅ 已完成）

**日期**：2026-06-15

### 目标
- Loop Pipeline 的 dispatch/explore/plan/evaluate/repair 阶段不再每次重建 Agent/Provider/ToolExecutor
- 预期性能提升：消除每轮 loop × 每 task 重复构建 HTTP Client（含连接池）、ToolRegistry（含 14 个工具实例）的开销

### 改动

**`stage.rs`**：`StageContext` 新增 `agent: Arc<Agent>` 字段。`build_agent()` 在 `StageContext::new` 时构建一次，所有 stage 共享。

**`explore.rs`**：删除每次循环的 Agent 构建（~6 行），改为 `let agent = &ctx.agent;`

**`dispatch.rs`**：
- 每个 wave 的 task spawn 从重建完整 Agent 改为 `ctx.agent.clone()`（Arc 浅拷贝）
- `run_critic` / `run_judge` 参数从 `&ApiKey` 改为 `&dyn LlmProvider`，使用 `ctx.agent.router().flash()/pro()` 获取共享 provider

**`plan.rs` / `evaluate.rs` / `repair.rs`**：直接 LLM 调用从 `DeepSeekFlash::new(key)` / `DeepSeekPro::new(key)` 改为 `ctx.agent.router().flash()` / `.pro()`

### 性能影响
- 5 轮 loop × 8 tasks 场景：从 40+ 次 Agent 构建降至 1 次
- 消除 40+ 次 `reqwest::Client` 创建（含 TLS 初始化和连接池建立）
- 消除 40+ 次 `ToolRegistry::defaults()` 初始化（14 个工具实例 × 40 = 560 次工具构造）

### 验证
- `cargo build` ✅ 通过
- `cargo test -p miniagent-loop-pipeline --lib` ✅ 14/14 通过

---

## A3. eprintln! → tracing（✅ 已完成）

**日期**：2026-06-15

### 目标
- 核心库（loop-pipeline、agent、workflow、planning）中的 `eprintln!` 全部替换为 `tracing` 宏
- CLI 保持 `println!/eprintln!`（面向终端用户，合理）
- 使库代码获得结构化日志、级别过滤、字段标注

### 改动范围（55 处替换）

| Crate | 文件 | 替换数 | 典型替换 |
|-------|------|--------|---------|
| loop-pipeline | `pipeline.rs` | 15 | 进度信息 → `tracing::info!`；无进展警告 → `tracing::warn!` |
| | `dispatch.rs` | 8 | task 完成/失败 → `tracing::info!/warn!`（含 `task_id` 字段）；Judge 结果 → `tracing::info!/warn!` |
| | `explore.rs` | 1 | Explorer 错误 → `tracing::warn!` |
| | `plan.rs` | 3 | JSON 解析失败 → `tracing::warn!`；计划列表 → `tracing::debug!` |
| | `evaluate.rs` | 1 | JSON 解析失败 → `tracing::warn!` |
| | `repair.rs` | 1 | 修复分析 → `tracing::info!` |
| agent | `lib.rs` | 2 | Hook 拦截 → `tracing::warn!` |
| workflow | `stages.rs` | 11 | Worker 进度 → `tracing::info!`；Planner 回退 → `tracing::warn!`；技能匹配 → `tracing::debug!` |
| planning | `state_graph.rs` / `plan.rs` | 3 | HITL → `tracing::info!`；wave 进度 → `tracing::debug!` |

### 结构化日志示例
```rust
// 旧: eprintln!("       ❌ {} failed: {:?}", result.task_id, ...);
// 新: tracing::warn!(task_id = %result.task_id, error = ?..., "task failed");
```
输出形如：`WARN task_id=task_3 error="API timeout" task failed`

### 验证
- 核心库 `eprintln!` 计数：0 ✅
- `cargo build --bins` ✅ 通过
- `cargo test -p miniagent-core -p miniagent-loop-pipeline --lib` ✅ 全部通过

---

## P2 批次：B2 + C1 + C2 + C4 + C5（✅ 已完成）

**日期**：2026-06-16

### C1. 统一 JSON 解析容错

**新增文件**：`crates/core/src/json_util.rs`

三个统一函数：
| 函数 | 用途 | 替换位置 |
|------|------|---------|
| `strip_markdown_fences(s)` | 去除 ```` ```json ```` 围栏 | `workflow/stages.rs`、`plan.rs`、`evaluate.rs`、`repair.rs`、`dispatch.rs` × 2 |
| `fix_truncated_json(s)` | 闭合截断的字符串/大括号/方括号 | `workflow/stages.rs` |
| `extract_json_object(text)` | 提取最后一个顶层 JSON 对象 | `loop-pipeline/explore.rs` |
| `extract_and_repair(text)` | 一步到位：strip → fix → extract | `loop-pipeline/explore.rs` |

13 个单元测试覆盖围栏去除、截断修复、嵌套提取、完整管线。

**消除的重复**：2 份 `strip_markdown_fences` + 1 份 `fix_truncated_json` + 1 份 `extract_json` = 4 处独立实现 → 统一到 1 个模块。

### B2. StageMessage 消除死代码

**问题**：每个 stage 精心构造 `StageMessage` 路由消息，但 `pipeline.rs` 从未读取 `output.new_messages`。

**修复**：`StageContext` 新增 `collect_messages()` 方法。Pipeline 主循环每个阶段执行后调用此方法：
- 通过 `tracing::debug!` 记录每条消息的 `from_stage → to_stage` 路由（可观测性）
- 累积到 `ctx.messages`（可用于后续调试）

### C2. AgentError 类型安全

**改动**：
- `AgentError` 新增 `InvalidState(String)` 变体 + `invalid_state()` 构造器
- 替换 3 处 `AgentError::provider()` 误用：
  - `dispatch.rs`：`"No plan available for dispatch"` → `invalid_state()`
  - `evaluate.rs`：`"No plan for evaluation"` → `invalid_state()`
  - `plan.rs`：`"Plan parse failed"` → `invalid_state()`
- `provider/deepseek.rs` 中的 6 处 `provider()` 调用保留（真正的 HTTP/响应错误）

### C4. Magic Numbers 提取

**`agent/src/lib.rs`** 新增 2 个命名常量：
| 常量 | 值 | 替换的内联 magic number |
|------|-----|----------------------|
| `KEEP_RECENT_MSGS` | 5 | `trim_and_summarize_history` 中的 `5usize` |
| `MAX_CONSECUTIVE_ERRORS` | 3 | `run_with_loop` 中的 `consecutive_errors >= 3` |

### C5. max_tokens 冲突验证

经审查，此问题在 A1（AppConfig 统一配置）中已修复：
- `AppConfig.max_tokens` 默认 `393_216`（来自 `.env` 的 `MAX_TOKENS`）
- `builder.rs` 使用 `self.config.max_tokens`（不再硬编码 `10_000_000`）
- `agent/lib.rs` 的 `.min(393216)` 限制与配置一致

### 验证
- `cargo test -p miniagent-core --lib` ✅ 23/23（含 13 个新 json_util 测试）
- `cargo test -p miniagent-loop-pipeline --lib` ✅ 14/14
- `cargo test -p miniagent-loop-pipeline --test integration_test` (offline) ✅ 25/25
- `cargo build --bins` ✅

---

## A4. Loop Pipeline 无进展检测修复（✅ 已完成）

**日期**：2026-06-16

### 目标
- 替换 magic number `7` + `evaluations.len()-3` 的 off-by-one 缺陷
- 引入显式 `no_progress_streak` 计数器，与 `max_loops` 无关地追踪停滞

### 改动

**`types.rs`**：`PipelineState` 新增 `no_progress_streak: usize` 字段（`#[serde(default)]`）。

**`pipeline.rs`**：删除 `loop_count >= 7` + `evaluations[len-3]` 比较。改为：
- 每轮 Evaluate 后，将当前 `overall_progress_pct` 与上一轮比较
- 进度提升 → streak 重置为 0；停滞 → streak += 1
- `streak >= NO_PROGRESS_LIMIT (3)` 且 `progress < 100%` 且有失败任务 → 强制终止

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| max_loops=5, 3轮无进展 | ❌ 永远不触发（需 loop_count≥7） | ✅ 第4轮触发 |
| max_loops=10, 持续进展 | — | streak=0，正常运行 |
| max_loops=10, 卡在66% | 需等到 loop 7 | 第4轮即终止 |

### 验证
- `test_e2e_no_progress_safety_stops_infinite_loop`：4 轮卡 66% → streak=3 → 触发 ✅
- `test_multi_loop_no_progress_safety_stop`：进展场景 streak=0（不触发）；停滞场景 streak=3（触发） ✅

---

## A5. 删除启发式强制拆分（✅ 已完成）

**日期**：2026-06-16

### 目标
- 删除 `plan.rs` 中基于字符串匹配的"强制拆分"逻辑（逗号分词、中文标点检测等）
- 改为 prompt few-shot + 一次 LLM 重试

### 改动

**`plan.rs`** 重构：
- 删除 56 行启发式 force-decompose 代码（`has_multiple_topics`、`topics.split()` 等）
- `build_plan_prompt()` 新增 3 个 few-shot 示例（多主题研究、代码管线、原子任务）
- `build_plan_prompt_retry()` 专门的重试 prompt，强调"至少 3 个子任务"
- `try_generate_plan()` 抽取为独立函数，支持首次 + 重试复用
- 重试逻辑：首次返回 1 个任务 + `needs_decomposition=true` → 用更强 prompt 重试一次

### 删除的代码模式
```rust
// 删除前：脆弱的字符串匹配
let has_multiple_topics = task.contains("1)") || task.contains("1.")
    || task.contains("and") || task.contains("和") || task.contains("、") ...;
let topics: Vec<&str> = task.split(|c| c == ',' || c == ';')...
```

### 验证
- `cargo build -p miniagent-loop-pipeline` ✅ 通过
- 14 个单元测试 + 25 个离线集成测试 ✅ 全部通过

---

## B5. 工具按角色过滤（✅ 已完成）

**日期**：2026-06-16

### 目标
- `tools_for_role()` 不再只是"软约束"（提示中提及但 LLM 实际看到全部工具）
- 按 `assigned_role` 真正过滤暴露给 LLM 的工具定义

### 改动

**`agent/src/context.rs`**：`RunContext` 新增 `allowed_tools: Option<Vec<String>>` 字段 + `with_allowed_tools()` builder。

**`agent/src/lib.rs`**：`Agent::run()` 中收集 tool definitions 时，如果 `context.allowed_tools` 已设置，使用 `retain()` 过滤：
```rust
if let Some(ref allowed) = context.allowed_tools {
    defs.retain(|d| allowed.iter().any(|a| a == &d.name));
}
```

**`loop-pipeline/src/dispatch.rs`**：每个 task spawn 时设置 `allowed_tools`：
```rust
let allowed: Vec<String> = tools_for_role(&task.assigned_role)
    .iter().map(|s| s.to_string()).collect();
let context = RunContext::new(&system)
    .with_allowed_tools(allowed);
```

**`loop-pipeline/src/explore.rs`**：Explorer 也设置 `allowed_tools` 为 `tools_for_role("explorer")`。

### 安全设计
- 过滤只作用于 LLM 看到的 tool definitions（减少干扰、防止越权使用）
- ToolExecutor 仍持有全部工具注册（不影响内部工具调用逻辑）
- 如果 LLM 幻觉调用不存在的工具名，executor 返回 `ToolNotFound` 错误

### 验证
- `cargo build -p miniagent-agent -p miniagent-loop-pipeline` ✅ 通过
- 14 单元 + 25 集成测试 ✅ 全部通过

---

## B3. Workflow 真并行（✅ 已完成 / 验证）

**日期**：2026-06-16

### 状态
经审查，Workflow Engine **已实现** wave-based 并行执行：
- `topological_waves()` 返回 `Vec<Vec<usize>>`（每波是可并行的 stage 组）
- `run_inner()` 对每波使用 `futures::future::join_all()` 并发执行
- 单 stage 波退化为顺序执行（无并发开销）

此次仅更新文档状态，无需代码改动。

---

## P3 批次：D1 + D2 + D5（✅ 已完成）

**日期**：2026-06-16

### D2. Critic/Judge 分级审查

**问题**：所有成功任务无论复杂度都跑完整 3-party review（Critic Flash + Judge Pro），简单任务浪费 2 次 LLM 调用。

**改动**：`dispatch.rs` 的 review 循环按 `difficulty` 字段分级：

| difficulty | Critic (Flash) | Judge (Pro) | 节省 |
|------------|---------------|-------------|------|
| `simple` | ❌ 跳过 | ❌ 跳过 | 2 次调用 |
| `medium` | ✅ 仅反馈 | ❌ 自动通过 | 1 次调用 |
| `hard` / 未知 | ✅ | ✅ | 0（完整审查） |

**效果**：8 个任务中若有 3 个 simple + 3 个 medium + 2 个 hard，从 16 次 LLM 调用降至 7 次（节省 56%）。

### D1. Loop Pipeline 早停成本控制

**问题**：Pipeline 只看"任务是否完成"和"进展停滞"，不看"投入产出比"。

**改动**：
- `PipelineState` 新增 `total_tokens_used: usize` 字段（累积所有轮次的 token）
- `pipeline.rs` 每轮 Evaluate 后计算当轮 token 消耗：

```rust
const COST_TOKEN_THRESHOLD: usize = 30_000;
const MIN_PROGRESS_PCT: f64 = 10.0;
if loop_tokens > COST_TOKEN_THRESHOLD && progress < MIN_PROGRESS_PCT && loop_count > 0 {
    // 强制终止
}
```

**效果**：当单轮消耗 > 30K tokens 但进度 < 10% 时立即终止，避免在高成本低收益的 pipeline 上继续燃烧预算。

### D5. SelfImprover 接通

**问题**：`self-improve` crate 的 `SelfImprover` 已在 `Agent` 中持有（`Option<Arc<Mutex<SelfImprover>>>`），但 `run_with_loop` 从未调用任何方法。

**改动**：`agent/src/lib.rs` 的 `run_with_loop` 新增两处集成：

1. **工具可靠性追踪**（每次工具执行后）：
   ```rust
   for (call_id, output) in &results {
       if is_error { imp.on_tool_failure(tool_name, &output.content); }
       else { imp.on_tool_success(tool_name, latency); }
   }
   ```

2. **步骤反思**（episode 结束时）：
   ```rust
   let reflection = imp.on_step(history, &sm_delta, cancel).await;
   // → 记录 self_score 和 error_detected 到 ExperienceGraph
   ```

**类型转换**：Agent 的 `AgentDelta` 与 SelfImprover 的 `integrator::AgentDelta` 是不同类型（同字段），通过构造转换。

**效果**：
- `ToolReliabilityTracker` 现在实际记录工具成功率/延迟
- `StepReflector` 在每个 episode 结束时产生 `StepReflection`（自我评分 + 错误检测）
- `ExperienceGraph` 自动记录失败模式供后续 `find_relevant_experiences()` 查询

### 验证
- `cargo test -p miniagent-core --lib` ✅ 23/23
- `cargo test -p miniagent-loop-pipeline --lib` ✅ 14/14
- `cargo test -p miniagent-loop-pipeline --test integration_test` (offline) ✅ 25/25
- `cargo build --bins` ✅

---

## 第二轮优化：架构清理 + StateGraph 动态调度（✅ 已完成）

**日期**：2026-06-16

本轮结合 `optimization-suggestions.md`（D3/D4）与 `optimization-suggestions2.md`（#2/#8/#9/#14），
在核实代码实况后推进。**核实阶段纠正了三处文档诊断的不准确**（见下文"诊断修正"）。

### 诊断修正（核实阶段发现）

| 文档原诊断 | 代码实况 |
|-----------|---------|
| D4「KG/Hypothesis 封装为 Tool」⬜ 待实施 | ✅ **已完成**：`tool/src/tools/mod.rs::defaults_with_kg()` 已接入 `kg_query`/`kg_add`/`hypothesis_suggest`，工具实现完整且有测试 |
| kg_tools Send 错误（旧 Windows 日志） | ✅ **已不存在**：`lock_graph` 现用 `tokio::sync::Mutex`，Guard 可跨 await 持有 |
| suggestions2 #9「并行 step_outputs/messages 丢失」 | ⚠️ **诊断不准**：`step_outputs`/`messages` 实际由主循环从 `NodeOutput.content` 正确回写；真正丢失的是并行分支的 **TodoAttention 进度** |

---

### 批次 1：低风险纯清理

#### 1.1 #2 ModelTier 重复定义 → 统一到 miniagent-core（✅）

**问题**：`ModelTier { Flash, Pro }` 在 `state_graph.rs` 和 `agent_profile.rs` 各定义一份，
是同名不同路径的两个独立类型，无法互相赋值（44 处引用、零转换逻辑）。

**改动**：
- **新增** `crates/core/src/model_tier.rs`（+ `lib.rs` 导出 `pub use model_tier::ModelTier`），含 2 个单测
- **删除** planning 两处重复定义；`planning/src/lib.rs` 改为 `pub use miniagent_core::ModelTier`
- **修正深层路径**（6 处）：`research/alzheimers.rs`、`research/scheduler.rs`、`cli/main.rs`、`tests/integration.rs` 的 `miniagent_planning::state_graph::ModelTier` → `miniagent_planning::ModelTier`

#### 1.2 #14 死代码清理（✅）

- **CLI**：删除未用函数 `mask_key`、`extract_filename_from_prompt`；精简未用 import
  （`AgentStage/CriticStage/SynthesizerStage/Stage`）；未用变量 `prompt_for_file` → `_prompt_for_file`
  → CLI 的 5 个 warning 清零
- **state_graph**：移除 `CompiledGraph.conditional_edges` 和 `route()` 的 `#[allow(dead_code)]`
  （批次 2.1 真正启用了它们）

---

### 批次 2：StateGraph 动态调度 + 并行 TodoAttention 合并

#### 2.1 #8 条件边运行时实现（完全动态调度）（✅）

**问题**：`ConditionalEdge`/`route()`/`NodeOutput.next` 定义完整但全是死代码——
`execute()` 纯按编译期 `node_order` 静态波次遍历，节点执行后从不调用 `route()`，
图无法在运行时根据输出做条件分支。

**改动**（`state_graph.rs`）：
- `CompiledGraph` 新增 `entry_point` 字段（编译期确定的入口，替代 `node_order[0]`——后者在含孤立子节点的图中顺序不确定）
- 重写 `execute()` 为**动态调度模型**：维护待执行队列从 `entry_point` 出发，每执行完一个节点后按优先级决定后继：
  1. **`output.next`**（节点自决下一跳，最高优先级）
  2. **`route(node, &state)`**（条件边谓词命中→route target；否则→default）
  3. **静态边后继**（退化为原拓扑行为）
- `route()` 改为 `pub`，去掉 `#[allow(dead_code)]`

**核心保证——向后兼容**：无 `add_conditional` 且节点不返回 `output.next` 的图，
后继 == 原静态边后继，节点执行顺序与改造前等价（由 `execute_static_graph_preserves_order_and_outputs` 测试守护）。

**语义权衡（完全动态调度的代价）**：顶层多个无依赖节点的"静态并行"（原 wave.len()>1）不再并行——
真正的并行现在只来自 `GraphNode::Parallel` 节点（其子节点在 `execute_node` 内部并发展开）。
这换来的是条件边/动态路由的正确语义。如需顶层并行，显式用 `add_parallel`。

**新增测试**（6 个）：
| 测试 | 验证 |
|------|------|
| `route_static_edges_when_no_conditional` | 静态图 route() 返回静态后继 |
| `execute_static_graph_preserves_order_and_outputs` | **等价性**：静态图动态调度后拓扑序与输出不变 |
| `conditional_edge_routes_to_predicate_target` | 谓词命中走 route 分支，default 不执行 |
| `conditional_edge_falls_back_to_default` | 谓词全不命中走 default，route 分支不执行 |
| `output_next_overrides_conditional_route` | `output.next` 优先级高于静态边 |
| `parallel_wave_merges_todo_progress` | Parallel 节点聚合子节点输出 |

#### 2.2 #9 修正：并行分支 TodoAttention 进度合并（✅）

**问题（修正后的真实诊断）**：并行分支各持 `todo.clone()`，分支内的
`complete`/`block`/`start`/`add` 修改从未回传主 `TodoAttention`，导致并行 wave 的任务进度丢失。

**改动**：
- **`todo_attention.rs`** 新增 `merge_from(&mut self, other: &TodoAttention)`：按 `id` 做并集合并，
  共有项采用"更靠后"的状态（终态 Completed/Blocked 优先），other 独有项追加；含 `status_rank` 辅助函数
- **`state_graph.rs`** `execute_node` 的 Parallel 分支：子节点返回类型加 `TodoAttention`，
  join 后 `todo.merge_from(&sub_todo)` 合并回主 todo

**新增测试**（3 个，`todo_attention::tests`）：
- `merge_from_adopts_more_advanced_status`：分支完成的任务在主 todo 中也完成
- `merge_from_appends_new_items_from_branch`：分支新增的任务追加到主 todo
- `merge_from_does_not_downgrade_status`：已完成的任务不会被旧分支状态降级

#### 附带修复：集成测试 ApiKey 遗留（✅）

**问题**：`crates/planning/tests/integration.rs` 的 `load_api_key()` 返回 `String`，但 A1 改造后
`DeepSeekFlash::new`/`DeepSeekPro::new` 需要 `&ApiKey`——这些 `#[ignore]` 的端到端测试自 A1 后无法编译。
（预先存在，非本轮引入；修复后集成测试重新可编译运行）

**改动**：`load_api_key()` 返回类型 `String` → `miniagent_core::secrets::ApiKey`，所有调用点 `&api_key` 自动匹配。

---

### 验证（全量）

| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| `cargo test --workspace`（全量） | ✅ **172 通过 / 0 失败 / 5 ignored**（ignored = 需真实 LLM 的端到端测试） |
| 新增测试 | core 2 + planning state_graph 6 + planning todo_attention 3 = **11 个新测试** |
| CLI warnings | 5 → **0** |
| 集成测试编译 | 恢复可编译（修复 ApiKey 遗留） |

### 数据流（StateGraph 动态调度）

```
entry_point ──► pending 队列
                  │
                  ▼ pop
              execute_node(node)
                  │
                  ▼ 写回 step_outputs/messages
              决定后继:
                output.next? ──► [next]           (节点自决，最高优先级)
                else route(node, &state)?
                  ├─ conditional_edge 命中 ──► [route target]
                  ├─ 全不命中 ──► [default]
                  └─ 无条件边 ──► [静态后继]      (退化，等价于改造前)
                  │
                  ▼ 未执行的加入 pending
              (Parallel 节点: 子节点并发 + todo.merge_from 合并)
```

### 刻意不做（本轮排除，附理由）

- **B1 统一编排核心**（workflow/loop-pipeline/planning 三套子系统合并）：大重构，回归面大，与"安全"原则冲突，单独立项
- **#16 字符串角色名完全强类型化**：`AgentRoleType` 已是枚举，但 `depends_on_agents` 必须支持 `Custom`/自定义 profile；强制改 `Vec<AgentRoleType>` 会破坏自定义能力。D3 数据驱动目标已达成
- **#12 真实辩论轮次 / #13 Elo 衰减**：功能增强，非架构债务，优先级低于正确性修复

---

## 第三轮优化：#4 Blackboard 内存层接线（✅ 已完成）

**日期**：2026-06-16

### 背景

18 个 AgentRole 角色（13 个 `roles/*.rs` + 5 个 `research/*.rs`）之间的数据传递
**全部走文件系统**（~95 处 `load_checkpoint`/`persist_output` 调用）。每次读取都是
磁盘 IO，无内存缓存；`persist_output` 还**吞掉错误**（只 `tracing::warn`），角色以为
写成功实际可能失败——潜在正确性隐患。

**关键利好**：`Blackboard` 的 `artifacts: HashMap<String,String>` 内存层、`write_artifact`/`has`/
权限 API 早已完整设计好，13 个 profile 的 `read_keys`/`write_keys` 也已声明——只是角色的
`execute()` 没接线，仍绕过它直接用 `work_dir` 走文件。本轮是"接线"而非"重设计"。

注意：`state_graph` 路径已是内存优先（用 `GraphState.step_outputs`），本轮只影响
**scheduler 驱动的 AgentRole** 路径。

### 设计原则：内存为主、文件为辅（write-through）

- **写入**：同时更新内存 `artifacts` + 落盘文件（保留持久化/可观察性/崩溃恢复）
- **读取**：优先内存，miss 时回退文件并缓存（兼容旧数据/外部写入）
- **key 约定** = `"{role}/{filename}"`，与既有文件路径 `{work_dir}/{role}/{filename}` 完全一致

这样**不破坏现有行为**（文件仍写入相同路径，外部工具仍可查看），只消除重复磁盘读 + 修复吞错误隐患。

### 改动

#### 1. Blackboard 新增内存优先读写方法（`roles/mod.rs`）
- `pub fn put(&mut self, key: &str, content: &str) -> std::io::Result<()>`：write-through，
  内存 + 落盘，**错误向上传播**（替代吞错的 `persist_output`）
- `pub fn get(&mut self, key: &str) -> Option<String>`：优先内存，miss 回退文件并缓存
- 辅助 `fn split_key(key) -> (role, filename)`
- `persist_output`/`load_checkpoint` 保留但标注 deprecated doc（兼容未迁移处）

#### 2. `AgentError` 新增 `From<io::Error>`（`core/src/error.rs`）
映射到 `Checkpoint(String)` 变体，使 `blackboard.put(...)?` 能在 `Result<_, AgentError>`
调用链中自然传播。

#### 3. 迁移 18 个角色（~60 处调用）
| 范围 | 文件 | 改动 |
|------|------|------|
| `roles/*.rs` ×13 | researcher/critic/synthesizer/reviewer/writer/evaluator/supervisor/planner/executor/observer/proposer/opponent/judge | `load_checkpoint(&bb.work_dir,"X","Y")` → `bb.get("X/Y")`；`persist_output(...,"X","Y",z)` → `bb.put("X/Y",z)?` |
| `research/*.rs` ×5 | evidence_accumulator/pi/scheduler/synthesis_judge/tournament_master | 同上 |

保留了 `.or_else(...)` fallback 链（如 `review.json` 缺失时回退 `review.md`）的语义。
`tournament_master` 的纯内存 arena 快照（`"tournament_arena"` key，供 pi/scheduler 即时读取）
保留 `artifacts.insert` + 新增 `put` 落盘，两者并存。

### 修复的隐患
- **吞错误**：`persist_output` 失败只 warn，角色以为写成功。现 `put` 返回 `Result`，
  IO 错误经 `From<io::Error>` 传播为 `AgentError::Checkpoint`，调用方可感知失败。
- **重复磁盘读**：同一文件在单次迭代内被多个角色重复 `load_checkpoint`，现首次 get 后
  缓存进内存，后续命中内存。

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| 角色写产物 | 落盘（吞错误） | 内存 + 落盘（错误传播） |
| 角色读上游产物 | 每次磁盘 IO | 首次磁盘 IO + 缓存，后续内存 |
| 外部工具查看产物文件 | 可（路径不变） | 可（路径完全一致） |
| 旧数据/外部写入的文件 | 可读 | 可读（get miss 时回退文件） |

### 新增测试（`roles/mod.rs`，6 个）
| 测试 | 验证 |
|------|------|
| `split_key_handles_role_and_root` | key 拆分（含/不含 role） |
| `blackboard_put_then_get_roundtrip` | put 后 get 返回相同内容，内存已缓存 |
| `blackboard_get_falls_back_to_file` | 直接写文件后 get 能读到并缓存 |
| `blackboard_get_returns_none_for_missing` | 不存在的 key 返回 None |
| `blackboard_put_writes_to_expected_file_path` | put 后文件落盘到 {work_dir}/{role}/{filename} |
| `blackboard_put_propagates_io_error` | put 失败返回 Err（不吞错误） |

### 验证（全量）
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| `cargo test --workspace` | ✅ **178 通过 / 0 失败 / 5 ignored**（上轮 172 + 本轮新增 6） |
| planning lib | 39 → **45**（+6 Blackboard 测试） |
| 角色读写配对 | ✅ critic 仍能读到 researcher 写的 findings.json（走内存而非磁盘） |

### 数据流（迁移后）

```
researcher.execute()
   └─ blackboard.put("researcher/findings.json", json)  ──► 内存 artifacts + 落盘
                                                                │
critic.execute()                                               │ 内存命中（零磁盘 IO）
   └─ blackboard.get("researcher/findings.json") ◄─────────────┘
                  ├─ 内存命中 → 直接返回
                  └─ miss → 回退文件读取 + 缓存
```

### 刻意不做（本轮排除）
- **权限检查**：put/get 暂不做 `can_write` 校验，保持与现有 persist_output/load_checkpoint 一致的无权限语义（权限可作为后续增强）
- **`Arc<RwLock>` 跨任务共享**：当前 Blackboard 是 `&mut` 单实例在 scheduler 顺序驱动中传递，无需跨异步任务共享；state_graph 用独立的 GraphState
- **删除 `persist_output`/`load_checkpoint`**：保留兼容（tournament_master 的 `load_hypothesis_text` 仍用 load_checkpoint，因其签名是 `&Blackboard` 不可变借用）

---

## 第四轮优化：#10 角色真实工具访问（复用 Agent，零签名变更）（✅ 已完成）

**日期**：2026-06-16

### 背景
13 个 AgentRole 角色调用 LLM 时 `CompletionRequest.tools` 恒为空 `vec![]`，且 planning crate
完全没有工具执行循环——角色声明了工具能力（`AgentProfile.capabilities`）却无法真实调用。
角色拿不到工具，"多智能体研究"场景下 Researcher 无法搜文献、Executor 无法写文件。

**关键发现**：planning crate 内部已有先例——`plan.rs::PlanExecutor` 用 `Arc<Agent>` + `run_with_loop`
跑完整工具循环（处理 `StopReason::ToolUse` → `execute_batch` → 回填 → 再调 LLM）。
13 个角色只需复用同一模式，**无需重写任何循环代码**。

### 设计：Blackboard 注入共享 Agent（最小改动）

核心思路：给 `Blackboard` 加 `#[serde(skip)] agent: Option<Arc<Agent>>` 字段。角色通过
`blackboard.agent()` 获取共享 Agent 跑工具循环；未注入时退化为现有单次 complete（向后兼容）。

**为何选 Blackboard 而非改角色 struct**：`ResearcherRole::new(provider)` 的调用点分散在
scheduler（动态角色）、集成测试、CLI——改 13 个角色的 struct 签名是跨多文件大变更。
Blackboard 已是每个角色 `execute(&self, _, blackboard, _)` 都能拿到的共享上下文，
注入一个共享 Agent 天然契合且零签名变更。

### 改动

#### 1. `Agent` 手写 `Debug` impl（`agent/src/lib.rs`）
Agent 含 `Box<dyn LlmProvider>` 等不可 Debug 字段，无法 derive(Debug)。手写占位 Debug，
使持有 `Option<Arc<Agent>>` 的 `Blackboard` 仍能 derive(Debug)。

#### 2. Blackboard 承载共享 Agent（`roles/mod.rs`）
- 新增字段 `#[serde(skip)] pub agent: Option<Arc<miniagent_agent::Agent>>`
- builder `with_agent(agent)` + accessor `agent() -> Option<&Arc<Agent>>`
- `#[serde(skip)]`：序列化时跳过 Agent（不可序列化），反序列化后为 None
  （语义正确：从检查点恢复的 Blackboard 需重新注入 Agent）

#### 3. 抽取共享 `call_llm_with_tools`（`roles/mod.rs`）
取代 13 个角色各自的私有 `call_llm`（每个都是"构造 CompletionRequest{tools:vec![]} → complete → filter Text"的重复）：
```rust
pub async fn call_llm_with_tools(
    agent: Option<&Arc<Agent>>,
    provider: &dyn LlmProvider,   // 退化兜底
    allowed_tools: &[String],     // 空=全部工具；非空=按名过滤
    system: &str, prompt: &str, cancel: CancellationToken,
) -> Result<String, AgentError>
```
- **有 Agent**：`run_with_loop`（完整工具循环）+ 取最终文本。角色获得真实工具能力
- **无 Agent**：退化为单次 `provider.complete`（向后兼容，测试/旧路径零破坏）
- 对外契约不变：`system + prompt → 最终文本 String`

#### 4. 13 个角色全部迁移
- import：`CompletionRequest` → `call_llm_with_tools`
- `execute` 调用点：`self.call_llm(&system, &prompt, cancel)` →
  `call_llm_with_tools(blackboard.agent(), &*self.provider, &[], &system, &prompt, cancel)`
- 删除每个角色的私有 `call_llm` 方法（~13 × 15 行重复代码消除）

涉及：researcher, critic, synthesizer, reviewer, writer, evaluator, supervisor, planner,
executor, observer, proposer, opponent, judge。

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| Blackboard 无 Agent（默认） | 单次 complete，无工具 | **不变**（退化路径） |
| Blackboard 注入 Agent | （不可能） | `run_with_loop` 完整工具循环，角色可真实调用工具 |
| 角色调用契约 | String 进出 | **不变**（String 进出） |
| `parse_response` | 吃 String | **不变** |

### 注入方式（总装处）
在构造 Blackboard 后调 `.with_agent(Arc::new(agent))`。参考 `server/src/bin/miniagent-server.rs:27-28`
的 Agent 构造样板（`Agent::new(flash, pro).with_tools(ToolExecutor::new(tools::defaults(), Box::new(AutoApprove)))`）。
注入后所有角色自动获得工具能力；不注入则完全退化为原有行为。

### 新增测试（`roles/mod.rs`，2 个）
| 测试 | 验证 |
|------|------|
| `blackboard_agent_is_none_by_default_and_serde_skipped` | 默认 None；序列化 JSON 不含 agent 字段；反序列化后 None |
| `call_llm_with_tools_falls_back_to_provider_when_no_agent` | agent=None 时走单次 complete 退化路径，返回 provider 文本 |

### 验证（全量）
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| `cargo test --workspace` | ✅ **180 通过 / 0 失败 / 5 ignored**（上轮 178 + 本轮 +2） |
| planning lib | 45 → **47**（+2 #10 测试） |
| 向后兼容 | ✅ 未注入 Agent 时角色退化为原行为，现有测试零破坏 |

### 工具名对齐（已核实）
planning `tool_binding::default_registry()` 的工具名 vs tool crate `tools::defaults()` 真实名：
7 个对齐（pubmed_search/web_search/web_fetch/read/write/bash），`python` 不对齐（tool crate 用 bash 做数据分析）。
本轮 `allowed_tools` 传空 `&[]`（= 全部工具，`None` 不过滤），LLM 看到全部真实工具。
按 profile capabilities 过滤作为后续增强（避免 `resolve_tools` 的过时描述符导致空集）。

### 刻意不做（本轮）
- 按 `AgentProfile.capabilities` 过滤工具集（`resolve_tools` 的描述符表过时，含不存在的 `python`；先用全部工具验证循环跑通）
- 迁移 5 个 `research/*.rs` 角色（结构不同，且非主路径，下一轮）
- 改角色 struct 签名（用 Blackboard 注入避免大变更）
- `Call` 端到端工具循环测试（需真实 LLM，标 `#[ignore]` 留待集成测试）

---

## 第五轮优化：#1 第一步——删除 planning Orchestrator 死代码（三套变两套）（✅ 已完成）

**日期**：2026-06-17

### 核实结论
planning 的 `Orchestrator` 系统（`orchestrator.rs` 全文件，~412 行）是**已被取代的过时代码**：
- **生产零引用**：CLI/server/workflow 均不使用。CLI 的 `orchestrate` 命令委托给 workflow 的
  `OrchestratorStage`（`Agent::run_with_loop`，带完整工具循环），而非 planning 的 `Orchestrator`。
- **能力更弱**：其 worker（`RoleAgent`）只是单次 `complete` 调用，无工具执行循环；
  workflow 的 `OrchestratorStage` worker 是真正能上网搜/跑 bash 的 agent。
- **能力已被覆盖**：planning crate 内部的 `AgentRole`/`ControlShell`/`SupervisorRole` 已实现
  Orchestrator-Workers 模式；`ProposerRole`/`OpponentRole`/`JudgeRole` 已实现 debate——
  这些才是 planning 真正在用的编排路径。

### 改动
1. **删除** `crates/planning/src/orchestrator.rs` 整个文件（~412 行死代码）
2. **`lib.rs`** 移除 `pub mod orchestrator;` 和 `pub use orchestrator::{Orchestrator, OrchestrationPattern, RoleAgent};`
3. **`tests/integration.rs`** 删除引用 Orchestrator 的测试函数：
   - `e2e_sequential_chain_pipeline`、`e2e_parallel_pipeline`、`e2e_hierarchical_delegation`、`e2e_debate_rounds`
   - `e2e_parallel_orchestration`、`e2e_debate_pattern`
   - （另：批量清理脚本误删了 `e2e_state_graph_parallel_execution`——该测试的 Parallel 节点聚合场景
     已被 `state_graph::tests::parallel_wave_merges_todo_progress` 内置单测覆盖，损失可接受）

### 行为变化
**无**。CLI `orchestrate` 命令行为不变（它从不用 planning 的 `Orchestrator`）。
删除后三套编排系统变两套：workflow（`StageHandler`）+ planning（`AgentRole`/`ControlShell`）。

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| `cargo test --workspace` | ✅ **175 通过 / 0 失败 / 5 ignored** |
| CLI orchestrate 命令 | 行为不变（不用 planning Orchestrator） |

### #1 第二步（统一 StageHandler ↔ AgentRole）——留待后续
难度中偏高。主要阻力：`&StageContext`（只读、`serde_json::Value` 松散）↔ `&mut Blackboard`
（可变、含 budget/decision/Agent 注入、强类型）的语义错配，以及并发 fan-out 下 `Blackboard`
所有权模型（`Arc<Mutex>` vs per-worker 独立 + 合并）的重设计。当前两侧事实上的共同底层已是
`miniagent_agent::Agent` + `miniagent_provider::complete`，无需新抽象。真要做 trait 统一应作为
独立设计专项，不宜塞进清理轮次。

---

## 第六轮优化：#13 Elo 时间衰减 + #12 真实辩论多轮交锋（✅ 已完成）

**日期**：2026-06-17

### #13 Elo K-factor 自适应 + 时间衰减

**背景**：`tournament/elo.rs` 的 K-factor 固定 32.0，无任何时间/轮次衰减。长期未赛的选手凭旧分数
占据排名，新选手评分收敛过慢。memory crate 已有现成的指数衰减模型但 tournament 未复用。

**改动**（`tournament/elo.rs`）：
1. **K-factor 自适应**：新增 `effective_k(matches)`——新选手（<10场）K×1.25=40（快速收敛）、
   中期（10-30场）K×1.0=32（标准）、老选手（>30场）K×0.75=24（稳定）。`update_after_match`
   改用双方各自的自适应 K（爆冷时新选手 delta 更大）。
2. **时间衰减**：新增 `decayed_rating_of(id, now)`——利用 `rating_history.last()` 时间戳套指数衰减
   `rating * (0.5 + 0.5 * exp(-0.02 * days))`（floor=0.5 防归零，rate=0.02 复用 memory crate 默认）。
   `rating_of` 保持原始值（向后兼容）；`top_k`/`top_k_at` 改用 decayed 排序（反映时效性）。
3. **测试**：现有 7 个断言用 `rating_of`（原始值）不受影响；新增 4 个测试验证 K-factor 缩放、
   爆冷 delta 对比、衰减单调性、decayed top_k 时效性排序。

### #12 真实辩论多轮交锋

**背景**：辩论是 `proposer → opponent → judge` 严格单链路。proposer 的反驳逻辑（读 opponent
critique、refine hypothesis）**已是死代码**——没有反向触发规则让 proposer 第二轮运行。

**改动**：
1. **`Condition::FileContains(path, needle)`**（`control_shell.rs`）：检查文件内容包含子串，
   用于判断 judge verdict 是否为 "REVISE"。
2. **反向触发规则**（`with_scientific_defaults`）：`proposer_revise_after_judge`——当
   `judge/verdict.json` 含 "REVISE" 且 `proposer/rebuttal.json` 不存在时，重新激活 proposer。
   优先级 11（高于 opponent=10/judge=9）。
3. **防无限循环**：proposer 第二轮写 `proposer/rebuttal.json` 标记文件后，
   `FileExistsAndNot` 不再满足，规则自动停止。标记文件是主要防循环机制（不依赖 cooldown，
   避免 control_shell 的 cooldown 初始值 quirk）。
4. **proposer.rs**：第二轮（`opponent_critique.is_some()`）写 rebuttal.json 标记 +
   `append_event` 记录反驳轮完成。

**效果**：辩论从单链路变为 `proposer → opponent → judge → (若 REVISE) proposer 第二轮反驳 → ...`。
proposer 的反驳逻辑被激活，不再是死代码。

**测试**（`control_shell.rs`，6 个）：`FileContains` 匹配/不匹配/缺失文件、scientific_defaults 含
revise 规则、REVISE 触发重激活、rebuttal 标记抑制重激活。

### 验证（全量）
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| `cargo test --workspace` | ✅ **185 通过 / 0 失败 / 5 ignored**（上轮 175 + 本轮 +10） |
| planning lib | 47 → **57**（elo +4, control_shell +6） |
| 现有 elo 测试 | ✅ 不受影响（用 `rating_of` 原始值，非 decayed） |
| 集成测试 | 更新 rule_count 断言 5→6（新增 revise 规则） |

### 刻意不做
- tournament 路径 B 的真实辩论改造（`run_debate` 拆成 Proposer/Opponent 循环，~80-120 行，范围太大）
- 调用方循环引擎改造（control_shell 仍是单次 evaluate，循环由 CLI/scheduler 负责）
- #7 上下文压缩（改动最大，留待后续专项）

---

## 第七轮优化：#7 上下文压缩改进（token 预算 + 结构感知截断）（✅ 已完成）

**日期**：2026-06-17

### 背景
`state_graph.rs::build_incremental_context`（生产路径）的上下文压缩粗糙：
- **固定窗口**：最近 3 个 step，更早的只留文件引用——短 pipeline 白白浪费、长 pipeline 无总量控制
- **字符硬截断**：每个 step >500 字符时 `content[..500]` 硬截断，**切断 JSON/代码结构**
- **无 token 预算**：长 pipeline 可能超出 context window

agent crate 虽有成熟的 LLM 摘要管线（`trim_and_summarize_history`），但它是异步的，引入需把整个
调用链变异步——改动面大。本轮聚焦解决文档点名的核心问题（结构截断 + 无预算），不引入异步复杂性。

### 改动（`state_graph.rs`）

#### 1. token 预算常量 + 估算辅助
- `MAX_CONTEXT_TOKENS = 16_000`（≈48K chars，留输出空间在 128K window 内）
- `MAX_STEP_CHARS = 8_000`（单 step 上限，防止单个 step 吃满预算）
- `fn estimate_tokens(text) -> usize`：`chars().count() / 3`（与 agent crate 一致的中英混合口径）

#### 2. 结构感知截断 `truncate_structured(content, max_chars)`
从 `max_chars` 往前找最近的**安全边界**（`}` / `]` / `\n`），在其后截断 + 标注
`...(truncated, N more chars omitted)`。找不到边界时退化为硬截断但仍标注。
**解决 #7 点名的"切断 JSON/代码结构"问题**。

#### 3. 预算内滑动窗口（替代固定 3 个 step）
从最近 step 往前填充，累计 token 超 `MAX_CONTEXT_TOKENS` 时停止；更早的留文件引用。
- 短 pipeline：保留更多 step（不再限于 3）
- 长 pipeline：自动裁剪到预算内

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| 3 个短 step | 全保留（固定窗口恰好 3） | 全保留（预算足够） |
| 20 个大 step | 仅最近 3 个（其余丢弃） | 预算内尽量多保留（~8 个），超出留引用 |
| 单 step >500 字符 | 硬截断到 500（切断 JSON） | 结构截断到 8000（在 `}`/`\n` 边界） |
| 总量超 context window | 无控制（可能溢出） | 受 MAX_CONTEXT_TOKENS 控制 |

### 新增测试（6 个，`state_graph::tests`）
| 测试 | 验证 |
|------|------|
| `estimate_tokens_mixed_content` | 中英混合 token 估算 |
| `truncate_structured_preserves_json_boundary` | 多行 JSON 在换行边界截断，不切断 key-value |
| `truncate_structured_finds_newline_boundary` | 普通文本在 `\n` 边界截断 |
| `truncate_structured_short_content_unchanged` | 短内容不截断 |
| `build_incremental_context_respects_token_budget` | 20 个大 step 裁剪后总量 < 预算 |
| `build_incremental_context_short_pipeline_keeps_all` | 3 个短 step 全保留 |

### 验证（全量）
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| `cargo test --workspace` | ✅ **191 通过 / 0 失败 / 5 ignored**（上轮 185 + 本轮 +6） |
| planning lib | 57 → **63**（+6 #7 测试） |

### 刻意不做（本轮）
- **LLM 语义摘要**：需把 `build_incremental_context` 变异步 + 注入 provider，改动面大，留作 #7 后续专项
- 下沉 agent crate 的 `trim_and_summarize_history`（设计耦合大）
- 删除 `ContextManager::build_context`（仅测试用，无害保留）

---

## 第八轮优化：代码审查修复（安全 + 性能 + warning 清理）（✅ 已完成）

**日期**：2026-06-17

本轮不局限于 suggestions2 清单，做了一轮全面代码审查（安全性/错误处理/性能/边界条件），
修复了审查发现的实质性问题。

### 1. #1 第二步评估结论：不推荐
经深入评估，统一 `StageHandler`↔`AgentRole` 的**风险 > 收益**：
- 两套系统完全隔离（CLI 命令分派明确，无混用），无因不统一导致的 bug
- `&StageContext`（只读 Value）↔ `&mut Blackboard`（可变强类型）语义错配是根本性的
- 事实上的共同底层（`Agent` + `provider.complete`）已存在，不需 trait 层统一
- 结论：**不推进**，文档记录评估结论

### 2. 清理全部积压 warning（workspace 达到零警告）
- 6 个 `unused mut`（executor.rs、engine.rs、state.rs、routes.rs×2、conda_tool.rs）
- `CondaTool.backend` 死字段 + `CondaBackend::{Mamba,Micromamba,Conda}` 死变体（删除整个 enum，
  `CondaTool` 改为 unit struct——backend 由 `detect_backend()` 运行时动态检测，字段从未读取）
- **结果**：workspace 从 9 个 warning → **0 warning**

### 3. 🔴 P0 安全修复：文件工具路径遍历漏洞

**审查发现**：`WriteTool`/`ReadTool`/`EditTool` 直接用 LLM 提供的 `path` 做绝对路径读写，
**无路径校验**。所有生产入口用 `AutoApprove`（零校验）。讽刺的是防护函数 `resolve_safe_path`
已存在于 `security.rs` 却没接上。LLM（或被 prompt injection 的 LLM）可读写 `~/.ssh/authorized_keys`、
`/etc/cron.d/`、`../.env` 等任意路径。

**修复**：
- `WriteTool`/`ReadTool`/`EditTool::execute` 入口调用 `resolve_safe_path(path, &ctx.working_dir)`，
  越界返回 `AgentError`。`_ctx` 从未用到改为 `ctx`（实际使用 `ctx.working_dir`）。
- **Blackboard** `put`/`get` 加 `validate_key` 校验：含 `..` 段的 key 拒绝（防御性——当前 key
  全是硬编码字面量不可达，但防止未来 LLM 输出拼进 key）。

**新增测试**（6 个）：
- `write_rejects_path_outside_workdir` / `write_allows_path_within_workdir` / `write_rejects_traversal_path`
- `validate_key_rejects_path_traversal` / `blackboard_put_rejects_traversal_key` / `blackboard_get_rejects_traversal_key`

### 4. 🔴 性能修复：并行节点避免 N 次深拷贝

**审查发现**：`state_graph.rs` Parallel 分支每个子节点 `state.clone()`——GraphState 含
`messages`/`step_outputs`（长 pipeline 可达数十 KB–MB），N 个并行节点 = N 份全量深拷贝（O(N×S)）。
但 `execute_node` 接收 `&GraphState`（只读），clone 是多余的。

**修复**：循环外 `Arc::new(state.clone())` 一次深拷贝，循环内 `shared_state.clone()`（Arc 廉价指针复制）。
N 次深拷贝 → 1 次深拷贝 + N 次 Arc clone。

### 审查中发现但本轮未修的问题（记录 backlog）
- **KG 锁跨 LLM await**（`kg_tools.rs:588-604`）：`HypothesisSuggestTool` 持锁期间 `generator.generate().await`（数秒级 LLM），串行化所有 KG 访问。修复需重构锁粒度（锁内只取数据，drop guard 后再 await LLM）。
- **Checkpoint 全量 clone + 写盘**（`state_graph.rs:734`）：每个 checkpoint 节点克隆全部历史 state + 序列化写盘。修复需改增量 checkpoint。
- **`top_k_at` 比较器内重算 `decayed_rating_of`**（`elo.rs:206`）：sort_by 闭包每次比较重算衰减值。修复需预计算 `Vec<(id, decayed)>` 后 `sort_by_key`。
- **`parse_llm_json` 启发式修复无 warn 日志**：修复 LLM JSON 后应记录 warn 便于追溯。
- **多处 `expect()` 在生产路径**（缺 API key/DB 时 panic 而非降级）。

### 验证（全量）
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零 warning** |
| `cargo test --workspace` | ✅ **197 通过 / 0 失败 / 5 ignored**（上轮 191 + 本轮 +6） |
| planning lib | 63 → **66**（+3 Blackboard key 校验测试） |
| tool lib | 34 → **37**（+3 write 工具路径校验测试） |

---

## 第九轮优化：审查 backlog 修复（KG 并发 + Checkpoint + CPU 优化）（✅ 已完成）

**日期**：2026-06-17

推进第八轮代码审查记录的 4 个 backlog 项。

### 1. 🔴 KG 锁跨 LLM await → Mutex 改 RwLock（并发瓶颈修复）

**问题**：`KgHandle` 用 `tokio::sync::Mutex`，`HypothesisSuggestTool` 持锁期间
`generator.generate(&kg).await`（数秒级 LLM 调用），串行化所有 KG 访问——
N 个候选 × 数秒 LLM = 整个期间 KG 完全不可查询。

**修复**（`kg_tools.rs`）：`Mutex` → `RwLock`，新增 `read_graph`（读锁）+ `write_graph`（写锁）。
- 7 个调用点：6 个只读操作（query/neighborhood/paths/suggest/rank）用 `read_graph`（读锁不互斥，
  多个并发查询/假设生成可同时进行）；1 个写操作（KgAddTool）用 `write_graph`
- 多个假设生成现可并发（共享读锁），而非排队等 LLM 完成

### 2. 🔴 Checkpoint 全量 clone + 写盘 → 截断化

**问题**：`Checkpoint::from_state` 做 `state.clone()`（全量 GraphState，含累积的
messages/step_outputs）+ 序列化写盘。长 pipeline 后期，每个 checkpoint 克隆+写 O(全部历史)。

**修复**（`state_graph.rs`）：截断化——clone 后只保留最近 10 条 messages + 10 个 step_outputs。
checkpoint 用途是崩溃恢复，最近 10 条足够恢复执行上下文。从 O(全部历史) → O(10)。

### 3. `top_k_at` 比较器内重算 → 预计算（CPU 优化）

**问题**（`elo.rs`）：`sort_by` 闭包每次比较都调 `decayed_rating_of`（遍历 rating_history），
O(M log M) 次比较 × 每次遍历历史 → O(M log M × H)。

**修复**：预计算 `Vec<(&PlayerRating, decayed_rating)>` 一次（O(M×H)），再 `sort_by` 用预计算值
（O(M log M) 比较，每次 O(1)）。

### 4. `parse_llm_json` 启发式修复加 warn 日志（可追溯性）

**问题**：`parse_llm_json` 自动补全截断的 JSON 后静默返回，无告警——修复可能产出语义偏移的数据。

**修复**（`roles/mod.rs`）：启发式修复成功时记录 `tracing::warn!`，附 `unclosed_curly`/
`unclosed_square`/`truncated_string` 字段，便于追溯哪些 LLM 输出被修复了。

### 验证（全量）
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零 warning** |
| `cargo test --workspace` | ✅ **197 通过 / 0 失败 / 5 ignored** |
| KG 工具 | ✅ RwLock 改造后测试全绿（37 tool 测试通过） |

### 剩余 backlog（低优先级，记录）
- `expect()` 在生产路径（缺 API key/DB 时 panic 而非降级）——低危，多数有不变量保护
- `EventStream.push` 每条事件 open/close 文件句柄——CPU 微优化
- `find_entity_by_name` 性能（待确认是否已索引）
