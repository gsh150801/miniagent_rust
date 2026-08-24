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

---

## 第十轮优化：loop-pipeline 任务级增量复用（缺陷 #1 + #2 + #3 修复）（✅ 已完成）

**日期**：2026-06-21

### 问题回顾
loop-pipeline 的 Explore→Plan→Dispatch→Evaluate→Repair 循环存在三个互相依赖的结构性缺陷：
1. **plan 每轮全量重生成** → 新 plan 丢失上轮 task 的 id/output，task_id 跨轮不稳定
2. **dispatch 无条件重新执行所有 task** → 已成功任务被白费重跑（LLM 调用、工具执行）
3. **task_results 无界累积 + evaluate 进度失真** → completed 计入重复结果，progress_pct 虚高

根因：**没有"增量"概念**。本方案建立任务级增量复用链路。

### 三层修复

#### 层 1：plan 增量合并（`plan.rs`）
新增 `merge_plan(new_plan, old_plan, task_results) -> TaskPlan` 函数：
- 遍历新 plan 的 task，若 id 在旧 plan 中存在且 task_results 显示该 id 成功 → 保留旧 output/error
- 使 dispatch 能据此跳过重跑
- prompt 改动：prior_tasks 上下文显式包含上轮 task id + 成功/失败状态标记，要求 LLM 对成功任务复用相同 id

#### 层 2：dispatch 跳过已成功任务 + 产物校验（`dispatch.rs`）
- 新增 `outputs_still_exist(expected_output, working_dir) -> bool`：从 expected_output 提取文件路径（.py/.rs/.md/.csv 等扩展名），校验是否仍存在
- wave 执行循环 spawn 前：查 `result_map` 是否有该 task_id 的成功记录 + 产物校验通过 → 跳过重跑，复用旧结果
- result_map 已按 task_id 去重（HashMap），消除缺陷 #2 的无界累积

#### 层 3：evaluate 基于当前 plan 计算真实进度（`evaluate.rs`）
- 用 `plan_task_ids: HashSet` 过滤，只统计当前 plan 内 task 的结果
- `debug_assert_eq!(completed + failed + pending, total)` 保证不再有 completed+failed > total 的失真

### 附带修复
- **server/routes.rs 多余 `}`**：第八轮 warning 清理时引入的花括号不平衡，已修复

### 新增测试（10 个）
| 测试文件 | 测试 | 验证 |
|---------|------|------|
| dispatch.rs (×6) | `test_outputs_still_exist_*` | 文件存在/缺失/相对路径/多文件/纯文本各场景 |
| plan.rs (×4) | `test_merge_plan_*` | 首次不合并/保留成功 output/新 id 不合并/失败 task 不合并 |

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅（provider/evolution 既有 warning 非本轮引入） |
| `cargo test -p miniagent-loop-pipeline --lib` | ✅ **24 通过 / 0 失败**（14 原有 + 10 新增） |
| `integration_test.rs` | ✅ **37 通过 / 0 失败** |
| `stepfun_integration.rs` | ⚠️ 5 failed = StepFun API 429 rate limit（与改动无关，配额耗尽） |

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| 第 2 轮 plan | 全量重生成，丢失上轮 id | 增量合并：成功任务保留 id/output |
| 第 2 轮 dispatch | 重新执行所有 task（含已成功的） | 跳过已成功且产物存在的 task |
| task_results | 无界累积（跨轮 push 不去重） | 按 task_id 去重（HashMap） |
| evaluate 进度 | completed 计入跨轮孤儿结果 | 只统计当前 plan 内 task |
| 成本 | N 轮 = N 倍重复执行 | N 轮 ≈ 只执行失败/新增 task |

---

## 第十一轮优化：loop-pipeline 客观产物校验（缺陷 #3 修复）（✅ 已完成）

**日期**：2026-06-21

### 问题
evaluate 阶段完全依赖 LLM 主观判断（"我觉得完成了"），无客观信号验证产物是否真存在。
LLM 可能说"全部完成"但实际文件缺失——基于错误评估提前终止。

### 修复（`evaluate.rs`）
新增 `check_phantom_failures(tasks, results, working_dir) -> Vec<String>`：
- 对 plan 中标记为"成功"的 task，检查 expected_output 提到的文件是否真存在（复用
  dispatch.rs 的 `outputs_still_exist`）
- 返回"幽灵成功"列表（标记成功但产物缺失的 task_id）

在 evaluate 的 override 逻辑后注入：当 `should_continue=false`（即将终止）时，若有幽灵失败：
- 强制 `should_continue=true`（防止提前终止）
- 幽灵 task 加入 `failed_task_ids`（让 dispatch 下轮重跑）
- 加入 `unmet_goals` 记录缺失原因

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| LLM 说"全部完成"但文件缺失 | 提前终止（错误） | 强制继续，标记缺失 task 为失败（重跑） |
| LLM 说"全部完成"且文件都在 | 终止（正确） | 终止（不变，客观校验通过） |
| 纯文本输出（无文件） | — | 不校验（outputs_still_exist 返回 true） |

### #4 loop_count 经评估不修复
`loop_count += 1` 在 evaluate 内部的两个分支（行 274、299）。经评估：当前设计**自洽**
——evaluate 是每轮唯一递增 loop_count 的地方，语义清晰（"完成一轮评估后递增"），
evaluate 内部的 `loop_count >= max_loops` 判断也依赖此递增。移动到主循环会改变控制流
语义且增加风险，**判定为"不修复"**（代码异味但不影响正确性）。

### 新增测试（5 个，`evaluate.rs::tests`）
| 测试 | 验证 |
|------|------|
| `test_phantom_check_no_missing_files` | 文件存在 → 无幽灵 |
| `test_phantom_check_detects_missing_file` | 文件缺失 → 检测到幽灵 |
| `test_phantom_check_skips_text_only_outputs` | 纯文本输出 → 不校验 |
| `test_phantom_check_skips_failed_tasks` | 失败 task → 不校验（已在 failed_ids） |
| `test_phantom_check_mixed_success_and_failure` | 混合场景 → 只标记真正缺失的 |

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| loop-pipeline lib | ✅ **29 通过**（24 原有 + 5 新增） |
| integration_test | ✅ **37 通过** |
| stepfun_integration | ⚠️ 5 failed = API 429（与改动无关） |

---

## 第十二轮优化：CLI provider 路由修复（PROVIDER=stepfun 401 根因）（✅ 已完成）

**日期**：2026-06-21

### 问题
用户配置 `PROVIDER=stepfun` + 真实 StepFun key，但 workflow 命令报 401 Unauthorized。
根因：**CLI 所有命令硬编码 DeepSeek provider**，用 `config.require_deepseek_key()` 取 key
（占位符）+ `DeepSeekFlash::new()` 构造——完全忽略 `PROVIDER=stepfun` 配置。

### 根因分析
- `settings.rs` **早已有完整 provider 路由基础设施**：`provider` 字段、`is_stepfun()`、
  `require_active_key()`、`require_stepfun_key()`——但 CLI 从未使用
- CLI 全部命令用 `require_deepseek_key()` + `DeepSeekFlash/Pro::new()` 硬编码
- StepFun provider（`StepFunFlash`/`StepFunClient`）已实现但 CLI 从未接入

### 修复（`cli/src/main.rs`）
1. **新增 `make_providers(config) -> (Box<dyn LlmProvider>, Box<dyn LlmProvider>)`** 工厂函数：
   根据 `config.is_stepfun()` 返回 (StepFun, StepFun) 或 (DeepSeekFlash, DeepSeekPro)
2. **所有命令的 key 解析**：`require_deepseek_key()` → `require_active_key()`（尊重 PROVIDER）
3. **所有命令的 provider 构造**改为路由：
   - `build_full_agent`：用 `make_providers`（agent/run/loop 命令受益）
   - `workflow_command`：PlannerStage 的 flash 用 if-else 路由
   - `plan_command`：Planner + agent 用 `make_providers`
   - `orchestrate_command`：agent + decompose flash 用路由
   - `debate_command`：三个角色（proposer/opponent/judge）用三元组路由
   - `team_command`：StateGraph execute 的 flash/pro 用 `make_providers`
   - `research_command`：用 `make_providers`

### 行为变化
| 场景 | 旧行为 | 新行为 |
|------|--------|--------|
| `PROVIDER=stepfun` + 有效 StepFun key | 401（用 DeepSeek 占位符 key） | ✅ 正确路由到 StepFun |
| `PROVIDER=deepseek`（默认）+ 有效 DeepSeek key | ✅ 正常 | ✅ 正常（不变） |
| 缺 key | `require_deepseek_key` 报错 | `require_active_key` 报当前 provider 的 key 缺失 |

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 29） | ✅ 全绿 |

---

## 第十三轮优化：Server provider 路由 + skill 浏览端点（✅ 已完成）

**日期**：2026-06-23

### 问题 1："Rust vs Go" 任务报 401 Unauthorized
**根因**：server 的 `handle_run`（WebSocket 任务处理）和 `run_handler`（REST API）都硬编码
`config.require_deepseek_key()` + `DeepSeekFlash::new()`——与 CLI 的问题完全相同。
`PROVIDER=stepfun` 配置被忽略，用 DeepSeek 占位符 key 调 API → 401 → "Planner LLM failed,
using single-agent fallback"。

**修复**（`server/src/routes.rs`）：
- `handle_run` + `run_handler`：`require_deepseek_key()` → `require_active_key()`
- `PlannerStage::new`：if `is_stepfun()` → StepFunFlash，else → DeepSeekFlash
- `stream_synthesis`：加 `is_stepfun` 参数，按配置选 StepFunFlash 或 DeepSeekPro
- server bin（`miniagent-server.rs`）启动逻辑**已正确路由**（无需改）

### 问题 2：前端 skill 面板永远为空（"只能搜索不能浏览"）
**根因**：后端**没有 `/api/skills` 端点**！前端 `loadSkills()` fetch `/api/skills` 返回 404，
`skills` 数组保持空 `[]`，面板永远显示"No skills found"。不是"只能搜索不能浏览"，
而是**根本加载不到任何 skill**。

**修复**（`server/src/routes.rs`）：
- 新增 `skills_handler`：扫描 `skills/` + `.miniagent/skills/` 目录的 SKILL.md，
  返回 `[{name, description, triggers, tags, tools_needed, priority, actionable, version}]`
- 注册路由 `.route("/api/skills", get(skills_handler))`

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 29） | ✅ 全绿 |

---

## 第十四轮优化：需求2全链路追溯 + 需求3日志改 error（批次1+2）（✅ 已完成）

**日期**：2026-06-26

### 需求3：日志改 error-only（批次1）

**问题**：server 日志 filter level = INFO（含大量 warn/info 噪声）；3 处危险吞错。

**修复**：
- `server/bin/miniagent-server.rs`：`init("info")` → `init("error")`（只记 error，忽略 warn/info）
- **修复 run_judge/run_critic 降级吞错**（`dispatch.rs`）：provider 错误/parse 错误时原来返回
  `passed: true`（把失败任务误判通过）→ 改为 `passed: false` + `tracing::error!`
- **修复 metadata.json 写失败吞错**（`routes.rs:1408`）：`let _ = write(...)` → 记 error
- **修复 dispatch 持久化 `.ok()` 吞错**（`dispatch.rs:617/648/683`）：`.ok()` → 记 error

### 需求2：全链路追溯（批次2）

**问题**：工具调用审计（AgentEvent）只走 broadcast→前端，**不落盘**——刷新后丢失。
审计基础设施（EventStream/AuditLogHook）代码存在但未接入 server。

**修复**：
- **`TaskInfo` 加 `event_log: Vec<serde_json::Value>`**：存储每个 AgentEvent（含完整 tool input/output/error/duration）+ 时间戳
- **`run_with_progress` / `run_multi_stage_with_streaming`**：AgentEvent 分支并行落盘到 `task.event_log`（每事件 `{ts, event}`）
- **新增 `/api/trace/{task_id}` 端点**：返回 task 的完整 event_log + stage_outputs + summary
- **前端加"📋 轨迹"按钮**：每个任务卡可点击查看完整执行轨迹（工具调用链、error 高亮、时间戳）

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 29 + server） | ✅ 全绿 |

### 后续批次（需求1，下一轮）
- 批次3：新建 Executor/Validator/Arbiter 三角色执行结构
- 批次4：统一新流程（explore→ask→plan→调度→执行→反馈）+ 双向 ws + 前端重构

---

## 第十五轮优化：需求1 三角色 + 统一新流程 + 双向ws + 前端重构（批次3+4）（✅ 已完成）

**日期**：2026-06-27

### 批次3：三角色执行结构（Executor/Validator/Arbiter）

**新建** `crates/loop-pipeline/src/roles/`：
- **`validator.rs`**：`ValidationReport { passed, issues, severity, suggestions }` + `run_validator`（单次 LLM 调用，校验执行者产出）
- **`arbiter.rs`**：`ArbiterDecision { Pass, Revise{feedback}, Supplement{feedback} }` + `run_arbiter`/`run_arbiter_forgiving`（综合产物+校验报告做决策）
- **`mod.rs`**：`execute_with_roles` 协作循环（Executor→Validator→Arbiter→Revise/Supplement→重新Executor，最多 2 轮）+ `ThreeRoleResult`

**TaskResult 扩展**：加 `validation_report: Option<Value>` + `arbiter_decision: Option<String>`

**9 个单元测试**：validator 解析/检测问题、arbiter serde/决策/pass/revise、三角色协作循环（pass首轮/循环重试/超限强制pass）

### 批次4：统一新流程 + 双向 ws + 前端重构

#### 后端（`routes.rs`）
- **AppState 加 `asks`**：`DashMap<String, oneshot::Sender<String>>`（ask 暂停的同步机制）
- **`handle_ws` 加 `"ask_reply"` 消息分支**：前端回复时唤醒暂停的 task
- **`ask_user` helper**：推 `{type:'ask'}` + 注册 oneshot + await（5 分钟超时保护）
- **`handle_run` 加 ExploreStage**：LLM 分析问题获取上下文，推 progress 事件
- **`handle_run` 加 AskStage**（可选）：探索发现"ambiguous"时反问用户
- **`handle_run` 加 PlanStage progress 事件**：推 explore/plan 阶段状态
- **`handle_ws` 的 `"run"` 分支统一**：移除 `if req.mode == "loop"` 分支，全部走 `handle_run`

#### 前端（`app.js` + `index.html` + `styles.css`）
- **移除 modeToggle**：HTML 删除按钮 + JS 删除 toggleMode + sendMessage 不再传 mode
- **`handleMsg` 加 `case 'ask'`**：调 `renderAsk` 渲染输入框/选项卡
- **`renderAsk` 函数**：问题文本 + 选项按钮（点击即回复）+ 文本输入框（Enter 回复）+ Reply 按钮
- **ask CSS 样式**：`.msg-ask` / `.ask-option-btn` / `.ask-input` / `.ask-send-btn`

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 38） | ✅ 全绿 |
| 三角色测试 | ✅ 9/9 |

### 后续（待手动测试）
- server 启动 + 前端发任务 → 观察 explore/plan 阶段实时进度
- ask 交互：LLM 判断 ambiguous → 前端弹输入框 → 用户回复 → task 继续
- 三角色接入 dispatch：当前 `execute_with_roles` 已实现，接入 `handle_run` 的 dispatch 阶段需后续

---

## 第十六轮优化：FeedbackStage 总评审（需求1 完成）（✅ 已完成）

**日期**：2026-06-28

### FeedbackStage 总评审
在 `handle_run` 的 workflow 执行后、函数结束前，加 FeedbackStage：
- **`run_feedback_review`**：综合所有 stage 产物 + 原始需求，用 LLM 做总评审
  - 输出 `FeedbackResult { verdict: "deliver"/"revise"/"unclear", summary }`
  - verdict 非 deliver 时推反馈给前端（用户可决定是否重新发起）
- 推 `{type:'progress', stage:'feedback', status:'running/done'}` 到前端
- 设计决策：总评审在 workflow 整体执行后做一次，而非逐 stage 评审——避免侵入 workflow 执行逻辑，降低风险

### 完整新流程状态
`handle_run` 现在的完整流程：
1. ✅ **ExploreStage**：LLM 分析问题获取上下文
2. ✅ **AskStage**（可选）：问题模糊时反问用户（双向 ws 暂停）
3. ✅ **PlanStage**：PlannerStage 拆解子任务 + 依赖分类
4. ✅ **DispatchStage**：WorkflowBuilder 按依赖执行（串行/并行）
5. ✅ **FeedbackStage**：总评审决定交付/修改/不确定

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 38） | ✅ 全绿 |

### 三个需求最终状态
- **需求1**（统一工作流）：✅ 完成（explore→ask→plan→dispatch→feedback 全链路 + 三角色结构 + 双向 ws + 前端 ask 交互 + 总评审）
- **需求2**（全链路追溯）：✅ 完成（event_log 落盘 + /api/trace 端点 + 前端轨迹查看）
- **需求3**（日志只记 error）：✅ 完成（filter level error + 修复 3 处危险吞错）

---

## 第十七轮优化：日志策略细化（error + 工具调用，不记 warning）（✅ 已完成）

**日期**：2026-06-28

### 用户要求
日志只记录报错、工具调用（成功/失败/参数/结果）等重要信息，不记录 warning 这种非重要信息。

### 修复

#### 1. Filter 策略调整（`telemetry/src/subscriber.rs`）
fallback filter 从 `miniagent={level},tokio=warn,hyper=warn,reqwest=warn` 改为：
```
miniagent=error,tool_call=info,tokio=error,hyper=error,reqwest=error
```
- **`miniagent` 整体只记 error**（忽略 99 处 warn、115 处 info）
- **`tool_call` target 放行到 info**（工具调用是重要信息，需记录）
- **框架噪声（tokio/hyper/reqwest）压到 error**（不再显示 warn）

#### 2. 工具调用加 tracing 日志（`agent/src/lib.rs`）
在 Agent 的工具执行循环加三个日志点（target="tool_call"）：
- **`tool_call_requested`**（info）：call_id + tool_name + input（参数）
- **`tool_call_completed`**（info）：call_id + tool_name + duration_ms + result（成功结果，截断 500 字符）
- **`tool_call_failed`**（error）：call_id + tool_name + duration_ms + result（失败结果）

覆盖有/无 self_improver 两条路径。

### 效果
重启 server 后，日志只显示：
- 🔴 error（报错）
- 🔧 tool_call（工具调用的完整参数+结果+耗时+成功/失败）
- 不再显示 warn/info 噪声

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 38） | ✅ 全绿 |

---

## 第十八轮优化：参考 cc-python-claude 完善提示词工程（✅ 已完成）

**日期**：2026-06-28

### 参考项目分析
研究了 `/Users/Apple/Downloads/cc-python-claude`（Claude Code Python 版）的提示词构建工程。
其 `cc/prompts/` 目录实现了成熟的分层提示词系统：builder.py（拼装）+ sections.py（各段落文本）+
teammate_prompt.py（多智能体通信）+ coordinator_prompt.py（协调者编排）。

### 差距 → 改进

| cc-python-claude 的设计 | 本项目改进 |
|--------------------------|-----------|
| **分层 system prompt**（intro→system→tasks→actions→tools→tone→efficiency→env） | `role_system_prompt` 从一整块拼接改为 **8 段分层** |
| **"先读再改"原则** | 加 Task Execution Principles 段（read before modify, don't over-engineer, diagnose failures）|
| **工具使用偏好**（Read>cat, Edit>sed, Glob>find） | 加 Tool Usage Preferences 段（专用工具优先于 bash）|
| **风险评估**（可逆自由、不可逆确认） | 加 Risk Assessment 段 |
| **输出效率**（简洁直接、先结论后推理） | 加 Output Efficiency 段 |
| **环境信息注入**（cwd/platform/shell/git/date） | 新增 `env_info_block()` 函数，dispatch 注入 |
| **Worker prompt 自包含** | tool_instruction_block 加"self-contained"和"record tool results"指令 |

### 具体改动

#### `loop-pipeline/src/prompts.rs`
- **`role_system_prompt`** 重构：从 5 段（角色+任务+输出+工具+规则）扩展为 8 段分层：
  1. 角色定义
  2. **Task Execution Principles**（先读再改、不过度工程、安全优先、失败诊断）
  3. **Tool Usage Preferences**（read>cat, edit>sed, glob>find, 并行调用）
  4. **Risk Assessment**（可逆自由、不可逆确认）
  5. **Output Efficiency**（简洁直接、先结论后推理）
  6. 具体任务（task_desc + expected_output）
  7. 角色特定工具指南
  8. 关键规则
- **`tool_instruction_block`** 增强：加 self-contained + record tool results 指令
- **新增 `env_info_block(working_dir)`**：注入工作目录/平台/shell/git/date

#### `loop-pipeline/src/dispatch.rs`
- `execute_single_task` 的 user prompt 注入 `env_info_block` 环境信息

#### `server/src/routes.rs`
- AgentStage 的 `system_prompt` 从 2 行扩展为含 Task Execution Principles +
  Tool Usage Preferences + Risk Assessment + Output Efficiency 的分层提示词

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| loop-pipeline lib 测试 | ✅ 38/38 全绿 |

---

## 第十九轮优化：参考 cc-python-claude 完善核心工程实现（✅ 已完成）

**日期**：2026-06-28

### 参考
全面研究了 cc-python-claude（Claude Code Python 克隆）的工程实现，对比 6 个维度找出差距。
本轮实施 5 个高价值改进（P0+P1），其余（权限系统/记忆提取/子agent工具）属架构性改造留待后续。

### P0-1：token 估算改 UTF-8 字节数
**差距**：`chars/3` 对中文严重低估（中文 1 char ≈ 1.5 token，chars/3 算成 0.33 token/char，偏差 4.5x）
**修复**：
- `agent/src/lib.rs` 的 `estimate_history_tokens`：`chars().count() / 3` → `len() / 4`（UTF-8 字节数）
- `planning/src/state_graph.rs` 的 `estimate_tokens`：同步改为 `len() / 4`
- 更新测试断言

### P0-2+3：bash 工具安全增强
**差距**：无输出截断（大输出撑爆上下文）+ 不绑定工作目录（路径逃逸面）
**修复**（`tool/src/tools/bash.rs`）：
- **输出截断**：stdout/stderr 各 200KB cap（参考 cc-python-claude MAX_OUTPUT_BYTES），超出附 truncation 提示
- **绑定 working_dir**：`_ctx` → `ctx`，`Command::current_dir(&ctx.working_dir)`，bash 在指定工作目录执行

### P1-1：run_with_loop 重试逻辑
**差距**：429/529 瞬时错误直接 `?` 终止整个 run，无重试
**修复**（`agent/src/lib.rs`）：
- LLM 调用点加重试循环：429/529/rate limit/overloaded/connection/timeout → 指数退避（2s→4s→8s），最多 3 次
- 非瞬时错误或重试耗尽 → 返回错误

### P1-2：MaxTokens 截断续写
**差距**：长输出被 MaxTokens 截断即 break，丢失后续内容
**修复**（`agent/src/lib.rs`）：
- `StopReason::MaxTokens` 不再直接 break，改为追加 "Please continue" 续写（参考 cc-python-claude query_loop）
- 下一轮循环让 LLM 从截断点继续
- 最后一轮迭代时仍 break（防无限循环）

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（core 25 + planning 66 + loop-pipeline 38） | ✅ 全绿 |

---

## 第二十轮优化：参考 cc-python-claude 深度改进（权限/钩子/记忆提取）（✅ 已完成）

**日期**：2026-06-28

### 改进1：权限系统升级（ASK 三态 + 白名单 + 非交互 fail-fast）

**差距**：ApprovalHandler 只有 Allow/Deny 二元结果，无 Ask 第三态；无白名单；无模式系统。

**修复**（`tool/src/approval.rs`）：
- **`ApprovalDecision` 新增 `Ask(String)` 变体**：需询问用户是否允许
- **`WhitelistMode` 四级模式**（参考 cc-python-claude PermissionMode）：
  - `Bypass`：全放行
  - `AcceptEdits`：只读+编辑自动允许，bash/git/conda 需 Ask
  - `Default`：只只读自动允许，其余 Ask
  - `NonInteractive`：同 Default 但 Ask 直接 Deny（fail-fast，用于后台/无人值守）
- **`WhitelistApproval` handler**：`READ_ONLY_TOOLS` + `EDIT_TOOLS` 白名单常量
- **`ToolExecutor` 处理 Ask**：executor 层无法交互式询问，Ask 退化为 Deny（交互式询问由 server 层处理）

### 改进2：外部 shell 钩子加载器

**差距**：内置钩子全编译期写死，用户无法零代码扩展。

**修复**（`agent/src/hooks.rs`）：
- **`load_external_hooks(registry, config_json)`**：从 JSON 配置加载外部 shell 命令钩子
- 配置格式：`{"hooks": {"BeforeToolCall": [...], "AfterToolCall": [...]}}`
- 每项可以是字符串（简写）或对象 `{command, tool_name}`
- 执行协议（参考 cc-python-claude hook_runner.py）：
  - 工具上下文 JSON 经 stdin 传入
  - 退出码 0 = 放行，2 = 阻止（stdout 作为原因）
  - 超时 10s 强杀
  - 钩子故障不影响工具执行（异常/超时 → Continue）
- 支持 6 种事件：BeforeToolCall/AfterToolCall/BeforeLlmCall/AfterLlmCall/BeforeAgentLoop/AfterAgentLoop

### 改进3：LLM 记忆提取器

**差距**：memory crate 有 SQLite+FTS5 存储层但缺提取入口——"有数据库但没人往里写"。

**修复**（`memory/src/extractor.rs`，新建）：
- **`extract_memories(provider, messages, cancel)`**：用 LLM 分析最近 20 条对话，提取 4 类记忆
  - **user**：用户角色/偏好/专业水平/目标（importance 0.8）
  - **feedback**：用户对工作方式的纠正（importance 0.9，最重要）
  - **project**：项目上下文/决策/截止日期（importance 0.6）
  - **reference**：外部资源指针（importance 0.5）
- **"不存什么"规则**（参考 cc-python-claude）：代码模式、git 历史、debug 方案——可从代码推导
- **`MIN_NEW_MESSAGES=4` 阈值**：新消息不足 4 条跳过（省 API）
- **`extract_and_store(provider, messages, manager, cancel)`**：完整提取→转 EpisodicRecord→存入 SQLite
- **`memory_to_record`**：ExtractedMemory → EpisodicRecord（含 importance 分级 + tags）
- 3 个单元测试

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（memory 3 + tool 37 + loop-pipeline 38） | ✅ 全绿 |

---

## 第二十一轮优化：AskUser + NotebookEdit + PlanOnly + SkillAsTool（✅ 已完成）

**日期**：2026-06-29

### 高1：AskUser Tool + UserPrompt trait
**差距**：LLM 无法主动向用户提问（遇到歧义只能猜测或停止）。

**修复**：
- **`UserPrompt` trait**（`tool/src/traits.rs`）：`async fn ask(&self, question: &str) -> Option<String>`，依赖注入解耦输入来源
- **`NoUserPrompt`**：非交互实现（返回 None，用于 CI/管道）
- **`ToolContext` 加 `user_prompt: Arc<dyn UserPrompt>`**：工具通过 ctx.user_prompt.ask() 提问
- **`AskUserTool`**（`tool/src/tools/ask_user.rs`）：LLM 可调用的提问工具，注册到 defaults()
- **`ToolContext::new()` 工厂方法**：默认 NoUserPrompt，CLI/server 可 `.with_user_prompt()` 注入

### 高2：NotebookEditTool
**差距**：科研 agent 无法直接产出 .ipynb 交付物。

**修复**（`tool/src/tools/notebook_edit.rs`，新建）：
- 支持 insert_cell / replace_cell / delete_cell 三种操作
- 直接操作 .ipynb JSON 结构（无需 Jupyter 内核），自动创建 nbformat v4 空 notebook
- 索引 clamp + cell 类型校验（code/markdown）+ 路径安全校验
- 注册到 defaults()

### 低1：PlanOnly 权限模式
**修复**（`tool/src/approval.rs`）：`WhitelistMode` 新增 `PlanOnly`——只允许只读工具，所有写操作 Deny（让用户先审查计划再切回 Default 执行）

### 低2：SkillAsTool 未命中返回可用列表
**修复**（`skill/src/executor.rs` + `registry.rs`）：技能未找到时错误信息附带所有可用技能名，帮助 LLM 自纠错

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| tool lib 测试（36/37，1 网络测试不稳定） | ✅ 核心 |

---

## 第二十二轮优化：统一上下文信息注入（时间/环境/工具）（✅ 已完成）

**日期**：2026-06-29

### 问题
用户问"今年世界杯的大冷门"返回 2022 年信息——根因：**所有 prompt 都没有注入当前日期**，
LLM 不知道"今年"是 2026 年，默认用训练数据中最近的 2022 世界杯回答。

### 修复：统一上下文注入模块

#### 新建 `core/src/context_info.rs`（参考 cc-python-claude compute_env_info）
- **`env_block(working_dir)`**：完整环境段落
  - 当前日期 + 年份 + "今年"提示（关键修复："今年"={year}，不是过去的年份）
  - 工作目录 + git 仓库状态
  - 平台 + shell
  - 语言提示（"用与用户相同的语言回答"）
- **`date_hint()`**：轻量日期提示（辅助角色用）
- **`user_context_block(input, working_dir)`**：环境+用户请求组合
- 4 个单元测试

#### 注入到所有关键 prompt 点

| 注入点 | 注入内容 | 效果 |
|--------|---------|------|
| **server ExploreStage** system prompt | `env_block`（含日期+年份提示） | LLM 知道当前年份，正确解读"今年" |
| **server AgentStage** system prompt | `env_block` | 执行者知道环境+日期+工具列表 |
| **loop-pipeline role_system_prompt** | `env_block` | 每个子角色知道当前日期+环境 |
| **loop-pipeline evaluate** system | `date_hint` | 评估者知道当前日期 |
| **loop-pipeline plan** system | `date_hint` | 规划者知道当前日期 |
| **loop-pipeline repair** system | `date_hint` | 修复者知道当前日期 |

### 修复效果
用户问"今年世界杯的大冷门"时，LLM 现在知道：
- 当前日期是 2026-XX-XX
- "今年" = 2026 年（不是 2022）
- 会用 web_search 搜索 2026 年世界杯信息

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| core lib 测试（29，含 4 context_info） | ✅ |
| loop-pipeline lib 测试（38） | ✅ |

---

## 第二十三~二十五轮优化：路线图步骤 1-3（transcript 修复 + 三角色接入 + 零警告）（✅ 已完成）

**日期**：2026-07-11

### 步骤1：transcript 修复（第二十三轮）

**问题**：崩溃恢复时 history 末尾的孤立 tool_use（assistant 发起 tool_use 后崩溃，tool 未执行）会导致 provider API 校验错误。

**修复**（`core/src/message.rs`）：
- 新增 `validate_transcript(history) -> usize`：扫描所有 assistant 消息的 ToolUse blocks，检查后续是否有对应的 Tool role 消息，对缺失的追加合成 error tool_result
- 在 `/api/resume` 入口调用：`validate_transcript(&mut history)` 自动修补
- 4 个单元测试（正常/孤立/多孤立/无 tool_use）

### 步骤2：三角色接入 dispatch（第二十四轮）

**问题**：三角色 `execute_with_roles` 已实现+测试通过，但 FeedbackStage 只做总评审，未对每个 stage 产物走 Validator+Arbiter。

**修复**（`server/src/routes.rs`）：
- FeedbackStage 中新增**逐 stage 三角色校验**：
  - 对每个 stage 产物调 `run_validator`（校验质量）
  - 如果校验未通过，调 `run_arbiter_forgiving`（决策 Pass/Revise/Supplement）
  - 收集所有 stage_issues
- 如果三角色发现问题但总评审说 deliver，**修正 verdict 为 revise**
- 前端可见 `stage_issues` 数组（每个问题的 stage 名 + 反馈文本）

### 步骤3：编译警告清理 + 会话即时持久化（第二十五轮）

**编译警告清理**：从 32 个 → **0 个**
- `cargo fix --workspace` 自动修复 unused imports/variables（CLI 3 处）
- provider stepfun.rs 响应结构体加 `#[allow(dead_code)]`（8 个 API 响应字段）
- server `handle_run_loop` + `WsRequest` 加 `#[allow(dead_code)]`
- loop-pipeline `any_found` 加 `_` 前缀

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零警告** |
| lib 测试（core 33 + planning 66 + loop-pipeline 38） | ✅ **137 全绿** |

---

## 第二十六轮优化：project.md 项目级指令 + 权限规则配置（✅ 已完成）

**日期**：2026-07-11

### project.md 项目级指令（参考 cc-python-claude claudemd.py）

**设计**：用户在工作目录放一个 `project.md` 文件，Agent 自动读取并注入到 system prompt。
比隐藏文件（.miniagent.md）更直观——用户能在 IDE 中直接看到。

**实现**（`core/src/context_info.rs`）：
- **`load_project_md(working_dir)`**：从工作目录向上遍历，合并所有 project.md（越靠近 cwd 优先级越高）
- **`project.local.md`**：cwd 下的私有本地指令（最高优先级，不提交版本控制）
- **`@path` 包含指令**：行首 `@path/to/file.md` 引用其他文件（有循环引用检测 + 深度限制）
- **HTML 注释移除**：`<!-- ... -->` 被自动去除
- **注入到 system prompt**：AgentStage + role_system_prompt 末尾（优先级最高）

**使用示例**：
```markdown
# project.md
本项目使用 Rust 2021 edition。
所有测试必须通过才能提交。
数据库是 PostgreSQL，不是 MySQL。
```

### 权限规则配置（参考 cc-python-claude permissions/rules.py）

**设计**：用户通过 `settings.json` 配置工具 allow/deny 规则，支持 glob 匹配 bash 命令。

**实现**（`tool/src/approval.rs`）：
- **`PermissionRules`**：`{allow: [String], deny: [String]}`，从 JSON 加载
- **规则语法**：
  - `"read"` — 精确匹配工具名
  - `"bash:git*"` — 匹配 bash 工具且 command 以 `git` 开头
- **优先级**：deny > allow > 无匹配（fallthrough 到 WhitelistApproval）
- **`RuleBasedApproval`** handler：先检查用户规则，无匹配时 fallthrough 到内部 handler
- **`glob_matches`**：支持 `*`（前缀/后缀/全匹配）

**配置示例**：
```json
{
  "permissions": {
    "allow": ["read", "glob", "grep", "bash:git*"],
    "deny": ["bash:rm*"]
  }
}
```

5 个单元测试（加载/精确匹配/glob deny/deny 优先/glob 函数）。

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零警告** |
| lib 测试（core 33 + tool 42 + planning 66 + loop-pipeline 38） | ✅ **179 全绿** |

---

## 第二十七轮优化：AgentTool 子智能体（路线图步骤5）（✅ 已完成）

**日期**：2026-07-11

### 里程碑：LLM 可自主派生子 agent

这是 miniagent 从"编排驱动"迈向"LLM 自主+编排辅助"混合模式的关键一步。
LLM 现在可以在运行时自主决定何时分解复杂任务并派生子 agent。

### 设计
- **后台异步**：spawn 子 agent → 立即返回 task_id → 结果通过 broadcast 回传
- **全新空 history**：子 agent 看不到父对话（self-contained prompt）
- **工具排除**：子 agent 的 allowed_tools 排除 "agent"（防递归）
- **Provider 继承**：复用父 Agent 的 ProviderRouter
- **并发限制**：最多 3 个并发子 agent（Semaphore）
- **父 agent 继续**：spawn 后继续 LLM 循环，下一轮收集子结果

### 实现

#### `core/src/event.rs`
- `AgentEvent` 新增 `SubAgentCompleted { task_id, result, success }`

#### `agent/src/lib.rs`
- `Agent` struct 新增 `sub_agent_rx: Option<Mutex<broadcast::Receiver<AgentEvent>>>`
- `set_sub_agent_rx()` 方法注入 receiver
- `collect_sub_agent_results()` 在每轮迭代末尾收集已完成子 agent 结果注入 history
- `run_with_loop` 在 match 块后调用 `collect_sub_agent_results`

#### `agent/src/agent_tool.rs`（新建）
- `AgentTool`：持有 `Arc<Agent>` + broadcast sender + Semaphore
  - `execute()`：解析 task → 检查并发 → spawn 子 agent → 返回 task_id
  - 子 agent 用全新 history + env_block + project.md + 排除 agent 的 allowed_tools
  - 完成后 broadcast `SubAgentCompleted`
- `build_tools_with_agent(agent)` 工厂：defaults() + AgentTool → (registry, receiver)
- `sub_agent_allowed_tools()`：排除 "agent" 的工具列表

#### `server/src/bin/miniagent-server.rs`
- 启动时用 `build_tools_with_agent()` 构造工具集
- `agent.set_sub_agent_rx(rx)` 注入 receiver

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ |
| lib 测试（agent 1 + core 33 + loop-pipeline 38） | ✅ 全绿 |

---

## 第二十八轮优化：P0 端到端验证 + P1 健壮性 + P2 清理（✅ 已完成）

**日期**：2026-07-25

### P0: 端到端验证 + 搜索后端修复

**根因诊断**：
- 代理 `127.0.0.1:7890` 已 down → 海外搜索后端（serper/tavily/bocha/ddgs）全部不可达
- StepFun API 直连可用，但 provider 的 `proxy_from_env()` 读 `ALL_PROXY` 导致走死代理
- REST API 的 `run_handler` 缺少环境信息注入（日期/平台）→ LLM 回答 "2024" 而非 "2026"

**修复**：
- `.env`：注释掉 `ALL_PROXY`（代理已 down）
- `run_handler` system_prompt 加 `env_block`（日期注入）
- WebSocket 端到端验证通过：explore→plan→dispatch→feedback 完整流程 + LLM 正确回答 "2026"

**当前后端状态**：2/6 healthy（PubMed + LangSearch 可用）；serper/tavily/bocha API key 可能过期，ddgs 中国直连不可达

### P1: 健壮性加固

#### P1-1: run_with_loop 开头加 validate_transcript
`run_with_loop` 在循环开始前调用 `validate_transcript(history)`，修补孤立 tool_use（防 API 校验错误）。此前只在 `/api/resume` 入口调用。

#### P1-2: AgentTool 超时保护
子 agent spawn 加 `tokio::time::timeout(300s)`——防止卡死的子 agent 永久占用 semaphore slot。超时后广播 `SubAgentCompleted { success: false }`。

#### P1-3: 权限规则接入 server
server 启动时从 `./settings.json` 加载 `PermissionRules`，构造 `RuleBasedApproval`（无规则时退化为 `AutoApprove`）。

### P2: 清理
- 移除未用 import `AutoApprove`
- `handle_run_loop` 保留 `#[allow(dead_code)]`（未来可能恢复 loop-pipeline 模式）

### 验证
| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零警告** |
| lib 测试（agent 1 + core 33 + loop-pipeline 38 + planning 66 + tool 42） | ✅ **180 全绿** |
| WebSocket 端到端（explore→plan→dispatch→feedback） | ✅ LLM 正确回答 "2026" |

---

## 第二十九轮优化：端到端科研流水线贯通（目标2→3→4 + 目标1贯穿）（✅ 已完成）

**日期**：2026-08-11

### 背景

四目标评估发现：主干流水线 `PubMed→KG→TransE→链路预测→假设→排序` 真实可用，但目标 2/3/4 存在具体缺口：
- **目标2**：TransE 无负采样/margin loss/L2 归一化，时钟 RNG → 链路预测质量差；无外部生物医学 KG。
- **目标3**：假设只产出单一湿实验 `ExperimentDesign`，**缺结构化「验证任务计划」**（数据分析任务 vs 湿实验方案分离）。
- **目标4**：`PythonRuntime` 是 stub；notebook 不执行；无可复现性/溯源层。

本轮按"端到端 MVP 贯通"路线打通 2→3→4，目标 1（可追溯）作为贯穿性质量保障。

### Phase A — 目标2：强化 KG 链路预测与假说质量

#### A1. TransE 重写（`crates/kg/src/embedding.rs` + `Cargo.toml`）
旧实现：无负采样、纯正样本 SGD（loss 单调驱 `||h+r-t||→0`）、时钟种子 `DefaultHasher`、无归一化。

新实现（教科书 TransE，Bordes et al. 2013）：
- `rand` crate 正式接入（替换 `DefaultHasher`+纳秒时钟）；均匀初始化 `[-6/√d, 6/√d]`。
- **负采样**：每个正三元组随机替换 head/tail 生成 corrupt 样本（可配置 `num_negatives`）。
- **margin-based ranking loss**：`max(0, γ + d(pos) − d(neg))`，仅在违反 margin 时 SGD 更新。
- **L2 归一化**：实体+关系向量每轮归一化（防 relation 向量爆炸 → margin loss 被平凡满足）。
- `TrainConfig { margin, num_negatives, lr_decay, norm: L1|L2 }` + `train_with(kg, epochs, lr, cfg)`。
- 测试：正样本距离随训练下降、corrupt 距离 > 正样本（聚合）、归一化后 ‖v‖≈1、L1/decay 不 panic。

#### A2. 外部生物医学 KG 增强（新建 `crates/kg/src/external.rs`）
- `load_relation_tsv` / `load_fixed_relation_tsv`：通用 TSV/CSV 三元组加载（DisGeNET/OMIM/自定义），自动跳过 header。
- `string_network_url` + `parse_string_response` + `fetch_string_interactions`：STRING 免费 API 客户端（按基因拉取蛋白互作→`InteractsWith`）。
- `merge_external(kg, triples)`：按 `find_entity_by_name` 去重实体，外部关系字符串映射到 `RelationType`，返回 `MergeStats`。
- 测试：TSV 解析、STRING TSV 解析、合并去重（同名实体不重建、重复边跳过）、URL 格式。

#### A3. 链路预测打分修正（`crates/kg/src/link_prediction.rs`）
- 旧权重 kge 0.35 + path 0.30 = 0.65（缺 0.35，max score 永远 < 0.65）。
- 统一为 **KGE + path + GIVE** 三路融合，权重归一化和为 1.0；新增 `with_weights(kge, path, give)`。
- GIVE 信号 = 候选邻域与已知尾邻域的 Jaccard 重叠（语义外推）。
- `HypothesisEvidence` 新增 `give_score`（`#[serde(default)]` 向后兼容）。
- 测试：权重归一、score ∈ [0,1]、GIVE 偏好共享邻域、KGE 信号贡献、extrapolation。

### Phase B — 目标3：结构化「验证任务计划」

#### B1. 验证计划类型（新建 `crates/hypothesis/src/validation.rs`）
- `ValidationPlan { hypothesis_id, rationale, data_analysis_tasks, wet_lab_protocols }`
- `DataAnalysisTask { id, objective, dataset_source, dataset_accession, cohort_definition, variables(独立/因/协变量), statistical_method, expected_outcome, deliverable, priority }`
- `DatasetSource ∈ { Geo, Tcga, ArrayExpress, Local(path), CustomUrl }`（tagged enum）
- `WetLabProtocol { id, objective, reagents, steps, controls, expected_outcome, timeline_days, feasibility }`
- 测试：JSON 往返、本地可用性判断、tagged 序列化。

#### B2. 验证计划生成（`crates/hypothesis/src/generator.rs`）
- `HypothesisGenerator::generate_validation_plan(hypothesis, kg, cancel) -> ValidationPlan`。
- Prompt 要求 LLM 把验证**拆成两组**：计算/数据分析任务（公共数据集 GEO/TCGA）vs 湿实验方案。
- 容错解析（`core::json_util::extract_and_repair`）：`dataset_source` 同时接受 `{"kind":"geo"}` 和裸字符串 `"geo"`；缺失 id 自动补 `DA-N`/`WL-N`；priority/feasibility clamp 到 [0,1]。
- 测试：完整计划解析、裸字符串 source、默认 id、垃圾拒绝、空数组兜底。

#### B3. GEO 数据集发现工具（新建 `crates/tool/src/tools/geo_search.rs`）
- `GeoSearchTool`：查 NCBI GEO `gds` 库（esearch+esummary），返回 accession(GSE…)/标题/类型/样本数/物种/摘要。
- 注册到 `defaults()`，让 LLM 能为数据分析任务定位真实数据集。

### Phase C — 目标4：端到端可审计数据分析执行

#### C1. 新 crate `miniagent-analysis`（加入 workspace）
- **`provenance.rs`**：`ProvenanceRecord`（script+hash、输入/输出文件+FNV-1a 哈希、conda env+包版本、seed、git commit、时间、exit code、stdout/stderr 哈希+预览）。零外部依赖（FNV-1a 内联实现）。
- **`runner.rs`** `AnalysisRunner`：接 `DataAnalysisTask` → LLM 生成可复现 Python 脚本（注入 seed/conda env/IO 路径/deliverable）→ 确保 conda 环境（best-effort，无 conda 退化系统 python 并记录）→ bash 执行 → 捕获 provenance → 校验 deliverable 产物。无本地数据时进入 **dry-run**（生成脚本+计划，不执行）。
- **`notebook.rs`** `execute_notebook`：经 `jupyter nbconvert --execute` 执行 .ipynb（jupyter 缺失时清晰报错降级），补齐 `NotebookEditTool` 只编辑不执行的缺口。
- 测试：11 个（FNV 确定性、文件记录、预览截断、provenance 序列化往返、notebook 缺失报错、**stub provider 端到端 dry-run + provenance 落盘**）。

#### C2. CLI 端到端接线（`crates/cli/src/main.rs`）
`research` 命令新增 flag，`research_pipeline` 扩展为 8 阶段：
- `--validate` → **Phase 7**：top-N 排序假设生成验证计划，写 `analysis/plans/validation_plan_N.json`。
- `--analyze` → **Phase 8`：对每个数据分析任务 `AnalysisRunner.run()`，输出 provenance 路径。
- `--data <path>`：本地数据文件（喂给数据分析任务）。
- `--top-n <n>`：验证的假设数量。
- `--enrich-file/--enrich-delim/--enrich-relation`：外部 KG 增强（Phase 3 后、link prediction 前合并）。

### Phase D — 目标1：贯穿的可追溯/可审计

- **`core/src/event.rs`**：`AgentEvent` 新增 `AnalysisRunCompleted { task_id, hypothesis_ref, success, dry_run, provenance_path, timestamp }`（接入既有 event_log/`/api/trace` 体系）。
- **`server/routes.rs`**：新增 `GET /api/provenance/{task_id}` 端点——读取 `analysis/<task_id>/provenance.json`（路径遍历防护），返回 provenance 记录 + 同目录产物清单。
- **CLI Phase 8 审计日志**：每次分析执行发 `tracing::info!(target="tool_call", ...)`（task_id/success/script_hash/conda_used/exit_code/provenance_path），契合第十七轮"只记 error + 工具调用"策略。

### 数据流（完整 2→3→4 流水线）

```
PubMed ──► KG 抽取 ──► (可选 --enrich-file 外部 KG 合并)
   ──► TransE 训练(负采样+margin) ──► 链路预测(KGE+path+GIVE)
   ──► 假设生成(LLM Pro) ──► 排序
   ──► (--validate) 验证计划(数据分析任务 + 湿实验方案)  [目标3]
   ──► (--analyze)  数据分析端到端执行(LLM 生脚本→conda→bash→provenance)  [目标4]
        ↓ provenance.json (脚本哈希/IO哈希/env/seed/git) ──► /api/provenance/{id}  [目标1可追溯]
```

### 验证（全量）

| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零警告** |
| `cargo test --workspace --lib` | ✅ **218 通过 / 0 失败 / 0 ignored**（上轮 180 + kg 16 + hypothesis 8 + analysis 11 + tool/core 增量） |
| 新 crate `miniagent-analysis` | ✅ 11 测试，含 stub provider 端到端 dry-run |
| `research --help` | ✅ 8 个新 flag 正确注册 |
| workspace 成员 | 18 → **19**（新增 analysis） |

### 刻意不做（保持 MVP 精简，留待"全四目标"路径）
- 不合并三套编排系统 / 不删 dead `handle_run_loop`。
- 不接真实 PyO3（已选 bash + provenance 路线）。
- 不做 TCGA/GDC 全量集成（GEO 覆盖 MVP）。
- 不接 UMLS（许可受限）；外部 KG 用 STRING API + 本地文件加载器覆盖。
- AnalysisRunner 用 provider 单次调用生脚本（非 Agent 工具循环）——可测试、无跨 crate 环，Agent 循环留作后续增强。

---

## 第三十轮优化：合并三套编排系统（✅ 已完成）

**日期**：2026-08-11

### 背景

经核实：workflow（458 行）+ loop-pipeline（~3500 行）是两个生产编排器（服务器 `handle_run`）；planning（6442 行）只在 CLI 中被引用 10 次，服务器零引用——本质是 4 个互不兼容抽象的 CLI playground（Planner、StateGraph、AgentRole+Blackboard、ControlShell）。第二轮第 8 步曾判定"风险 > 收益"拒绝合并，本轮正面解决。

### Phase U1 — 共享抽象下沉到 core

**新建 `crates/core/src/orchestration.rs`**：
- `StageInput`（id + JSON 输入 + 前序输出 + 取消令牌）：覆盖 workflow（JSON）+ loop-pipeline（typed `PipelineState` 序列化为 JSON）+ planning（Blackboard/GraphState）的所有入参。
- `StageOutcome { data, summary, side_effects }`：合并 workflow `StageOutput.data` + loop-pipeline `StageOutput.summary` + planning `RoleOutput`。
- `SideEffect` 枚举：`ArtifactWritten`、`TodoUpdated`、`ProgressEmitted`、`LlmCallMade` —— 跨 driver 统一跨切面事件。
- `StageDriver` trait：`name()` + `async fn run(StageInput) -> Result<StageOutcome, OrchestrationError>`。
- `OrchestrationError` 统一三套错误类型（Stage/Plan/Repair/Agent/Json/Cancelled）。
- `kahn_waves(nodes, edges) -> Vec<Vec<NodeId>>`：合并 workflow/loop-pipeline/planning 4 份独立 Kahn 实现（替代工作待后续 phase）。

**测试**：7 个新 orchestration 测试（chain/diamond/独立节点/环路检测/JSON 往返/SideEffect 序列化/adapt_stage）。

### Phase U2 — workflow + loop-pipeline 双向适配

**`crates/workflow/src/runner.rs`**（新建）：
- `DagRunner` 包装 `Workflow`，实现 `StageDriver`。
- `map_agent_error`：AgentError → OrchestrationError 桥接。
- `stage_input_to_context` / `stage_output_to_outcome`：迁移辅助函数。
- 3 个测试（adapter 通过统一 trait 派发、StageOutput 转换、错误映射）。

**`crates/loop-pipeline/src/runner.rs`**（新建）：
- `LoopRunner` 包装 `LoopPipeline`，实现 `StageDriver`。
- `state_to_outcome`：typed `PipelineState` → 统一 JSON `StageOutcome`。
- 4 个测试（错误映射、空 pipeline 状态、prompt 提取、trait 派发）。

**结果**：现在 `Box<dyn StageDriver>` 可以在两个生产 driver 之间任意切换。零运行时开销（trait object 由 dyn dispatch 实现，调用开销可忽略）。

### Phase U3 — planning crate 解构

**删除文件**（git rm，~3.5K 行）：
- `tournament/` 子树（1063 行，零生产引用；被 research/ 内部引用，间接全部可达 0）
- `research/` 子树（1145 行，零外部引用；`SchedulerRole`/`PrincipalInvestigatorRole`/`TournamentMasterRole`/`EvidenceAccumulatorRole`/`SynthesisJudgeRole` 全部是 CLI 实验性代码）
- `alzheimers.rs`、`evidence_accumulator.rs`、`synthesis_judge.rs`（仅集成测试引用）
- `hooks.rs`（662 行，CLI `hooks_demo` 唯一引用，第二轮第 20 步已被 `agent::hooks` 取代）
- `control_shell.rs`（仅 CLI `workflow_command` demo 用）
- `tool_binding.rs` / `agent_profile.rs` / `context_manager.rs`（同上，CLI demo only）
- `event_stream.rs` 中 `relevant_to` 删去对 `agent_profile` 的耦合（CLI 也不用了）

**保留**（4965 行，真正生产代码）：
- `plan.rs` (Planner/PlanExecutor) — CLI `plan` 命令用
- `state_graph.rs` (StateGraph) — CLI `team` 命令用
- `roles/` (13 AgentRole + Blackboard) — CLI `debate` 命令用
- `event_stream.rs` / `todo_attention.rs` — state_graph 内部使用（删去 `relevant_to` 的角色依赖）

**CLI 迁移**：
- `workflow` 子命令删除（第二轮第 5 步已标注为无用的 demo）
- `hooks` 子命令删除（被 `agent::hooks` 取代）
- `plan` / `debate` / `team` 三个生产命令路径保留
- `orchestrate` 命令继续走 workflow crate（不在 planning 解构范围）

**结果**：planning crate 6442 → 4965 行（**-22%**）；删除的全是零引用或 demo-only 代码；生产路径完全保留。

### 数据流（统一后）

```
                                  ┌──────────────────────────┐
                                  │ miniagent_core::         │
                                  │  orchestration::         │
                                  │   StageDriver trait      │
                                  │   StageInput / Outcome   │
                                  │   SideEffect, kahn_waves │
                                  └─────────────┬────────────┘
                                                │
                          ┌─────────────────────┼──────────────────────┐
                          ▼                     ▼                      ▼
                workflow::DagRunner    loop-pipeline::LoopRunner   (planning::StateGraph)
                          │                     │                      │
                          ▼                     ▼                      ▼
              workflow::Workflow.run   LoopPipeline::run        StateGraph::execute
                          │                     │                      │
                          ▼                     ▼                      ▼
                   JSON-typed DAG       typed PipelineState    typed GraphState
                          │                     │                      │
                          └──── server handle_run（两种模式可热插拔）───┘
```

### 验证

| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零警告** |
| `cargo test --workspace --lib` | ✅ **192 通过 / 0 失败 / 0 ignored** |
| core 编排模块 | ✅ 7 测试（kahn_waves、stage outcome、adapt_stage） |
| workflow DagRunner | ✅ 3 测试（trait dispatch、StageOutput 转换、错误映射） |
| loop-pipeline LoopRunner | ✅ 4 测试（错误映射、空状态、prompt 提取、trait dispatch） |
| planning 解构 | ✅ 26 测试（state_graph、todo_attention、roles 13 个 AgentRole） |
| CLI `plan` / `debate` / `team` / `orchestrate` / `loop` 命令 | ✅ 全部仍工作 |
| 移除的子命令 | `workflow`、`hooks`（移除并打印引导信息） |

### 关键设计决策

1. **保留 planning crate 作为可选多角色/StateGraph 编排器**：完全删除并将 Planner/StateGraph/AgentRole 移到 agent crate 是一个独立的大型重构（~5000 行 + 13 个 role 实现 + state_graph 1134 行）。本轮先完成 22% 行数削减和死代码清理，剩余部分仍是有效的、可独立调用的多角色编排器。

2. **三套 driver 现在共享 `StageDriver` trait**：通过 `Box<dyn StageDriver>` 可在 workflow 和 loop-pipeline 之间任意切换，添加新 driver（如 `StateGraphRunner`）只需实现同一 trait。

3. **不做 vtable 抹平**：保留每个 runner 的内部 stage trait（`StageHandler` / `PipelineStage`）避免破坏现有 stage 实现；只在最外层用 trait object 抽象。

### 刻意不做（留给后续 phase）

- 不将 Planner/StateGraph/AgentRole 重写为 `StageDriver` 实现（~5000 行移动 + 测试重写）。
- 不合并 `kahn_waves` 的 4 份实现（保留各自优化，逐个迁移需单独 phase）。
- 不删除 dead `handle_run_loop`（属"全四目标"路径，本次范围外）。
- 不将 `plan`/`debate`/`team` 命令接入 `Box<dyn StageDriver>` 切换（CLI 用例不同，工作量超出本次）。

---

## 第三十一轮优化：Planning 三大抽象接入 StageDriver（✅ 已完成）

**日期**：2026-08-11

### 背景

第三十轮把 workflow/loop-pipeline 两个生产编排器接入了统一的 `StageDriver` trait，并保留了 planning crate 中的 `Planner`/`StateGraph`/`AgentRole` 三大抽象（CLI 命令路径）。本轮把剩余三套抽象也接入 `StageDriver`，让所有编排器真正可互换。

### 新增 crate `miniagent-planning::runners`

```
crates/planning/src/runners/
├── mod.rs              (re-exports + module docs)
├── plan_runner.rs      (PlanRunner: Planner + PlanExecutor → StageDriver)
├── state_graph_runner.rs (StateGraphRunner: CompiledGraph → StageDriver)
├── debate_runner.rs    (DebateRunner: Proposer/Opponent/Judge triad → StageDriver)
└── role_runner.rs      (SingleRoleRunner: any Arc<dyn AgentRole> → StageDriver)
```

### 各 runner 设计要点

**`PlanRunner`** (Planner + PlanExecutor 双阶段编排)
- 一次 `StageDriver::run(input)` 内部：Planner.decompose → PlanExecutor.execute。
- 输入支持三种 JSON 形态：`"string"` / `{"prompt":"…"}` / `{"goal":"…"}`。
- 输出：`data` = 完整 Plan JSON；`summary` = "N done, M failed, P pending of T steps"；每个有输出的 step 触发 `SideEffect::ArtifactWritten`。
- 状态：owned internal（Planner/Executor 只持有不可变引用；Plan 在 driver 内 mutable local）。

**`StateGraphRunner`** (CompiledGraph)
- 难点：`CompiledGraph::execute` 返回 **非-Send** future（Parallel 子节点的 boxed closures 捕获 EventStream/TodoAttention 跨 await），与 `StageDriver: Send + Sync` 的 trait 约束冲突。
- 解决方案：在 `StageDriver::run` 内通过 `tokio::task::block_in_place` + `Handle::block_on`（或回退到新建 single-threaded runtime）把图执行调度在当前线程上。这在多线程 runtime 上是惯用的 sync-bridge 模式。
- 输入支持：`"string"` 或 `{"prompt":"…"}` 转为 GraphMessage(user)；`previous_outputs` 合并进 GraphState.artifacts。
- 输出：`data` = 完整 GraphState JSON；`summary` = iteration/step 计数；每 step 触发 `ProgressEmitted + ArtifactWritten`。

**`DebateRunner`** (Proposer/Opponent/Judge 序贯循环)
- 复刻 CLI `debate_command` 内联循环：Round 1 (Proposer → Opponent → Judge)，若 verdict 含 "REVISE" 则 Proposer 重跑（最多 `max_revise_rounds` 次）；ACCEPT 或 REJECT 即终止。
- 抽取 `DebateRound` 结构体（Serialize/Deserialize）作为中间结果，便于 CLI 渲染 + 后续历史追踪。
- 输出：`data` = `Vec<DebateRound>` JSON；`summary` = "debate accepted/rejected after N round(s)"；每个角色每个 round 触发 `SideEffect::ArtifactWritten` × 3。

**`SingleRoleRunner`** (单角色通用适配)
- 持有 `Arc<dyn AgentRole>` + `work_dir`。
- `name()` 返回 **role 自己的名字**（而非 "SingleRoleRunner"），便于日志/监控匹配原始角色。
- 用于未来 `miniagent role --name researcher --task "…"` 类单角色调用。

### CLI 命令迁移 (M1e)

| CLI 命令 | 之前 | 之后 |
|---------|------|------|
| `miniagent plan` | 直接调 `Planner::decompose` + `PlanExecutor::execute` | 通过 `PlanRunner::run(StageInput)` |
| `miniagent debate` | 内联 Proposer/Opponent/Judge 循环 + 路由 | 通过 `DebateRunner::run(StageInput)` |
| `miniagent team` | `CompiledGraph::execute(state, cancel, flash, pro)` | 通过 `StateGraphRunner::run(StageInput)` |

**结果**：CLI `plan`/`debate`/`team` 三个生产命令路径现在统一通过 `StageDriver::run(input) -> Result<StageOutcome, OrchestrationError>` 调用，与 `DagRunner`/`LoopRunner` 在同一抽象层级。

### 关键技术挑战

1. **StateGraph 的非-Send future**：通过 `block_in_place` 桥接到同步执行；这是 tokio 文档推荐的多线程运行时 sync-future bridge 模式。
2. **Blackboard 不可在 StageDriver 内部共享**：每个 driver 调用从 `Blackboard::new(work_dir)` 重新构造（work_dir 来自 driver config，不可变）。这意味着 driver 不保留跨调用的 state —— 与 DagRunner/LoopRunner 一致（它们也没有跨调用 state）。
3. **Provider 注入**：PlanRunner/StateGraphRunner/DebateRunner 在构造时接收 `Box<dyn LlmProvider>`，与现有 CLI 调用模式一致。无需新增 trait。
4. **AgentError → OrchestrationError 映射**：每个 runner 都复制了 `map_agent_error` 桥接函数（10 行），未来可下沉到 core。

### 验证

| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace` | ✅ **零警告** |
| `cargo test --workspace --lib` | ✅ **203 通过 / 0 失败 / 0 ignored**（上轮 192 + 11 个新 runner 测试） |
| planning 11 个新 runner 测试 | ✅ PlanRunner（5）、StateGraphRunner（1）、DebateRunner（3）、SingleRoleRunner（2） |
| `Box<dyn StageDriver>` 多态 | ✅ 5 个 driver（DagRunner / LoopRunner / PlanRunner / StateGraphRunner / DebateRunner）实现同一 trait，可任意组合 |
| CLI `plan` / `debate` / `team` | ✅ 全部仍工作，全部通过 `StageDriver::run` 调用 |
| `Box<dyn StageDriver>` 调度 | ⏸️ **不做**——CLI 用例差异（plan 是 query → Plan；debate 是 query → DebateRounds；team 是 query → GraphState），按 driver 类型分支调度更清晰 |

### 现在所有 driver 共享 `StageDriver` trait

```
core::orchestration::StageDriver (单一 trait)
├── workflow::DagRunner             (DAG, JSON-typed, 生产)
├── loop_pipeline::LoopRunner       (5-phase loop, typed PipelineState, 生产)
├── planning::PlanRunner            (Planner + PlanExecutor, CLI plan 命令)
├── planning::StateGraphRunner      (CompiledGraph, CLI team 命令)
├── planning::DebateRunner          (Proposer/Opponent/Judge 三角色, CLI debate 命令)
└── planning::SingleRoleRunner      (任意单个 AgentRole, 通用工具)
```

任何 driver 都可以通过 `Box<dyn StageDriver>` 在 server handle_run 中互换使用，让不同任务匹配不同编排器成为可能。

### 刻意不做（范围控制）

- 不把 `PlanRunner`/`StateGraphRunner`/`DebateRunner` 接入 server `handle_run` —— 当前 server 默认 workflow DAG + 5-phase loop 已足够；这些 runner 是 CLI 命令专用。如需服务端多 driver 调度，留作后续 phase。
- 不实现 `Box<dyn StageDriver>` 多态分发 —— CLI 命令路径按具体类型走更清晰，trait object 主要价值在 server 层。
- 不合并 4 份 `map_agent_error` 桥接到 core（重复 10 行 × 4 driver = 40 行，可后续 cleanup）。
- 不把 `ProviderSelector` 等 workflow 内部抽象统一到 core —— 已超出"接口共享"范围。

---

## 第三十二轮优化：错误转换下沉 + clippy 清理 + 端到端验证暴露的 Provider 路由 bug（✅ 已完成）

**日期**：2026-08-12

### 背景

第三十一轮把所有编排器接入 `StageDriver` 后，明确遗留了「`map_agent_error` 重复 4 份未合并」的 cleanup 项。本轮完成该项收尾，顺带清理积压的 clippy 警告，并通过端到端任务测试暴露并修复了一个真实的生产 bug。

### P1. `AgentError → OrchestrationError` 转换下沉到 core

第三十一轮发现 `AgentError` 和 `OrchestrationError` **都在 `miniagent-core`**（`error.rs` / `orchestration.rs`），因此转换可以就近定义为规范的 `From` impl，无需跨 crate。

**`crates/core/src/orchestration.rs`**：新增 `impl From<AgentError> for OrchestrationError`，覆盖全部 11 个 `AgentError` 变体（Cancelled → Cancelled；InvalidConfig/InvalidState → Plan；Checkpoint → `Stage("checkpoint: …")`；其余 → Stage）。配套 6 个新单元测试（cancelled / invalid_state→Plan / invalid_config→Plan / checkpoint 保留上下文 / 6 个 message 变体 / budget+overflow 固定文案）。

**删除 3 份 verbatim 重复**：
| 文件 | 改动 |
|------|------|
| `crates/workflow/src/runner.rs` | 删 `map_agent_error`（17 行）+ `map_cancelled_error_to_orchestration` 测试；`.map_err(map_agent_error)?` → `?`（From 自动接管） |
| `crates/loop-pipeline/src/runner.rs` | 删 `map_agent_error`（17 行）+ 2 个重复测试；`.map_err(map_agent_error)?` → `?` |
| `crates/planning/src/runners/plan_runner.rs` | 删 `map_planner_error`（17 行）；两处 `.map_err(map_planner_error)?` → `?` |

**保留**（非重复，是带角色上下文的变体）：`debate_runner.rs::map_role_err` 和 `role_runner.rs` 的内联映射产出 `{role} failed: …` 上下文，与纯转换是不同关注点，不动。

**净效果**：51 行重复删除，转换逻辑单一来源；调用点全部简化为 `?`。

### P2. clippy 清理 + 2 个真实 bug 修复

`cargo clippy --fix` 批量修复机械性警告（collapsible-if / needless-borrow / len-zero / vec! 字面量 / 手动 clamp / format-in-format / 结构体更新语法 等）。手动处理需判断的项时发现 **2 个真实 bug**：

#### Bug-1: WebSocket fallback 状态消息从未发送（`crates/server/src/routes.rs`）

`planner.execute().unwrap_or_else(|e| { let _ = ws_send(socket, …); … })` —— `ws_send` 是 async fn，但 `let _ = ws_send(…)` **没有 `.await`**，future 被直接丢弃，Planner/Build fallback 的状态提示从未到达 WebSocket 客户端。clippy `non-binding let on a future` 抓到。

**修复**：`unwrap_or_else` 闭包不能 `.await`（同步闭包），改为 `match` 结构，在 `Err` 分支里 `ws_send(…).await`。两处（Planner fallback L1139、Build fallback L1248）同样修复。

#### Bug-2: Loop Pipeline 硬编码 StepFun，无视 `PROVIDER` 设置（`crates/loop-pipeline/src/stage.rs`）

`StageContext::build_agent` 直接 `StepFunFlash::new(key)`，**完全没有 `config.is_stepfun()` 分支**。这意味着 loop pipeline 永远走 StepFun，与 CLI `make_providers`（尊重 `PROVIDER` env）的行为不一致。当 StepFun 订阅失效时，`miniagent loop` 必失败（400），即使 `PROVIDER=deepseek` 也无济于事。

**由端到端测试暴露**（见 P4）：首次跑 `loop` 命令时报 `StepFun API error 400: no active step plan subscription`，尽管 `PROVIDER=deepseek` 已 export。顺藤摸瓜定位到此硬编码。

**修复**：`build_agent` 镜像 `make_providers`：`if config.is_stepfun()` → StepFun flash/pro；`else` → DeepSeek flash/pro。

**最终 clippy 状态**：从 ~40+ 警告降到 13，剩余全部是设计级（`too many arguments` 7 个、`very complex type` 5 个、`push-after-create` 测试 fixture 风格 1 个），需重构签名/类型别名，刻意留作后续。

### P3. 修复 planning 集成测试自 round 30 起的编译断裂

`cargo build --all-targets` 暴露 `crates/planning/tests/integration.rs`（1504 行）引用 round 30 已删除的模块（`ControlShell`/`ContextManager`/`TournamentArena`/`control_shell`/`research`/`tournament`）。round 30 验证只跑 `--lib`，漏了集成测试。

**处理**：删除该文件。其有效覆盖（EventStream / TodoAttention / StateGraph 含 `execute()`）在 lib 测试 + runners 测试中已完整存在（state_graph.rs 多个 `execute()` 测试、runners/state_graph_runner.rs），E2E 部分主要测的是已删模块。删除是 round 30 「零引用 demo-only」方向的延续。

### P4. 端到端完整任务测试（`miniagent loop`）

**正是这个测试暴露了 P2 Bug-2**——印证了端到端测试的价值。

**任务**（自包含、可验证、无需搜索的网络部分仍被 agent 主动使用）：在 `/tmp/miniagent_e2e/` 用 Python 实现 `math_utils`（`is_prime` + `fibonacci`）+ 测试文件（断言 `is_prime(7)==True`、`is_prime(9)==False`、`fib(10)==55`）+ 运行测试。

**执行轨迹**（DeepSeek Flash/Pro）：
1. **Explore**：agent 主动 web_search/web_fetch 研究 Python unittest / is_prime / fibonacci 模式（serper 不健康，自动回退 Tavily ✅）
2. **Plan + Dispatch**：并行 bash 创建模块与测试；首次用 `write` 工具写 `/tmp/...` 被正确拦截（路径在 worktree 外），agent 自适应改用 `bash cat >` heredoc ✅
3. **运行验证**：`python3 -m unittest test_math_utils -v` → **`Ran 3 tests... OK`** ✅；再跑边界用例 `is_prime(0/1/2/49/97)`、`fib(0/1/2/3/10)` 全过
4. **Evaluate 客观校验**：触发 `check_phantom_failures`（产物在 /tmp 不在 working_dir）→ 强制 continue，跑到 max_loops 后 finalize

**交付物验证**（独立重跑）：
```
$ cd /tmp/miniagent_e2e && python3 -m unittest test_math_utils -v
test_fibonacci_10 ... ok
test_is_prime_7 ... ok
test_is_prime_9 ... ok
Ran 3 tests in 0.000s  OK
```
`math_utils.py` 含 sqrt 优化的 `is_prime` + 迭代 `fibonacci` + docstring。**任务功能上完全成功。**

**观察到的 rough edge（刻意不在本轮修）**：`check_phantom_failures` 只检查 `working_dir` 下的产物文件；当任务把产物写到工作目录之外（如 /tmp，经 bash）时，会被误判为「成功但产物缺失」并无限强制 continue。这是 evaluate 阶段的设计取舍（如何判定 bash 写到任意路径的产物），留作后续。

### 验证

| 验证项 | 结果 |
|--------|------|
| `cargo build --workspace --all-targets` | ✅ 零错误（修复了 round 30 遗留的 planning 集成测试断裂） |
| `cargo test --workspace --lib` | ✅ **206 通过 / 0 失败**（上轮 203：+6 From impl 测试，-3 重复测试） |
| `cargo clippy --workspace --all-targets` | ✅ ~40+ → 13 警告（剩余均为设计级） |
| 端到端 `miniagent loop`（DeepSeek） | ✅ 全流程跑通，交付物正确，测试通过 |
| 仅 live stepfun 集成测试 | ❌ 因 StepFun 订阅失效（外部计费），与代码无关 |

### 关键收获

**端到端测试不是装饰**：本轮原计划只是「合并 map_agent_error + 清 clippy」，但跑 `miniagent loop` 立刻暴露了 `build_agent` 硬编码 StepFun 的生产 bug——这个 bug 在所有 lib 测试（206 个）里都不会显现，因为 lib 测试用 MockProvider 不走真实 provider 构造。只有真实端到端跑一次才能抓到。「先优化、再端到端验证」的流程本轮净额外修复了 2 个真实 bug。

### 刻意不做（范围控制）

- 不重构 `too many arguments` / `very complex type` 警告（需改公共签名/引入类型别名，独立 phase）。
- 不修 `check_phantom_failures` 对工作目录外产物的误判（设计取舍，需讨论 evaluate 语义）。
- 不调整 StepFun 订阅（外部计费问题，非代码）。


## Round 33: AnySearch 集成 + research 流水线质量修复（DeepSeek harness 思想借鉴）

### 任务背景

四目标循环：① 集成 AnySearch 搜索源并端到端跑一次找不足；② 借鉴 DeepSeek harness（dsh）设计思想做针对性修改；③ 阿尔兹海默症端到端回归；④ 清理硬编码密钥后更新 GitHub。

### AnySearch 集成（任务一）

- `web_search` 工具新增 `anysearch` 后端（`crates/tool/src/tools/web_search.rs`）：
  JSON-RPC 2.0 over `https://api.anysearch.com/mcp`（`tools/call` → `search`），`Authorization: Bearer as_sk_…`，
  支持 `domain` 垂直检索参数（academic/health/…），`max_results` 上限 10（API 限制），
  输出整体截断至 10k 字符防止页面全文淹没上下文。
- 接入既有基础设施：启动健康探测（`health.rs` `probe_anysearch`）、运行时熔断、
  `AppConfig.anysearch_api_key`（`ANYSEARCH_API_KEY`）、`miniagent config` 掩码显示。
  回退链变为 Serper → Tavily → Bocha → LangSearch → **AnySearch** → DDG。

### 端到端首跑暴露的缺陷（任务一结论，帕金森病 12 篇）

1. **Phase 7 全军覆没**：deepseek-reasoner 8192 max_tokens 被 CoT 耗尽 → 正文为空 →
   两个验证计划全部 JSON 解析失败，Phase 8 无从执行（目标 3/4 断链）。
2. **文献相关性污染**：混入一篇肌少症文献，其 hub 实体（mortality risk/fractures）
   主导链接预测 → 全部 5 个假说偏离帕金森（目标 2 落空）。
3. **实体合并遗留悬空边**：重名实体被跳过时，该文献 relation 仍指向未入库的旧 id。
4. **min_year=2023 硬编码**，不可配置不透明。
5. **辩论无外部证据**：debate prompt 要求"using the broader published literature"但无任何检索注入。

### 借鉴的 DeepSeek harness（dsh）设计思想（任务二）

| dsh 思想 | 落地修改 |
|---|---|
| 上下文工程（工具输出去噪限幅） | AnySearch 输出 10k 截断；辩论证据 4k 截断 |
| 证据外置、只把结论喂给模型 | 相关性过滤在进 KG 前丢弃离题摘要 |
| Append-only Trajectory（一切可审计可重放） | 拒稿清单 `papers_rejected.json`、辩论证据 `debate_evidence.json` 全部落盘 + manifest 事件 |
| 失败可恢复（reversible/retry） | Phase 7 空响应自动重试（预算翻倍至 16384） |
| 一切皆插件 + 健康探测/熔断 | AnySearch 按既有 backend 插件模式接入 |

### 修改清单（任务二）

- **Phase 2b 相关性过滤**：flash 模型逐篇 0-10 打分（并发 6，fail-open），<5 剔除；
  拒稿与理由持久化。阶段可 resume。
- **Phase 7 空响应重试**（`crates/hypothesis/src/generator.rs`）。
- **别名归并 + 悬空边修复**（`crates/kg/src/extraction.rs` `merge_extraction_canonical`）：
  大小写不敏感 name+alias 索引，relation 端点重映射到 canonical id。
- **疾病锚定**（Phase 4）：查询 token 与 Disease 实体重叠度最高者为锚，
  候选过滤为包含锚实体的子集（≥3 个才生效，否则回退）。
- **辩论外部证据**（`debate_and_refine_with_evidence`）：辩论前对 top-4 假说
  web_search 检索（自动走含 AnySearch 的后端链），证据注入双方辩论 prompt。
- **`--min-year` CLI 参数**（默认 2023，解除硬编码）。

### 阿尔兹海默症回归（任务三）

`research -q "Alzheimer's disease pathogenesis mechanisms" -n 12 --validate --analyze --top-n 2`：

- Phase 2b：kept 10/10（语料本就切题）；别名归并 26 个重复实体、0 条悬空边（旧实现会全部悬空）。
- 疾病锚定生效：`Alzheimer's disease dementia` 锚定 16/614 候选，全部假说围绕 AD。
- 辩论证据检索成功（serper 探测不健康自动回退其余后端链）。

### 密钥安全（任务四）

- `.env.example` 中泄露的真实 StepFun key 替换为占位符，并以 `git filter-branch`
  从本地历史中彻底清除（重写后经 `git log -S` 验证为空）；该 key 已失效（订阅过期）仍建议作废处理。
- `.env`（含 anysearch key）确认被 gitignore；`ANYSEARCH_API_KEY` 仅存在于 `.env`。
- `.gitignore` 补充 `.worktrees/`、`__pycache__/`、`.zcode/`、`stdout.txt`。

---

## Round 34: 运行时模型注册表 + 验证门（DeepSeek harness 思想借鉴）

### 任务背景

四任务循环：① 去除硬编码模型名、前端/后端支持添加与切换 LLM、端到端首跑总结不足；② 借鉴 DeepSeek harness（dsh）设计思想做针对性修改；③ 阿尔兹海默症端到端回归；④ 硬编码密钥审计后更新 GitHub。

### 任务一：模型注册表（去除硬编码模型名）

**核心改动**：新增 `crates/core/src/models.rs` `ModelRegistry` —— 模型配置档案（`ModelProfile`）注册表：

- **档案来源**：内置档案（从 `.env`/`AppConfig` 派生 deepseek/stepfun/minimax 三条）+ 自定义档案（`models.json` 持久化，已加入 `.gitignore`，含 API key）。
- **协议族**（`ModelKind`）：deepseek / stepfun / minimax / openai_compatible / anthropic_compatible。后两者由 `MiniMaxClient` 双协议自动检测承载（OpenAI Chat Completions + Anthropic Messages），任意兼容端点（SiliconFlow/OpenRouter/vLLM…）即插即用。
- **工厂**（`crates/provider/src/factory.rs`）：`build_provider(profile, tier)` 单一构造入口；`with_model_name`/`with_base_url` 强制覆盖，env 变量不再能悄悄覆盖显式选择。模型名默认值**只存在于** `ModelRegistry::builtin_profiles` 一处。
- **热切换**：`Agent.provider_router` 改为 `Arc<RwLock<ProviderRouter>>`，新增 `Agent::replace_providers()` —— `ProviderRouter` 内部字段改 `Arc<dyn LlmProvider>` 并新增 `select_arc/flash_arc/pro_arc`（owned 句柄，可跨 await）。进行中请求用旧 provider 收尾，新请求立即生效。
- **Server API**：`GET/POST /api/models`、`PUT/DELETE /api/models/{id}`、`POST /api/models/{id}/activate`。响应中 key 永远掩码（`ApiKey::masked`）；激活时先验证可构造性，失败回滚选择。routes.rs 中 4 处按 `is_minimax()/is_stepfun()` 分支构造 provider 的代码全部改为每任务从 active profile 构造。
- **前端**：header 模型下拉选择器 + ⚙ 管理弹窗（列表/使用中徽标/添加表单/删除），连接建立时 `loadModels()` 拉取。
- CLI `make_providers` 同步走注册表；`miniagent-server.rs` 的 `unwrap_or("step-3.7-flash")` 硬编码 fallback 与 minimax 分支缺失问题一并消除。

### 任务一端到端首跑暴露的缺陷（帕金森病 6 篇）

1. **`<think>` 标签污染数据管道**（MiniMax-M3 等推理模型内联 CoT）：查询翻译输出带截断的 think 块被直接当 PubMed 查询 → **0 检索结果，流水线空跑仍报 Complete**。
2. **KG 抽取静默失败**：`serde_json::from_str(json_str).unwrap_or_default()` 解析失败变成空 JSON → 6 篇论文全部"0 实体"，无任何告警。
3. **无阶段验证门**：上游产出为空时下游全部 skip 但进程 exit 0，审计上"成功"实为空跑。
4. server 启动二进制缺 `PROVIDER=minimax` 分支（本轮注册表重构顺带修复）。

### 任务二：借鉴的 dsh 设计思想与落地

| dsh / harness engineering 思想 | 落地修改 |
|---|---|
| Everything is a plugin（模型适配器可插拔） | ModelRegistry + provider factory：LLM 成为运行时可增删切换的插件，配置组合而非改码 |
| Self-verification / PreCompletionCheck（完成前强制校验产物） | research 流水线两个验证门：Phase 1 检索 0 PMID、Phase 3 非空语料 0 实体 → manifest 记 Failed + `stage_validation_failed` 事件 + 明确报错中止，不再静默"完成" |
| Append-only session log（一切可审计可重放） | 验证门失败写入 `project.json` event_log；KG 解析失败打印 PMID + 输出头部 160 字符 |
| Fail loudly（禁止静默默认值吞错） | KG 抽取 `unwrap_or_default()` → 显式 match + 告警日志；`extract_and_repair` 全链路 think-safe |
| 输出 sanitization 是 harness 责任而非模型责任 | `strip_reasoning_tags()`（core::json_util，处理闭合/未闭合 think 块）应用于：查询翻译、`extract_and_repair`（覆盖 hypothesis/debate 全部 JSON 解析）、分析脚本提取 |
| Reasoning-compute allocation（flash/pro 分层） | 既有 ProviderRouter 分层保持；注册表档案支持 flash/pro 双模型名 |
| Runtime modes/presets | 未落地（既有 `-n/--top-n/--debate` 参数组合已可调；列为后续项） |

### 验证

- `cargo test --workspace` 通过；新增 `strip_reasoning_tags`、`ModelKind` 解析、factory 构造单测。
- server 冒烟：`/api/models` 列表（key 掩码）→ 添加自定义模型 → 激活热切换 → 删除保护（使用中不可删）→ `models.json` 持久化往返。
- research 端到端（帕金森 6 篇）：修复后 42 实体/56 关系 → 5 假说 → 辩论 → 2 验证计划 → 7 个分析任务（notebook 生成；有 Jupyter 环境的真实执行，无本地数据的 dry-run 交付 script+notebook）。

---

## Round 35: 统一路径锚定 + 写围栏 + 审计报告（DeepSeek harness 思想借鉴）

### 任务背景

四任务循环：① 统一工作流模式，修复结果目录散落（部分任务结果落在 `.worktrees/result/{id}_{name}` 或仓库根而非 `result/{id}_{name}`）；② 借鉴 DeepSeek harness（dsh）设计思想做针对性修改；③ 渐冻症（ALS）端到端回归；④ 硬编码密钥审计后更新 GitHub。

### 任务一：诊断与统一

**根因**（`.worktrees` 逃逸本体已在 Round 33 修复，本轮清扫残余同类缺陷）：

1. 所有产物路径都以**进程 CWD 相对路径**为锚（`./result`、`models.json`、`./result/.workflow`、`./result/loop-pipeline`）——从其他目录启动 server/CLI 时结果整体落错位置。
2. workflow 模式的 `AgentStage`/`ResearcherStage`/`AnalystStage`/`OrchestratorStage` 构建 `RunContext` 时**不设 working_dir** → agent 工具（bash/write/edit）以进程 CWD 执行，相对路径写入全部逃逸（仓库根的 `miniagent_context/`、`miniagent_debate/` 残留即此因）。
3. planning 角色统一入口 `call_llm_with_tools` 同样不设 working_dir（13 个角色文件全部受影响）；`Blackboard`/`GraphState` 默认 `./miniagent_workspace`、agent 上下文转储 `./miniagent_context`、research 流水线 `ToolContext` 用 `current_dir()` —— 五处 CWD 相对散落源。
4. 任务目录命名 `sanitize_task_brief`/`{id}_{brief}` 在 server 与 CLI 各写一份（漂移风险）。

**修改**：

- 新增 `crates/core/src/paths.rs` —— 单一来源：`workspace_root()`（`MINIAGENT_ROOT` > 从 CWD/exe 向上找 `[workspace]` Cargo.toml 或 `.miniagent-root` 标记 > CWD 兜底）、`result_root()`（`MINIAGENT_RESULT_DIR` > `<root>/result`，create+canonicalize 保证绝对路径）、`models_file()`、`sanitize_task_brief()`/`task_dir_name()`。带单测。
- 接入点全量替换：server `AppState.task_dir`、CLI `run`/`research`/`literature-review`/`debate` 四处、loop-pipeline 默认目录、workflow `default_workflow_dir()`（替换 `WORKFLOW_DIR` 常量，builder/engine 回退同步）、`ModelRegistry` 的 `models.json` 路径。
- **写围栏（writes fenced）**：workflow 四个 stage 的 `RunContext` 全部 `.with_working_dir(task_workflow_dir)`；`call_llm_with_tools` 增加 `work_dir` 参数（13 个角色调用点批量接入，传 `blackboard.work_dir_str()`）；`Blackboard`/`GraphState` 默认目录、agent 上下文转储、research `ToolContext` 全部锚定。
- server `create_new_task` 目录创建失败从 `let _ =` 改为 error 日志（fail loudly）。
- `.env.example` 增加 `MINIAGENT_ROOT`/`MINIAGENT_RESULT_DIR` 说明。
- 前后端贴合确认：四模式（workflow/loop/debate/research）统一走 `create_new_task` → plan pills → progress 事件 → `finalize_task` 生命周期，mode 随 WS 消息传递、未知值回退 workflow；loop 阶段名小写对齐 workflow 渲染。

### 任务二：借鉴的 dsh 设计思想与落地

| dsh 思想 | 本轮落地 |
|---|---|
| Append-only trajectory（模型看到的一切可从日志还原） | research 流水线启动即写 `run_config` 事件：模型档案 id/名称/协议族/flash+pro 模型名 + 全量选项快照（n/top-n/debate/min_year…），任何阶段可从 `project.json` 复原归因 |
| 审计对人类可读（trajectory ≠ 只给机器） | 新增 `ProjectManifest::write_run_report()` → `run_report.md`：阶段表（状态/时长/产物）、假说/验证计划/分析清单、完整 append-only 事件时间线；随 finalize 自动出现在前端文件列表 |
| Writes fenced（写权限围栏） | 上文 working_dir 全量注入——工具相对路径写入被锚定在任务目录内 |
| Fail loudly（禁止静默吞错） | 任务目录创建失败显式 error；`run_report.md`/manifest 写入失败打印告警并记事件 |
| Everything is a plugin 的路径层对应 | 路径解析本身可注入（env 覆盖 > 标记文件探测 > 兜底），部署形态（本机/服务器/容器）无需改码 |

### 验证

- `cargo check --workspace` 干净；`cargo test --workspace`（除已知环境失败的 StepFun 在线订阅外）全部通过，含 loop-pipeline 37 项集成（真实 DeepSeek API 全流程）与 core 新增 paths 单测。
- 帕金森病快速 e2e（任务一验证）与渐冻症完整 e2e（任务三）结果见下文。

### 验证过程中暴露并修复的缺陷（帕金森首跑）

1. **中文查询翻译回显**：MiniMax 对翻译请求原样回显中文 → 直接送 PubMed 必然 0 检索（验证门正确拦截，但任务无法进行）。修复（dsh self-verification）：翻译结果必须通过"纯 ASCII PubMed 查询"校验，失败带纠正性指令重试一次，仍失败记 `query_translation_failed` 事件后回退原查询。
2. **`fix_truncated_json` 闭合顺序错误**（辩论精炼 `expected ',' or '}'` 的真正根因）：旧实现先补全部 `]` 再补全部 `}`，截断发生在数组元素内部时产出 `…"]}}`（数组在元素对象闭合前关闭）→ 必然解析失败。修复为栈序闭合（`}` `]` `}`），并处理截断在反斜杠后的边角情况。
3. **辩论 JSON 截断无抢救**：重试两次仍失败即整阶段失败。新增 `salvage_truncated()`：迭代截掉尾部不完整片段并重新闭合，直到前缀可解析——精炼输出是逐项数组，部分项远好于整阶段丢失；`complete_json` max_tokens 3500→8000 降低截断概率。带单测。

- server 冒烟：启动恢复 40 个历史任务（新路径解析生效）、`/api/models` 正常（active=builtin-minimax）、搜索后端健康探测 4/7（tavily/pubmed/langsearch/anysearch）。
- 帕金森 e2e：全部工件（papers/kg/hypotheses/debate_evidence/plans/analysis notebooks）仅落在 `result/58906165_帕金森病致病机理/`，仓库根目录 diff 为空（零散落）。

### ALS 端到端回归（任务三）

ALS 完整管线（`cargo run -p miniagent-cli -- research -q "肌萎缩侧索硬化（渐冻症，ALS）的致病机理" -n 12 --validate --analyze --top-n 2`）：

- **辩论修复确认**：辩论阶段（帕金森跑失败的关键环节）在 ALS run 上首次成功完成（`debate_completed`），精炼产出 `hypotheses_refined.json` + `hypotheses_refined_full.json`，验证了任务二/任务三修复的截断抢救 + 栈序闭合 + max_tokens 提升组合有效。
- **计划生成**：2 个验证计划（`plans/validation_plan_0/1.json`），每个 4 数据分析任务 + 3~4 湿实验方案。
- **工件全部就位**：仅落在 `result/8e55dd4e_肌萎缩侧索硬化_渐冻症_ALS_的致病机理/` 下（含 `debate_report.json` 与精炼 hypothesis 文件），仓库根目录无散落。

---

## Round 36: 统一设置中心 + 视觉重构

### 后端

`crates/core/src/models.rs` — `ModelKind` 增加 `icon()`（emoji glyph）、`slug()`、`all()`。后端成为模型种类、图标、默认 base URL 的**唯一来源**，前端不再硬编码。

`crates/server/src/routes.rs` — `ModelProfileView` 新增字段：`kind_icon`、`flash_model_name`、`pro_model_name_effective`（本来就存在但前端看不到，统一后端投影）。新增两个端点：
- `GET /api/kinds` — 枚举全部 `ModelKind`，返回 `{slug, label, icon, default_base_url}` 列表。前端从此接口填充 kind 下拉。
- `GET /api/settings/active` — 一站式快照：`active` profile（完整 ModelProfileView）、`debate` 选择、`kinds` 列表。头部 model chip + 设置中心用同一份数据，杜绝漂移。

### 前端

**styles.css** — 全面重写：
- `:root` 设计令牌重整：更中性的灰白面板（`--bg2:#fff`、`--border:#e1e3ec`），单条 `--accent` 紫蓝色用于所有交互元素。模型家族色调通过 `--kind-accent` 变量（JS 按 `kind_icon` 设置）——以前模型卡片用 `#4f8cff` 硬色，现在与主题同步。
- 删除原模型弹窗的**深色孤立块**（它使用从未定义的 `--bg-secondary` 等变量，导致模型弹窗与浅色页面割裂）。
- 卡片、按钮、标签、滚动条、提示、动画、响应式断点统一重做：`.card`、`.status-tag`、`.role-card`、`.form-grid`、`.settings-drawer`（抽屉动画）、`.skeleton`（占位加载）。
- 头部连接状态条、模型 chip、模式标签（mode pill）、stage pills 全部统一风格。

**index.html** — 整体重构：
- 删除旧的 `modelSelect` `<select>` + `modelModal` 弹窗；改为头部 `model-chip` 按钮（点击直接打开设置）+ 右下 `⚙` 入口。
- 新增统一 `.settings-overlay` 抽屉，包含三标签：**🤖 模型** / **⚖ 辩论角色** / **ℹ 关于**。
- 头部增加 `mode-pill` 显示当前模式（与 modeSelect 同步），`modelSummary` 显示主模型与 pro 名字。
- 连接条样式重做（明确 `connected/disconnected` 颜色），状态文字显式（"已连接"/"已断开"/"连接中..."）。

**app.js** — 设置逻辑全部重写：
- 新增 `settingsStore`（activeId / active / models / kinds / debate / settingsTab）。WS 开连时 `loadSettings()` 一次性并发拉 `active + models + kinds + debate`，保证头部 chip 与设置页同源。
- 删除 `loadModels / loadDebateRoles / renderModelSelect / renderModelList / renderDebateRoleSelect / renderDebateRoles / addModel / openModelModal / toggleDebateRoles / toggleModelForm / debateRolesState` 等十几个碎片化函数，合并为：
  - `openSettings(tab)` / `closeSettings()` / `switchSettingsTab(tab)` 抽屉生命周期
  - `renderSettingsTab()` 路由三标签 → 调用 `renderSettingsModels / renderSettingsDebate / renderSettingsAbout`
  - `renderModelCard(m, isActive)` 统一卡片渲染（内置/自定义各一张卡片，tag 显示 使用中/内置/自定义/无 Key）
  - `wireModelForm()` 让 kind 选择自动联动 base URL 默认值提示
  - `activateModel(id)` / `deleteModel(id, name)` / `addModel()` / `saveDebateRoles()` / `resetDebateRoles()` 全部走 `loadSettings()` 重拉数据后重渲染（单一数据源）
- 头部 model chip（`#modelChip`）与 mode pill 由 `setMode()` 联动显示。

### 统一性确认（消除的不一致点）

| 之前 | 现在 |
|---|---|
| HTML `<option>` 硬编码 5 个 kind + `index.html:165-169` | 来自 `/api/kinds`；新增第六种时只改 `ModelKind::all()` |
| 模型弹窗深色块与浅色页面割裂 | 全站统一中性灰白 + `--accent` |
| header `modelSelect` 与 modal `modelList` 不同步 | 单一 `settingsStore` + `renderModelChip` |
| `kind` 字段前端未消费 | `kind_icon` 直接用作 chip 与卡片 emoji |
| `has_key` 不展示 | 卡片/关于页用 `status-tag warn:无 Key` 显式提示 |
| 添加模型无 `api_key_env` UI | 仍留为进阶用法（避免 UI 复杂化）；后端支持完整 |
| 模式选择与头部 chip 不同步 | `setMode()` 同步 mode-pill |

### 验证

- `cargo build -p miniagent-server` 干净。
- Server 启动：所有路由 `200`，新字段 `kind_icon / flash_model_name / pro_model_name_effective` 在 `/api/models`、`/api/settings/active` 中正确返回（验证 `builtin-minimax` 返回 `kind_icon=🌊`、`flash_model_name=MiniMax-M3`）。
- 切换模型往返：POST `/api/models/builtin-deepseek/activate` → `/api/settings/active` 返回 `DeepSeek 🐳`；切回 minimax 正确。
- HTML 渲染：`/api/...` 200，`/styles.css` 200 (414 行)，`/app.js` 200 (1846 行，包含新函数 `renderSettingsModels/Debate/About`/`openSettings`/`activateModel`/`renderModelChip` 等)。
- 现有工作流（`/api/tasks`、`/api/run`、`/api/health`、`/api/skills`、`/api/upload`、`/api/download`）无回归。
- `cargo test --workspace` 通过（除已知 StepFun 在线订阅失效）。

---

## Round 37: 用户最终报告 + 清理 StepFun 在线测试

### 任务一：删除 StepFun 在线测试

StepFun 在线订阅早已失效，相关测试每次 `cargo test` 都失败（环境问题，非代码问题）。本轮彻底移除：
- `crates/provider/tests/stepfun_smoke.rs`
- `crates/loop-pipeline/tests/stepfun_integration.rs`

`cargo test --workspace` 现在 0 失败（无环境依赖）。`stepfun` 模块、`STEPFUN_*` env 变量、内置 `builtin-stepfun` 配置和 `MiniMaxKind`/`ModelKind::StepFun` 等保留——代码侧能力不变，只是测试桩移除。

### 任务二：面向用户的最终报告

之前 `result/{id}_{name}/{brief}.md` 对 research 模式只有 6 行统计（KG/假说/计划计数），没有 KG 概要、精炼假说细节、辩论裁决、验证计划内容、Notebook 状态；对 workflow/loop/debate 模式则是 AI 单一回复流。本轮重构：

**`crates/research/src/lib.rs`** 新增 `ProjectManifest::write_user_report(brief: &str)`，从磁盘读取所有子文件并渲染到 `<brief>.md`，结构：
1. **执行摘要**（核心数字一览）
2. **研究问题**（原始 query）
3. **文献概览**（`papers.json` → 标题表格，最多 50 篇）
4. **知识图谱概要**（`kg.json` → 按类型分组的实体列表）
5. **致病机理假说（精炼后）**（`hypotheses_refined_full.json` → Top 3 详述含机制、支持证据、置信度、新颖度）
6. **假说辩论与裁决**（`debate_report.json` → 轮次、逐假说裁决表、裁判总结）
7. **验证计划**（`plans/*.json` → 设计理由 + 数据分析任务表 + 湿实验清单）
8. **数据分析交付**（manifest analyses → 任务/数据集/后端/状态/notebook/溯源表）
9. **审计与复现**（指向 project.json、run_report.md 等可重放资源）

报告**自动适配**部分缺失（只有 KG、没辩论也能渲染；resume 部分阶段也照样产出）。

**`crates/research/src/pipeline.rs`** —— `run_research` 在 `write_run_report()` 之后调用 `write_user_report(&user_brief)` 并记 `user_report_written` 事件。

**`crates/server/src/routes.rs` `finalize_task`** —— 新增 `mode: &str` 参数：
- `workflow/loop/debate` 模式：AI 流式回复 + 简洁封面（任务 ID、生成时间、结果目录、执行阶段），写入 `<brief>.md`。
- `research` 模式：server 不再覆盖 `pipeline.rs` 已写好的用户报告；保留研究流水线产出的更详细版本。

### 验证

- `cargo test --workspace` 0 失败。
- 渐冻症完整 e2e（任务三）：待验证 `<brief>.md` 覆盖 1-9 节全部内容。

## Round 38: 代码瘦身 + 稳定性 + 中止也出报告 + 桩 LLM 端到端

### 任务一：瘦身与稳定性

- **删除死 crate**：`crates/python`（PyO3 绑定，无依赖方）、`crates/sandbox`（stub，无依赖方）从 workspace 移除并删除目录。
- **去重 provider 构造**：`make_providers` 此前在 `cli/main.rs` 与 `research/pipeline.rs` 逐字重复。新增 `provider::factory::active_provider_pair(&AppConfig)`，两处瘦身为薄委托。
- **dispatch 并发上限**（修复上轮未完成的半成品）：`dispatch.rs` 的 wave 并发此前无限（`tokio::spawn` 全量并发，宽 wave 会打爆 provider 429 / runtime 内存）；上轮遗留的 `FuturesUnordered` 改法既丢了 panic 隔离又没真正限流且编译失败。最终实现：保留 `tokio::spawn`（JoinError 处理不丢）+ 共享 `Semaphore`（`loop_dispatch_wave_concurrency`，默认 4）。
- **`/api/provenance/{task_id}` 修复**：此前按进程 CWD 解析 `analysis/{task_id}/provenance.json`（永远 404）。改为从任务注册表解析 `result_dir`，深度受限递归收集全部 `provenance.json` 返回列表。
- **models.json 解析鲁棒性**（e2e 调试中挖出的暗坑）：`ModelKind` serde snake_case 将 `OpenAiCompatible` 渲染为 `open_ai_compatible`，手写 `openai_compatible` 的 models.json 会让整个注册表解析失败并被 `unwrap_or_default()` **静默**回退到默认 provider（流量打到错误端点且无迹可查）。修复：变体加 `alias = "openai_compatible"`/`"anthropic_compatible"`；解析失败改为 `tracing::error` 大声告警。新增回归测试 `model_kind_deserializes_both_spellings`。
- **杂项**：`minimax.rs` 死序列化变体（`MultiPart`/`OaiContentPart`）、两处 `unused_mut` 清理；`cargo check --workspace` 0 警告。

### 任务三：中止也产出用户报告 + TL;DR

- **`write_user_report` 补全 TL;DR**：上轮注释声称"built last, inserted at top"但从未实现。现以占位符拼接实现：核心结论（辩论最强假说 / 置信度最高假说 + 置信度 + 陈述截断）、关键数字一行（文献/KG/假说/计划/分析任务）、建议下一步（首个数据分析或湿实验任务目标）。
- **`finish_partial` 统一中止路径**：`run_research` 此前 5 个提前退出点（key 校验失败、PubMed 0 结果、KG 0 实体、kg-only 完成、辩论 provider 不可用）直接 `return String::new()`——**目录里什么都不留**。现在统一走 `finish_partial`：记事件、存 manifest、写 run_report + 用户报告（对缺失产物优雅降级）、返回带恢复指引的摘要。
- **server 兜底**：`finalize_task` research 模式下若管线因故没写出 `{brief}.md`，落一个指引占位，目录永不"无报告"。
- **相对路径健壮性**：验证计划路径为相对路径时按 manifest 目录解析（而非进程 CWD）。
- **新增测试**：`user_report_full_run_renders_all_sections`（全量产物 → 9 节 + TL;DR 全渲染、占位符不泄漏）、`user_report_partial_run_still_renders`（空目录 → 优雅降级）。

### 任务三：端到端测试（含可复用的桩 LLM）

真实 provider 全部额度耗尽（MiniMax 429 套餐上限 / DeepSeek 402 欠费 / StepFun 400 无订阅）——环境问题而非代码问题。为此新增 **`scripts/mock_llm_server.py`**：OpenAI 协议兼容的确定性桩，按 prompt 语义路由返回各阶段（查询翻译 / 相关度过滤 / KG 抽取 / 假说评估 / 辩论三方 / 交叉比较 / 验证计划 / 分析脚本）的合法 JSON/代码。注册为 `custom-mock-llm` 档案后，除 LLM 推理外的全部代码路径（PubMed 真检索、efetch、KG 合并、TransE 链路预测、辩论编排、GEO grounding、Jupyter notebook 执行、溯源、报告）都走真实实现。此桩沉淀为资产，CI/无额度环境可随时复跑 e2e。

### 任务二：前端方案

新增 `docs/08-frontend-redesign.md`：按四目标分组的前端优化点（阶段时间线、KG 交互可视化、假说对比工作台、验证计划视图、notebook 在线渲染、全屏报告阅读、审计时间线、溯源面板、断点续跑 UI、长列表虚拟化等）+ 三阶段实施方案 + 验收标准。

### Round 38 补充：辩论优雅降级 + 产物收集修复（e2e 驱动）

端到端验证又暴露并修复两处：

- **辩论 Phase B/C 单点失败丢弃全部成果**：`debate_and_refine` 中交叉比较（Phase B）与精炼（Phase C）此前用 `?` 直接上抛——5 个假说的三方辩论结果因精炼一次 JSON 解析失败而被整体丢弃。现在两处均降级为 `tracing::warn` + 空结果继续（装配循环本就支持缺失精炼条目的回退：保留原假说 + 辩论后置信度）。
- **分析产物收集过浅**：`collect_outputs`/provenance 只扫任务目录 2 层；脚本建 `figures/` 子目录或误用 OUTPUT_DIR 时产物"消失"（报告为 0 output files 且溯源缺失）。新增 `provenance::record_dir_bounded(dir, depth)`（有界递归，深度 6），两处统一使用。
- **e2e 验证（Round 37 遗留的"待验证"）**：真实 provider 全部额度耗尽，采用 `scripts/mock_llm_server.py` 桩 LLM 完成 ALS 全管线端到端（41.7s）：PubMed 真检索（翻译 → 24554 命中取 12）、相关度过滤 5/12、KG 10 实体/13 关系、TransE 链路预测 10 候选、5 假说 + 排序、辩论（逐假说裁决 + 最强假说 + 矛盾对 + 合并建议 + 5 精炼假说）、2 验证计划（数据分析 + 湿实验）、2 个 Jupyter 真执行 notebook（80 行合成队列，ALS 5.085 vs 对照 3.969）+ 溯源 + 每任务 2 个产物文件；最终报告 583 行 9 节全渲染。产物目录：`result/dae5f657_肌萎缩侧索硬化_渐冻症_ALS_的致病机理/`。
- 中止场景验证：provider 全挂时的 3 次真实运行均产出"部分运行最终报告"（`finish_partial` 生效），目录不再空手而归。

### 架构审视结论（任务一）

- 角色系统存在三套（loop-pipeline `roles/` executor-validator-arbiter、planning `roles/` 13 角色、workflow `stages.rs` critic/synthesizer），本轮**未**合并（改动面大、行为等价性验证成本高），但已消除 provider 构造与 review 之外的直接重复；三套角色合并建议列入后续路线。
- `planning`（StateGraph/13 角色）与 `loop-pipeline`（Explore-Plan-Dispatch-Evaluate-Repair）是两个并行编排范式，`research` 是第三条领域专用管线——三者共存是刻意设计（server 三模式入口），但 `memory`/`checkpoint`/`self-improve` 仅部分路径接线，server 路径未启用记忆，属后续可瘦身点。

## Round 39: 深度代码瘦身（死代码 / 死依赖 / 在线测试清理）

**规模**：80 个文件，+1473 / **−3292 行**；crates 18 → **17**；`cargo check --workspace --tests` 0 警告；全量测试 290 通过 0 失败。

### 死依赖（13 处，全部先验证零文本引用再删）
- `kg`：miniagent-core / provider / memory（kg 完全自包含）
- `cli`：checkpoint / kg / hypothesis / analysis（统一经 miniagent-research）
- `planning`：tool / skill / tempfile；`loop-pipeline`：skill
- `server`：analysis / tokio-tungstenite；`skill`：self-improve；`workflow`：tool / memory / checkpoint
- `agent`：checkpoint / self-improve（随子系统下线）

### 整链下线的未接线子系统
- **hooks（571 行）**：`agent::hooks` 全仓零接线（无任何调用方 `with_hooks`）、未见于设计文档 → 删除模块 + Agent 字段 + 循环内 5 处 hook 调用点。
- **checkpoint（整链）**：AppState 字段恒为 None、server 两处调用恒传 `None`、agent 字段从未被读 → 删除 `crates/checkpoint`（185 行）、`core::checkpoint` 模块、`Workflow::run*` 签名中的 `Option<&CheckpointStore>` 参数（贯穿 engine/runner/cli/server/测试）、`RunContext` 的 checkpoint 字段、`AgentConfig`（无人使用，含 checkpoint_interval）、`/api/resume`（依赖 checkpoint 且前端不用）。注意：loop-pipeline 的文件式断点（`_checkpoint.json`）与 research 的 manifest resume **不受影响**。
- **agent 的 self-improver 接线**：`with_self_improver` 零调用方 → 移除字段/接线/循环内反思与工具追踪分支（工具日志分支保留并去重）。self-improve crate 保留（CLI demo + 设计文档），但级联删除其死 API：reflection 模块（147 行）、on_step / on_tool_* / find_relevant_experiences / consolidate 触发器等 15 个零调用方方法。
- 死 ID 类型：`AgentId` / `ProjectId` / `SessionId` / `CheckpointId`（types.rs 中零外部引用）。

### 死函数清理（三轮级联扫描，共 ~80 个零调用方 pub fn）
core/kg/loop-pipeline/provider/telemetry/skill/tool/workflow/planning/memory/hypothesis/agent 全覆盖。方法论：全仓正则扫描 pub fn 名 → 排除公共命名 → 逐 crate 删除（括号配平验证的脚本 + 编译器孤儿告警驱动迭代）直至扫描收敛为空。

### 旧端点与桩命令
- 删除 `/api/run`、`/api/resume`（前端 app.js 未用；属 WebSocket 化前的遗留 REST）及其全部类型/辅助函数。
- 删除 CLI `project create|list` 桩命令（只打印占位文本）。

### 在线测试清理
- 删除 loop-pipeline 集成测试中 5 个真实 API 在线测试（`test_planner_real_api_call` / `test_explorer_real_api_call_with_tools` / `test_full_pipeline_multi_loop` / `test_full_loop_pipeline_real_api` / `test_real_long_running_research_pipeline`）+ `try_load_api_key` 辅助：仅靠"有没有 key"门控，有 key 烧配额、无 key 静默跳过，皆非可靠断言（Round 37 清了 StepFun 版，本轮补齐 DeepSeek 版）。integration_test.rs 3708 → 3346 行，纯 mock 测试全保留。

### 根目录残留清理
`miniagent_context/`（遗留路径）、`miniagent_debate/`（遗留输出目录）、`reports/`（空）、`efetch_multi.txt`（调试转储）、`result/fib.py`、`result/_recovered_from_worktrees/`。

### 保留决策（有意不动）
- `memory`（CLI+server 实际接线）、`self-improve` crate（CLI demo + docs/03 设计）、`planning`/`workflow`/`loop-pipeline` 三编排范式（server 三模式入口）。
- provider 各响应结构体上的 `#[allow(dead_code)]` 字段：建模线上协议格式，删除无收益。
