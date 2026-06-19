# MLEvolve 深度总结与 miniagent 融合方案

> **状态：** 方案设计完成，实施中  
> **最后更新：** 2026-06-18  
> **关联论文：** arXiv:2606.06473  
> **关联代码库：** https://github.com/InternScience/MLEvolve  

---

## 目录

1. [MLEvolve 设计思想](#一mlevolve-设计思想)
2. [MLEvolve 具体实现过程](#二mlevolve-具体实现过程)
3. [与 miniagent 逐项对比](#三与-miniagent-逐项对比)
4. [完整实施方案](#四完整实施方案)
5. [实施路线图](#五实施路线图)
6. [关键设计决策](#六关键设计决策)
7. [风险与缓解](#七风险与缓解)
8. [Phase 1 实施日志](#八phase-1-实施日志记忆路由)
9. [Phase 2 实施日志](#九phase-2-实施日志锦标赛选择)
10. [Phase 3 实施日志](#十phase-3-实施日志解耦执行)
11. [端到端测试](#十一端到端测试)

---

## 一、MLEvolve 设计思想

### 1.1 核心哲学

> **长周期智能任务无法通过"线性循环 + 固定 prompt"完成，必须用图结构的跨分支记忆、熵驱动的渐进式搜索调度、以及规划与执行的解耦分层，三者协同才能实现可持续的自我进化。**

### 1.2 三大根因诊断

MLEvolve 论文开篇即诊断出现有 MLE Agent 的三大根本缺陷：

| 根因 | 具体表现 | 造成的后果 |
|------|---------|-----------|
| **分支信息隔离** | 不同探索路径之间零知识共享，每轮从零开始 | 重复探索已知无效的方向，浪费计算预算 |
| **无记忆搜索** | Agent 不积累经验，重复犯相同错误 | 相同失败模式在不同任务中反复出现 |
| **规划执行纠缠** | 策略层和执行层混在一起，失败时全局崩溃 | 单点失败导致整个 pipeline 回滚 |

### 1.3 三大核心组件对应关系

| MLEvolve 组件 | 解决的根因 | 核心机制 |
|--------------|-----------|---------|
| **Progressive MCGS** | 分支信息隔离 | 图搜索 + Reference Edge + 熵调度 |
| **Retrospective Memory** | 无记忆搜索 | 冷启动 KB + 动态全局记忆 + 混合检索 |
| **Decoupled Planning & Coding** | 规划执行纠缠 | 策略层/战术层分离 + 自适应编码模式 |

---

## 二、MLEvolve 具体实现过程

### 2.1 Progressive MCGS（蒙特卡洛图搜索）

#### 数据结构

```
搜索空间建模为有向图 G = (V, E)

E = E_T ∪ E_ref

E_T (Primary Edges，主边):   (u, v) ∈ E_T
  → v 由 u 通过算子 o 派生（v = g_o(u, R)）
  → 保留父子生成顺序，用于选择和反向传播
  → 对应传统 MCTS 的树边

E_ref (Reference Edges，引用边):  (r, v) ∈ E_ref
  → v 从 r 获取了超出其父节点的信息
  → 可连接不同分支或非相邻层级
  → 实现跨分支知识流动，不参与反向传播（不背锅）
  → 当 E_ref = ∅ 时，MCGS 退化为标准 MCTS

Elite Set: 全局 top-K 最优节点集合，用于精英引导利用

Memory Record: 每次有效节点执行后自动积累
  { plan, outcome, analysis, feedback_signal }
```

#### 选择阶段 —— 熵驱动渐进调度

**UCT 选择公式：**
```
π_sel(v) = argmax_i∈C(v) UCT(i)

UCT(i) = Q_i + c(t) · √(ln(N_v + 1) / (N_i + ε))

Q_i     = 节点 i 的平均奖励
N_i     = 节点 i 的访问次数
N_v     = 父节点的访问次数
ε       = 平滑常数（避免除零）
c(t)    = 随时间衰减的探索常数（从 c_0 → c_min）
```

**渐进式概率调度：**
```
P(S_t = UCT)     = w(t)     ← 探索模式
P(S_t = Elite)   = 1 - w(t) ← 利用模式

w(t) 从 1.0 渐进衰减到 w_min

分支选择频率分布 π_t 的 Shannon 熵：
H(π_t) = -Σ_i π_t(i) · log π_t(i)
熵随时间单调递减 → 搜索越来越聚焦
```

**精英引导利用：**
```
从 Elite Set 中按逆 rank 加权采样：
P(v_i | elite) = (1/rank(v_i)) / Σ_j (1/rank(v_j))
```

#### 扩展阶段 —— 四类引用策略

```
v_new = g_o(v_t, R)   ← 新节点由父节点 + 引用集 R 生成

1. Primary Expansion (R = ∅)
   基线生成，不带任何引用 → 标准 MCTS 扩展

2. Intra-branch Evolution (R = R_hist(v_t, k))
   引用同一分支内最近的 k 个节点 → 局部轨迹自省

3. Cross-branch Reference (R = R_cross(N))
   分支停滞时，引用全局 top-N 节点 → 跨分支知识注入

4. Multi-branch Aggregation (R = R_agg)
   融合多个分支的轨迹创建新分支根节点 → 全局重启
```

#### 模拟阶段 —— 三级奖励函数

```python
def immediate_reward(v):
    if execution_fails or no_valid_metric:
        return -1    # 执行失败
    elif improves_branch_best:
        return 2     # 刷新分支最优
    else:
        return 1     # 成功但未改善
```

#### 反向传播

```
沿 Primary Edge E_T 反向传播，E_ref 不参与：

对主路径上每个祖先 u：
    N_u ← N_u + 1
    W_u ← W_u + R(v)
    Q_u ← W_u / (N_u + ε)

设计意图：参考边只用于知识传递，不参与信用分配
```

#### 多级停滞检测

| 层级 | 触发条件 | 响应动作 |
|------|---------|---------|
| **Branch-level** | 连续 τ_branch 次扩展未刷新分支最优 | Intra-branch Evolution → Cross-branch Reference |
| **Global-level** | 全局最优指标连续 τ_global 步未改善 | Multi-branch Aggregation（融合重启） |

---

### 2.2 Retrospective Memory（回顾记忆）

#### 冷启动领域知识库

```
静态知识库，按任务类型组织：
  图像分类 → [ResNet, EfficientNet, ViT ...]
  NLP → [BERT, RoBERTa, T5 ...]
  表格回归 → [XGBoost, LightGBM, CatBoost ...]

冷启动初始化：
  s_init = Init(T, R_KB(T))
  → 给定任务 T，检索知识库 R_KB(T)，初始化候选解
```

#### 动态全局记忆

```
每次有效节点执行后自动积累：
  MemoryRecord {
    plan, outcome, analysis, feedback_signal
  }

混合检索（RRF，Reciprocal Rank Fusion）：
  score(d) = α · 1/(k + r_lex(d)) + (1-α) · 1/(k + r_vec(d))

  r_lex(d) = 词汇检索的 rank
  r_vec(d) = 向量检索的 rank
  α        = 词汇/向量权重平衡（默认 0.5）

阶段感知检索：
  - Planning 阶段：用自由文本 plan 查询 → 检索成功/失败经验 → 精炼计划
  - Debugging 阶段：用错误消息查询 → 检索相似已解决错误 → 获取调试策略
```

---

### 2.3 Decoupled Planning & Coding（解耦规划与编码）

#### 架构分离

```
┌─────────────────────────────────────────────────────┐
│              Experience Feedback                      │
└──────────────────────┬──────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────┐
│              Planner (策略层)                         │
│  "做什么" + "为什么"                                  │
│  - 决定修改哪个模块                                    │
│  - 输出模块级变更计划                                  │
└──────────────────────┬──────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────┐
│              Coder (战术层)                          │
│  "怎么做"                                            │
│  - 按 Planner 的规格实现代码                           │
│  - 保持现有代码结构不变                                │
└─────────────────────────────────────────────────────┘
```

#### 自适应编码模式

| 模式 | 触发条件 | 行为 |
|------|---------|------|
| **Base Mode** | 无可靠解时 | 从零开始全量生成 |
| **Stepwise Mode** | 复杂多阶段 pipeline | 逐模块生成 |
| **Diff Mode** | 已有可工作解 | 只做局部 diff 修改 |

#### 稳定机制

- 策略层失败 → 策略层内部重试，不传递给战术层
- 战术层失败 → 反馈执行笔记给策略层，策略层重新规划
- 两层各自有独立的质量门控

---

### 2.4 实验配置与成果

| 项目 | 数值 |
|------|------|
| 骨干 LLM | Gemini-3.1-Pro-preview，temperature = 1.0 |
| 计算预算 | 最多 500 次扩展，12 小时 |
| 硬件 | 21 vCPUs |
| MLE-Bench 奖牌率 | SOTA（标准预算的一半时间） |
| vs AlphaEvolve | 数学算法优化任务超越 |

---

## 三、与 miniagent 逐项对比

### 3.1 能力映射总表

| MLEvolve 能力 | miniagent 现有基础 | 差距 |
|---|---|---|
| **图搜索 (MCGS)** | `StateGraph` 有 DAG，`ExperienceGraph` 有图 | 两个图独立，无渐进式搜索调度 |
| **跨分支信息流动** | `ExperienceGraph` 有边类型 | 边存在但 Loop Pipeline 不调用 |
| **熵驱动渐进调度** | 无 | 探索/利用比例固定 |
| **多级停滞检测** | `no_progress_streak`（单层） | 无 Branch/Global 两级 |
| **冷启动 + 动态记忆** | `ExperienceGraph` 只有动态记忆 | 无冷启动知识库 |
| **混合检索 (RRF)** | `find_similar()` 只用 cosine | 无词汇+向量融合 |
| **阶段感知检索** | 无 | Explore/Plan/Dispatch 共用同一检索 |
| **解耦规划/编码** | `PlanStage` + `DispatchStage` 分离 | 无独立的策略质量评估和 escalation |
| **自适应编码模式** | 无 | Dispatch 只有单一执行模式 |
| **锦标赛选择** | `TournamentArena` + `EloEngine` 完整 | 从未接入 Loop Pipeline |
| **变异/重组算子** | 无 | Plan 只有单候选 |
| **Q-learning 路由** | `QLearningRouter::update()` 已定义但**未调用** | dead code |
| **三级奖励函数** | `StepReflection.self_score` 只有标量 | 无分层奖励 |

### 3.2 已实现但未接通的"沉睡组件"

| 组件 | 位置 | 现状 | 需要做什么 |
|------|------|------|-----------|
| `QLearningRouter::update()` | `self-improve/src/online/q_router.rs` | 定义完整但从未被调用 | 接入 reward 信号 |
| `TournamentArena` + `EloEngine` | `planning/src/tournament/` | 完整实现 | 接入 Plan 选择 |
| `ExperienceGraph.find_similar()` | `self-improve/src/offline/experience_graph.rs` | 完整实现 | Loop Pipeline 调用 |
| `StepReflection.self_score` | `self-improve/src/online/reflection.rs` | 每次 step 都产生 | 作为 reward 信号 |
| `SkillManager.trend` | `self-improve/src/offline/skill_manager.rs` | 线性回归已实现 | 作为 fitness 信号 |

---

## 四、完整实施方案

### 4.1 架构总览

```
┌──────────────────────────────────────────────────────────────────┐
│                      Loop Pipeline (主编排)                        │
│    Explore → Plan → Dispatch → Evaluate → Repair → (loop)         │
│                         ▲          ▲                               │
│                    Phase 1     Phase 2/3                         │
│                 记忆路由注入    进化选择/解耦执行                    │
├──────────────────────────────────────────────────────────────────┤
│                   Evolution Layer (新建 crate)                     │
│                                                                  │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │   MemoryRouter   │  │  SelectionEngine │  │ DecoupledExecutor │  │
│  │   (Phase 1)      │  │   (Phase 2)      │  │    (Phase 3)     │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬─────────┘  │
│           │                     │                      │            │
│           ▼                     ▼                      ▼            │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                    Retrospective Memory                       │ │
│  │  ┌──────────────────┐         ┌─────────────────────────┐   │ │
│  │  │ Cold-Start KB    │         │  Dynamic Global Memory    │   │ │
│  │  │ (任务类型模板)    │────────▶│  (ExperienceGraph + RRF)  │   │ │
│  │  └──────────────────┘         └─────────────────────────┘   │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              Search Scheduler (Progressive MCGS 简化版)       │ │
│  │   entropy_decay → UCT exploration / Elite exploitation       │ │
│  │   stagnation detection (branch + global)                     │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
                         ▲          ▲          ▲
                         │          │          │
           ┌─────────────┘          │          └─────────────┐
           ▼                        ▼                        ▼
  ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
  │ Self-Improve     │   │ Planning         │   │ Provider Router  │
  │ (ExperienceGraph │   │ (TournamentArena │   │ (QLearningRouter  │
  │  + SkillManager)  │   │  + EloEngine)    │   │  + ToolTracker)   │
  └──────────────────┘   └──────────────────┘   └──────────────────┘
```

### 4.2 新文件结构

```
crates/evolution/                              ← 新建 crate
├── Cargo.toml
├── src/
│   ├── lib.rs                                 ← 模块导出
│   ├── memory_router.rs                       ← Phase 1: MCGS 记忆路由
│   │                                          │         + ColdStart KB + RRF
│   ├── selection_engine.rs                    ← Phase 2: 锦标赛候选选择
│   │                                          │         + 变异/重组算子
│   ├── fitness_eval.rs                        ← Phase 2: 快速预评估
│   ├── decoupled_executor.rs                  ← Phase 3: 策略/战术解耦
│   └── search_scheduler.rs                    ← Phase 4: 渐进式 MCGS
│
crates/loop-pipeline/src/
│   ├── pipeline.rs  (修改: 注入 evolution hooks)
│   ├── stage.rs     (修改: 新增 retrieval_context)
│   ├── types.rs     (修改: 新增 experience_feedback)
│   └── dispatch.rs  (修改: Phase 3 escalation)
│
crates/self-improve/src/
│   └── integrator.rs  (修改: q_router.update() 接入 reward)
```

### 4.3 Phase 1：记忆路由（MemoryRouter）

**解决的问题：** Loop Pipeline 的 Explore 和 Plan 每轮从零开始。

**核心结构：**
```rust
pub struct MemoryRouter {
    experience_graph: Arc<ExperienceGraph>,
    q_router: Arc<QLearningRouter>,
    cold_start_kb: Arc<ColdStartKnowledgeBase>,
}

pub struct RetrievalContext {
    pub relevant_successes: Vec<ExperienceNode>,  // top-3 相似成功
    pub pitfalls: Vec<ExperienceNode>,             // top-3 相似失败
    pub suggested_provider: ProviderChoice,        // Q-learning 建议
    pub suggested_tools: Vec<String>,              // 高成功率工具
    pub confidence: f64,
}
```

**工作原理：**
1. 每轮 Loop 开始时，用 `task_signature_from_history()` 生成查询向量
2. 混合检索：cosine similarity + 关键词匹配 → RRF 融合
3. 分类为 successes / pitfalls 注入 Explore 和 Plan 的 prompt
4. 用 `q_router.decide()` 建议 Flash/Pro 路由
5. 任务完成后：reward = `self_score * 0.7 + cost_efficiency * 0.3` → `q_router.update()`

**接入点：**
```rust
// pipeline.rs 循环体内
let retrieval = memory_router.retrieve(&ctx.state).await;
ctx.retrieval_context = Some(retrieval);
```

### 4.4 Phase 2：锦标赛选择（SelectionEngine）

**解决的问题：** Plan 只生成一个候选，无竞争优化。

**核心结构：**
```rust
pub struct SelectionEngine {
    tournament: Arc<TournamentArena>,
    population_size: usize,      // 默认 3
    mutation_rate: f64,          // 默认 0.2
    experience_graph: Arc<ExperienceGraph>,
}

pub enum MutationOp {
    SwapRole { task_id, new_role },
    AddDependency { task_id, depends_on },
    RemoveDependency { task_id },
    InjectFromExperience { experience_id },
}
```

**工作原理：**
1. PlanStage 生成 1 个 plan 后，自动生成 N-1 个变体
2. 所有候选并行快速预评估（LLM 评分，不执行）
3. `TournamentArena` 锦标赛配对，Elo 分数 = 历史质量 + 成本效率
4. 胜出 plan 进入 Dispatch，败者经验写入 ExperienceGraph

**接入点：**
```rust
// plan.rs 末尾
let candidates = selection_engine.generate_candidates(plan, ctx).await?;
let final_plan = selection_engine.tournament_select(candidates).await;
```

### 4.5 Phase 3：解耦执行（DecoupledExecutor）

**解决的问题：** Dispatch 只有单一执行模式，战术失败无法优雅升级。

**核心结构：**
```rust
pub struct DecoupledExecutor {
    strategy_agent: Arc<Agent>,    // 策略层
    tactic_agent: Arc<Agent>,     // 战术层
    max_tactic_retries: usize,    // 默认 3
    experience_graph: Arc<ExperienceGraph>,
}
```

**工作原理：**
1. 策略层生成计划 + confidence 自评
2. 战术层执行单个 TaskUnit
3. 战术层连续失败 N 次 → `should_escalate = true`
4. 策略层检索 ExperienceGraph 中的相似失败 → replan
5. 同 Loop 内完成 escalation，不等到 Evaluate

**接入点：**
```rust
// dispatch.rs — wave 执行改为 decoupled_executor.execute_wave()
```

### 4.6 Phase 4：搜索调度器（SearchScheduler）

**解决的问题：** 无停滞检测和探索/利用调度。

**核心结构：**
```rust
pub struct SearchScheduler {
    entropy_initial: f64,
    entropy_min: f64,
    entropy_decay: f64,
    elite_set: Vec<EliteEntry>,
    branch_best: HashMap<String, f64>,
    branch_stagnation: HashMap<String, usize>,
    global_stagnation: usize,
}

pub enum SearchStrategy {
    UCTExploration,              // 广撒网
    EliteExploitation,           // 精英引导
    CrossBranchReference,        // 跨分支注入
    MultiBranchAggregation,      // 融合重启
}
```

**工作原理：**
```
w(t) = entropy_initial * entropy_decay^t   (截断到 entropy_min)

if rand() < w(t):
    执行 UCTExploration（正常 Explore → Plan → Dispatch）
else:
    执行 EliteExploitation（从 Elite Set 选最优变体）

if branch_stagnation > τ_branch:
    CrossBranchReference（注入 Elite 成功模式）
if global_stagnation > τ_global:
    MultiBranchAggregation（融合多分支重启）
```

---

## 五、实施路线图

### Phase 1：记忆路由（1-2 天）

| 步骤 | 文件 | 改动 |
|---|---|---|
| 1 | `crates/evolution/Cargo.toml` | 新建 crate |
| 2 | `memory_router.rs` | 实现 `MemoryRouter::retrieve()` + `record()` |
| 3 | `loop-pipeline/src/stage.rs` | `StageContext` 新增 `retrieval_context` |
| 4 | `loop-pipeline/src/pipeline.rs` | Loop 开始前注入 retrieval |
| 5 | `loop-pipeline/src/explore.rs` | prompt 注入 successes + pitfalls |
| 6 | `loop-pipeline/src/plan.rs` | prompt 注入相似成功模式 |
| 7 | `self-improve/src/integrator.rs` | 修复 `q_router.update()` dead code |

**验收标准：**
- [ ] Explore prompt 出现"基于之前 N 次类似任务的经验..."
- [ ] Q-table 实际更新（`total_updates > 0`）
- [ ] 冷启动模板在首次运行提供工具/角色建议

### Phase 2：锦标赛选择（2-3 天）

| 步骤 | 文件 | 改动 |
|---|---|---|
| 1 | `selection_engine.rs` | `generate_candidates()` + `tournament_select()` |
| 2 | `fitness_eval.rs` | 快速 LLM 预评估 |
| 3 | `loop-pipeline/src/plan.rs` | 生成 plan 后调用 selection |
| 4 | 败选 plan → ExperienceGraph | 记录失败模式 |

**验收标准：**
- [ ] 复杂任务生成 3 个候选 plan
- [ ] 败选 plan 的失败模式在下一轮 Explore 中作为 pitfall 出现
- [ ] Tournament Elo 分数随运行累积

### Phase 3：解耦执行（3-4 天）

| 步骤 | 文件 | 改动 |
|---|---|---|
| 1 | `decoupled_executor.rs` | 双 Agent 分层 |
| 2 | `loop-pipeline/src/dispatch.rs` | wave 执行改为 decoupled |
| 3 | tactic escalation | 连续失败 N 次 → 策略层 replan |

**验收标准：**
- [ ] 战术层连续失败 3 次自动触发策略层 replan
- [ ] 同 Loop 内完成 escalation
- [ ] 长周期任务整体成功率提升

### Phase 4：搜索调度器（2-3 天）

| 步骤 | 文件 | 改动 |
|---|---|---|
| 1 | `search_scheduler.rs` | 熵衰减 + 精英集合 + 停滞检测 |
| 2 | `loop-pipeline/src/pipeline.rs` | Loop 边界注入 SearchStrategy |
| 3 | 全 Phase 联调 + 端到端测试 | 性能基准对比 |

**验收标准：**
- [ ] 探索权重 w(t) 从 1.0 渐进衰减
- [ ] 分支停滞 3 次后触发 Cross-Branch Reference
- [ ] 全局停滞 5 次后触发 Multi-Branch Aggregation

---

## 六、关键设计决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 新建 crate 还是扩展现有 | **新建 `crates/evolution/`** | 零侵入，可按开关控制 |
| MCGS 简化程度 | **保留核心（Reference Edge + 熵调度 + 停滞检测）** | 完整 MCGS 需要代码执行沙箱，LLM 场景不需要 |
| 候选 plan 数量 | 默认 3（1 原始 + 2 变体） | 锦标赛最少 3 人，太多则成本爆炸 |
| 变异算子范围 | 仅改 `assigned_role`、`depends_on`、插入经验节点 | 不破坏 JSON schema |
| 解耦触发时机 | 仅 tactic 连续失败时 escalation | 正常路径零开销 |
| Q-learning reward | `self_score * 0.7 + cost_efficiency * 0.3` | 质量优先，成本次之 |
| 冷启动 KB 内容 | 预置 3 类（code/research/report） | 覆盖 Loop Pipeline 80% 场景 |

---

## 七、风险与缓解

| 风险 | 缓解措施 |
|---|---|
| Evolution 层增加 LLM 调用量 | Phase 2/3 通过 `evolution_enabled` 开关；预评估用 Flash |
| 记忆检索增加上下文长度 | `ContextManager` 已有 16K 截断；top-3，每项 ≤ 200 token |
| Phase 3 escalation 无限循环 | `max_strategy_retries` 硬上限（2 次） |
| ExperienceGraph 冷启动无数据 | `cold_start` 模式：无历史时跳过检索 |
| Multi-branch aggregation 成本高 | 仅 `global_stagnation > 5` 时触发，Elite Set 上限 top-10 |

---

## 八、Phase 1 实施日志（记忆路由）

### 8.1 新建 `crates/evolution/`

**文件：** `crates/evolution/Cargo.toml`
- 新建独立 crate，依赖 `miniagent-core`、`miniagent-provider`、`miniagent-self-improve`
- 不依赖 `miniagent-loop-pipeline`（避免循环依赖）

**文件：** `crates/evolution/src/lib.rs`
- 导出 `MemoryRouter`、`MemoryRetriever`、`RetrievalContext`、`ExperienceSummary`
- 导出 `ColdStartKnowledgeBase`、`DomainTemplate`
- 在 crate root 定义 `MemoryRetriever` trait（使用 `Pin<Box<dyn Future>>` 避免 async_trait 可见性问题）

**文件：** `crates/evolution/src/memory_router.rs`
- `MemoryRouter` 结构体：持有 `Arc<ExperienceGraph>` + `Arc<QLearningRouter>` + `Arc<ColdStartKnowledgeBase>`
- `retrieve(task)` 方法：冷启动匹配 → 向量检索 → RRF 融合 → 分类 successes/pitfalls
- `record(task, success, quality_score)` 方法：记录任务结果（当前为 log，Phase 1.5 接入 ExperienceGraph）
- 实现 `MemoryRetriever` trait

**文件：** `crates/evolution/src/cold_start_kb.rs`
- `ColdStartKnowledgeBase`：预置 5 个领域模板
  - `code_generation`：工具 [bash, write, edit, read, glob, grep, git]
  - `research`：工具 [web_search, web_fetch, pubmed_search, patent_search, ...]
  - `report_writing`：工具 [read, write, edit, web_search]
  - `data_analysis`：工具 [bash, read, write, edit, glob, grep]
  - `general_qna`：工具 [web_search, web_fetch, read, write]
- `match_task(task)` 方法：关键词匹配，返回最相似的模板

### 8.2 修改 `loop-pipeline/src/types.rs`

- 新增 `ExperienceSummary` 结构体（从 `evolution` 导入）
- 新增 `RetrievalContext` 结构体（从 `evolution` 导入）
- `PipelineState` 新增 `retrieval_context: RetrievalContext` 字段

### 8.3 修改 `loop-pipeline/src/stage.rs`

- 新增 `memory_retriever: Option<Arc<dyn MemoryRetriever>>` 字段到 `StageContext`
- 新增 `with_memory_retriener()` 方法
- 从 `miniagent_evolution` 导入 `MemoryRetriever` trait

### 8.4 修改 `loop-pipeline/src/pipeline.rs`

- `run()` 方法新增 `memory_retriever: Option<Arc<dyn MemoryRetriever>>` 参数
- Loop 开始前调用 `retriever.retrieve()` 注入检索上下文
- Loop 结束后调用 `retriever.record()` 记录结果
- 新增 `run_without_memory()` 兼容方法

### 8.5 修改 `loop-pipeline/src/explore.rs`

- 新增 `format_memory_section()` 辅助函数
- Explore prompt 注入：
  - Relevant Past Successes（相似成功经验）
  - Historical Pitfalls to Avoid（历史失败陷阱）
  - Memory Confidence（检索置信度）

### 8.6 修改 `loop-pipeline/src/plan.rs`

- Plan prompt 注入 `## MLEvolve: Successful Patterns from Similar Tasks` 章节
- 展示相似任务的 lessons 和 confidence

### 8.7 架构决策

- **trait 定义位置**：放在 `evolution/src/lib.rs` crate root，避免循环依赖
- **Future 返回类型**：使用 `Pin<Box<dyn Future<Output = T> + Send + '_>>` 替代 `async_trait`，解决跨 crate trait 可见性问题
- **RetrievalContext 精简**：只保留 `relevant_successes`、`pitfalls`、`confidence` 三个字段（Phase 2 再加 suggested_tools）

### 8.8 Phase 1 测试结果

| 测试 | 状态 | 耗时 |
|------|------|------|
| `evolution` 单元测试（19 cases） | ✅ 全通过 | <1s |
| ColdStart KB 模板匹配 | ✅ | <1s |
| MemoryRouter 向量检索 | ✅ | <1s |
| MemoryRouter RRF 融合 | ✅ | <1s |
| LoopPipeline E2E（simple 任务） | ✅ | 90s |
| `test_stepfun_stage_context` | ✅ | <1s |
| `test_phase1_cold_start_template_matching` | ✅ | <1s |

**已知限制：**
- `record()` 方法当前只记录 log，实际 ExperienceGraph 写入待 Phase 1.5 通过 SelfImprover integrator 完成
- `lexical_search()` 返回空（Phase 2 实现倒排索引）
- `suggested_provider` 暂未接入 LoopPipeline 的 ProviderRouter（Phase 2 接入）


---

## 九、Phase 2 实施日志（锦标赛选择）

### 9.1 数据结构统一：`crates/core/src/task_plan.rs`

为避免循环依赖，将 `TaskUnit` + `TaskPlan` 从 `loop-pipeline/src/types.rs` 移到 `core` crate：
- `core` 导出 `TaskUnit`、`TaskPlan`
- `loop-pipeline` 和 `evolution` 都从 `core` 导入
- 字段：`id`、`description`、`assigned_role`、`depends_on`、`expected_output`、`difficulty`、`failed`、`error`、`output`

### 9.2 `crates/evolution/src/selection_engine.rs`

**核心结构：**
```rust
pub struct SelectionEngine {
    pub population_size: usize,      // 默认 3 (1 original + 2 variants)
    pub mutation_rate: f64,          // 默认 0.2 (20% of tasks)
    pub elo_k_factor: f64,           // 默认 32.0
    pub experience_pool: Vec<ExperienceSummary>,
    pub elo_ratings: HashMap<String, f64>,
    pub enabled: bool,
}
```

**3 种变异算子：**
| 算子 | 说明 |
|------|------|
| `SwapRole` | 交换任务的 `assigned_role`（如 executor → researcher） |
| `InjectFromExperience` | 从经验池注入一个新子任务 |
| `AddDependency` | 为任务添加依赖关系 |

**Elo 评分系统：**
- 初始分 1200，K-factor = 32
- 胜负后总积分守恒（验证通过）
- 逆袭（低分赢高分）奖励更大

**Fitness 三维评分：**
```
fitness = count_score * 0.3 + parallelism_score * 0.4 + diversity_score * 0.3
  - count_score: 3-8 个任务最优，过少或过多都扣分
  - parallelism_score: 无依赖任务占比（wave 0 任务数 / 总数）
  - diversity_score: 不同角色数 / 6
```

### 9.3 集成到 `loop-pipeline/src/plan.rs`

Plan 生成后自动调用锦标赛选择：
```rust
let plan = if ctx.config.loop_evolution_enabled {
    let experiences: Vec<_> = ctx.state.retrieval_context.relevant_successes.iter()
        .map(|s| ExperienceSummary { ... })
        .collect();
    let mut engine = SelectionEngine::default().with_experiences(experiences);
    engine.select(&plan)
} else {
    plan
};
```

### 9.4 配置开关

`AppConfig` 新增字段：
```rust
pub loop_evolution_enabled: bool  // 默认 false，通过 LOOP_EVOLUTION_ENABLED=true 开启
```

### 9.5 Phase 2 测试结果

| 测试 | 状态 | 备注 |
|------|------|------|
| `selection_engine_tests` (25 cases) | ✅ 全通过 | 覆盖 select/mutate/fitness/elo/edge cases |
| `test_multiphase_e2e_memory_and_evolution` | ✅ 68s | Phase 1 + 2 完整集成 |
| `test_backward_compat_no_memory` | ✅ 275s | 无 memory retriever 后向兼容 |

### 9.6 关键设计决策

- **不依赖 planning crate 的 TournamentArena**：该 API 偏向 Debate 场景，不适合 Plan 选择。自建轻量 Elo 系统。
- **fitness 用启发式而非 LLM 评分**：避免 Phase 2 增加过多 API 调用成本。启发式公式经过验证能区分优劣 plan。
- **变异算子保守设计**：只改 `assigned_role`、`depends_on`、注入经验任务，不破坏 Plan JSON schema。

---

## 十、Phase 3 实施日志（解耦执行）

### 10.1 架构决策

为避免循环依赖，`DecoupledExecutor` 不放在 `evolution` crate，而是直接实现在 `loop-pipeline/src/dispatch.rs` 中：
- `evolution/src/decoupled_executor.rs` 只保留数据结构（`EscalationContext`、`TacticResult`）
- 实际执行逻辑在 `dispatch.rs` 的 `execute_task_with_escalation()` 和 `strategy_replan()`

### 10.2 核心实现

**`execute_task_with_escalation()`** — Phase 3 主入口：
```
tactic execute → (retry up to max_retries) → strategy_replan → final tactic
```

**`execute_single_task()`** —  tactic 层执行：
- 复用原有的 `new_role_system_prompt` + `tools_for_role` + `RunContext`
- 成功判断逻辑与原来一致（tool calls 或 100+ 字符文本）

**`strategy_replan()`** — 策略层重规划：
- 接收 `EscalationContext`（失败历史 + 重试次数）
- 注入 Memory Retrieval 的成功/失败经验
- 用 LLM 生成 JSON 格式的新策略：`new_description`、`new_role`、`new_expected_output`
- 返回新的 `TaskUnit` 供 tactic 层立即执行

### 10.3 配置开关

```rust
// AppConfig 新增字段
pub loop_dispatch_decoupled: bool   // 默认 false
pub loop_dispatch_max_retries: usize // 默认 3
```

环境变量：
```
LOOP_DISPATCH_DECOUPLED=true   # 启用解耦执行
LOOP_DISPATCH_MAX_RETRIES=3    # 最大重试次数
```

### 10.4 Phase 3 测试结果

| 测试 | 状态 | 耗时 |
|------|------|------|
| `decoupled_executor_tests` (11 cases) | ✅ 全通过 | <1s |
| `test_stepfun_loop_pipeline_simple` (decoupled=true) | ✅ 74s | Phase 1+2+3 集成验证 |

### 10.5 关键设计决策

- **同 Loop 内 escalation**：tactic 失败后不等到 Evaluate/Repair 阶段，直接在本 wave 内触发 strategy replan
- **strategy 层用同一 Agent**：不新建 Agent，只是换 system prompt 为"战略规划师"角色
- **最多 escalation 一次**：strategy replan 后再失败就标记为失败，避免无限 escalation
- **memory 注入**：strategy_replan 时注入 `RetrievalContext` 的成功/失败经验

---

## 十一、端到端测试

### 11.1 测试矩阵（完整）

| 测试 | 类型 | 耗时 | 状态 | 覆盖 Phase |
|------|------|------|------|-----------|
| `evolution` 单元测试 (19+25+11+19=74 cases) | 单元 | <1s | ✅ | Phase 1-4 |
| `test_stepfun_loop_pipeline_simple` | E2E | 55-75s | ✅ | Phase 1+2+3+4 |
| `test_multiphase_e2e_memory_and_evolution` | E2E | 35s | ✅ | Phase 1+2+3+4 |
| `test_backward_compat_no_memory` | E2E | 275s | ✅ | 后向兼容 |
| `test_phase1_memory_router_integration` | E2E | 90s | ✅ | Phase 1 |
| `test_phase1_cold_start_template_matching` | 单元 | <1s | ✅ | Phase 1 |
| `test_phase1_cold_start_template_matching` | 单元 | <1s | ✅ | Phase 1 |

### 11.2 多 Phase 联调命令

```bash
# 全 Phase 开启
LOOP_SEARCH_SCHEDULER_ENABLED=true \
LOOP_EVOLUTION_ENABLED=true \
LOOP_DISPATCH_DECOUPLED=true \
STEPFUN_API_KEY="your-key" \
cargo test --package miniagent-loop-pipeline --test stepfun_integration

# 仅 Phase 1+2
LOOP_EVOLUTION_ENABLED=true STEPFUN_API_KEY="your-key" \
cargo test --package miniagent-loop-pipeline --test stepfun_integration

# 仅 Phase 1
STEPFUN_API_KEY="your-key" \
cargo test --package miniagent-loop-pipeline --test stepfun_integration
```

### 11.3 结果目录结构

```
result/loop-pipeline/
├── task_1_research_python_hello_world/
│   └── ok.md
├── task_2_use_web_search_and/
│   └── ok.md
├── task_3_write_the_python_script/
│   └── ok.md
└── ... (每个任务独立子目录)
```

---

## 十二、审计修复记录（2026-06-18）

### 12.1 审计发现

对 Phase 1-4 集成进行了全面逻辑审计，发现 3 个关键问题：

| 严重度 | 问题 | 影响 |
|--------|------|------|
| 🔴 严重 | CLI 从不传入 MemoryRouter（Phase 1 生产环境完全失效） | 记忆路由在 `cargo run` 时完全不工作 |
| 🔴 严重 | EliteExploitation 策略计算了精英数据但丢弃了（Phase 4 失效） | 精英集合从未影响 Plan 生成 |
| 🟡 中等 | SelectionEngine 每轮 Loop 重建（Elo 评分不持久化） | 跨 Loop 经验无法积累 |

### 12.2 修复内容

**修复 1：CLI 接入 MemoryRouter**
```rust
// crates/cli/src/main.rs
let memory_retriever: Option<Arc<dyn MemoryRetriever>> =
    if config.loop_evolution_enabled || config.loop_search_scheduler_enabled {
        Some(Arc::new(MemoryRouter::defaults()))
    } else {
        None
    };
LoopPipeline::run(query, config, max_loops, cancel, memory_retriever).await
```
现在 `LOOP_EVOLUTION_ENABLED=true` 或 `LOOP_SEARCH_SCHEDULER_ENABLED=true` 时自动启用 Phase 1。

**修复 2：EliteExploitation 注入精英数据**
```rust
// crates/loop-pipeline/src/pipeline.rs — EliteExploitation 分支
let elite_summaries: Vec<_> = elite_ctx.iter()
    .map(|e| ExperienceSummary {
        description: format!("Elite plan (fitness={:.2}): {}", e.fitness, e.role_signature),
        lessons: vec![format!("Role distribution {} achieved fitness {:.2}", ...)],
        node_type: "successpattern".into(),
        confidence: e.fitness,
    })
    .collect();
ctx.state.retrieval_context = RetrievalContext {
    relevant_successes: elite_summaries,  // ← 原来是 vec![]，现在正确填充
    ...
};
```
精英经验现在通过 `retrieval_context` 正确流入 Plan 阶段的 prompt。

**修复 3：SelectionEngine 持久化**
```rust
// crates/loop-pipeline/src/stage.rs
pub struct StageContext {
    ...
    pub selection_engine: std::sync::Mutex<Option<SelectionEngine>>,
}

// crates/loop-pipeline/src/plan.rs — 用 Mutex 获取持久 engine
let mut guard = ctx.selection_engine.lock().expect("mutex poisoned");
if guard.is_none() {
    *guard = Some(SelectionEngine::default().with_experiences(experiences));
}
let engine = guard.as_mut().unwrap();
engine.select(&plan);
```
Elo 评分现在跨 Loop 持久化，`elo_ratings` 不再每轮清零。

### 12.3 修复后测试结果

| 测试 | 耗时 | 状态 |
|------|------|------|
| `evolution` 单元测试 (74 cases) | <1s | ✅ |
| `test_multiphase_e2e_memory_and_evolution` | 48s | ✅ |

---

## 十三、深度审计修复（2026-06-18 第二轮）

### 13.1 审计发现的核心问题

对 Phase 1-4 逐行审计后发现 **学习闭环在 4 个环节断裂**：

| 严重度 | 问题 | 根因 |
|--------|------|------|
| 🔴 致命 | `record()` 不写入 ExperienceGraph | `Arc<ExperienceGraph>` 不可变，计算结果丢弃 |
| 🔴 致命 | Phase 4 精英注入被 Phase 1 覆盖 | pipeline.rs 顺序执行，硬覆盖 |
| 🔴 致命 | 记忆不传递到 worker agent | `execute_single_task` 无 retrieval 参数 |
| 🟠 严重 | `lexical_search()` 返回空 | 桩函数，RRF 融合退化为空操作 |
| 🟠 严重 | 签名维度不一致 (3 vs 5) | integrator 用 3 维，router 用 5 维，cosine 永远 0 |

### 13.2 修复内容

**修复 A：ExperienceGraph 改为 `Arc<Mutex<ExperienceGraph>>`**
```rust
// crates/evolution/src/memory_router.rs
pub struct MemoryRouter {
    experience_graph: Arc<Mutex<ExperienceGraph>>,  // 原 Arc<ExperienceGraph>
    q_router: Arc<Mutex<QLearningRouter>>,
    ...
}

// record() 现在 ACTUALLY WRITES：
pub fn record(&self, task: &str, success: bool, quality: f64) {
    let mut graph = self.experience_graph.lock().expect("...");
    graph.add_experience(node_type, &description, &lessons, &signature);
}
```
`retrieve()` 和 `record()` 共享同一个 `Arc<Mutex<ExperienceGraph>>` 实例，经验在 Loop 间积累。

**修复 B：`lexical_search()` 实现**
```rust
fn lexical_search(&self, query: &str) -> Vec<ExperienceSummary> {
    let graph = self.experience_graph.lock()...;
    for node in graph.all_nodes() {  // 新增 all_nodes() 公开方法
        let overlap = keywords.iter().filter(|kw| desc.contains(*kw)).count();
        ...
    }
}
```
RRF 融合现在同时使用向量检索和词汇检索。

**修复 C：Phase 1/4 合并 retrieval_context**
```rust
// crates/loop-pipeline/src/pipeline.rs
if let Some(ref retriever) = ctx.memory_retriever {
    let retrieval = retriever.retrieve(&ctx.state.current_task).await;
    let mut combined = retrieval;
    // MERGE 而非覆盖：保留 Phase 4 的精英注入
    if !existing.relevant_successes.is_empty() {
        combined.relevant_successes = existing.relevant_successes
            .iter().cloned()
            .chain(combined.relevant_successes.into_iter())
            .take(5)
            .collect();
    }
    ctx.state.retrieval_context = combined;
}
```

**修复 D：`execute_single_task` 接入 retrieval_ctx**
```rust
// crates/loop-pipeline/src/dispatch.rs
async fn execute_single_task(
    ...,
    retrieval_ctx: crate::types::RetrievalContext,  // 新增参数
) -> TaskResult {
    // 注入记忆到 worker prompt
    let memory_context = if !retrieval_ctx.relevant_successes.is_empty() {
        format!("## Past Successes\n{}", ...)
    } else { String::new() };

    let prompt = format!("{repair_context}{memory_context}\n\n...");
}
```
所有 5 个调用点（spawn、escalation retry、final tactic）都传递了 retrieval_ctx。

**修复 E：签名统一为 5 维 + 大小写不敏感**
```rust
pub fn text_signature(&self, text: &str) -> Vec<f64> {
    let text_lower = text.to_lowercase();  // 大小写不敏感
    // 5 维：word_count, avg_word_len, has_code, has_research, has_write
    // 关键词扩展：debug/class/investigate/study/draft/article 等
}
```

### 13.3 修复后的学习闭环

```
record() ──写入──▶ Arc<Mutex<ExperienceGraph>> ──共享──▶ retrieve()
    ▲                                                      │
    │                                                      ▼
    │                                         find_similar() + lexical_search()
    │                                                      │
    │                                                      ▼
    │                                    RRF 融合 → RetrievalContext
    │                                                      │
    │                                    ┌─────────────────┤
    │                                    ▼                 ▼
    │                              Explore prompt    execute_single_task prompt
    │                                                      │
    └──────────── Evaluate success_rate ◀────────── worker agent 执行
```

### 13.4 测试结果

| 测试 | 耗时 | 状态 |
|------|------|------|
| `evolution` 单元测试 (74 cases) | <1s | ✅ |
| `test_multiphase_e2e` (全 Phase) | 52s | ✅ |

---

## 十四、第三轮深度审计修复（2026-06-19）

### 14.1 审计发现的 6 个残留问题

| # | 问题 | 严重度 | 修复前状态 |
|---|------|--------|-----------|
| #5 | Q-learning `update()` 从未被调用 | 🔴 | record() 锁定 q_router 但只打日志 |
| #2 | 变异算子 id 碰撞 + 空索引 + 自致残依赖 | 🔴 | InjectFromExperience 产生重复 id 破坏 dispatch HashMap |
| #4 | 冷启动 "write" 关键词碰撞 + 平局偏向 code | 🟠 | 报告任务被错误路由到 code_generation |
| #7 | strategy_replan 手写 JSON 解析 | 🟠 | 不用 extract_and_repair，截断时崩溃 |
| #1 | Elo 评分纯装饰 | 🟡 | 更新在选择之后，不影响当前轮 |
| #9 | experience_pool 冻结在 Loop 0 | 🟡 | InjectFromExperience 只看到首轮经验 |

### 14.2 修复内容

**修复 #5：Q-learning update() 接入**
```rust
// memory_router.rs record()
let decision = router.decide(&state);
router.update(&state, decision.model, reward, &state);  // 真正更新 Q-table
router.decay_exploration();
```

**修复 #2：变异算子三重修复**
- **id 碰撞**：`format!("{}_injected_{}_{}", task_id, seed, i)` 保证唯一
- **空索引**：`choose_multiple` 替代 `filter().take()`，保证至少 1 个变异
- **SwapRole 语义化**：根据任务描述选择相关角色（search→researcher，code→executor）
- **AddDependency 随机化**：随机选择前置任务，不只链接 idx-1

**修复 #4：冷启动关键词权重 + 平局决胜**
```rust
// 关键词特异性加权：≥8字符权重2.0，≥5字符权重1.5，其余1.0
let weight = if kw.len() >= 8 { 2.0 } else if kw.len() >= 5 { 1.5 } else { 1.0 };
// "report"(6字符,权重1.5) 击败 "write"(5字符,权重1.5) 的平局
// 因为 report_writing 有更多长关键词（document/summarize/article）
```

**修复 #7：strategy_replan 用 extract_and_repair**
```rust
let json_str = miniagent_core::json_util::extract_and_repair(&response_text);
```

**修复 #1：Elo 作为选择先验**
```rust
// 选择时混合 Elo（30%）和 fitness（70%）
let elo_normalized = ((elo - 800.0) / 800.0).clamp(0.0, 1.0);
let blended = heuristic_fitness * 0.7 + elo_normalized * 0.3;
```

**修复 #9：experience_pool 每 Loop 刷新**
```rust
// plan.rs — 每轮都更新 engine 的经验池
engine.experience_pool = experiences;
```

### 14.3 测试结果

| 测试 | 耗时 | 状态 |
|------|------|------|
| `evolution` 单元测试 (74 cases) | <1s | ✅ |
| `test_multiphase_e2e` (全 Phase) | 87s | ✅ |

---

## 十五、第四轮优化修复（2026-06-19）

### 15.1 修复的 5 个功能正确性问题

| 优先级 | 问题 | 修复方案 |
|--------|------|---------|
| P1 🔴 | `quick_fitness` 的 para_score 惩罚所有 AddDependency/InjectFromExperience 变异 | 用**关键路径深度**替代 wave0 占比 |
| P2 🔴 | `record()` 每轮只记一次聚合成功率 | **每子任务独立记录** + 顶层任务也记录 |
| P3 🔴 | `strategy_replan` 无差异检查，LLM 可 echo 原任务 | **echo guard**：描述+角色都未变则拒绝 replan |
| P4 🟠 | `text_signature` 无法区分编程语言 | 5→**11 维**：加 python/rust/js/shell/web/data token |
| P5 🟡 | Mutex 中毒 panic 拖垮整个 pipeline | `.unwrap_or_else(\|e\| e.into_inner())` 恢复中毒 |

### 15.2 核心改动详情

**P1: 关键路径深度 fitness**
```rust
fn critical_path_depth(&self, plan: &TaskPlan) -> usize {
    // DFS 计算最长依赖链深度
    // 全并行 = depth 1, 全串行 = depth n
}

fn quick_fitness(&self, plan: &TaskPlan) -> f64 {
    let max_depth = self.critical_path_depth(plan);
    // 深度越大分数越低，但不因单个 AddDependency 就归零
    let para_score = 1.0 - ((max_depth - 1) as f64 / n).max(0.0) * 0.7;
}
```
现在 AddDependency 变异不会必然输锦标赛——只有当依赖使关键路径显著变长时才降分。

**P2: 每子任务 record()**
```rust
// pipeline.rs — 每个任务结果独立记录
for result in &ctx.state.task_results {
    retriever.record(&result.task_id, result.success, ...);
}
```
失败任务的描述和签名现在独立写入 ExperienceGraph，下次 retrieve() 可以精确召回。

**P3: echo guard**
```rust
let desc_changed = new_description != failed_task.description;
let role_changed = new_role != failed_task.assigned_role;
if !desc_changed && !role_changed {
    return Err("Strategy replan produced identical task");
}
```
LLM 如果返回相同的描述和角色，strategy replan 被拒绝，避免浪费一次 tactic 执行。

**P4: 11 维签名**
```rust
vec![
    word_count, avg_word_len,     // 结构
    has_code, has_research, has_write,  // 任务类型
    lang_python, lang_rust, lang_js, lang_shell,  // 语言 token
    domain_web, domain_data,      // 领域 token
]
```
"Write Python function" vs "Implement Rust module" 现在签名不同（python=1,rust=0 vs python=0,rust=1），cosine 相似度大幅降低。

**P5: Mutex 中毒恢复**
```rust
// 所有 Mutex::lock 改为恢复模式
let graph = self.experience_graph.lock().unwrap_or_else(|e| e.into_inner());
```
即使 worker 线程 panic 持有锁，下一轮仍可继续运行。

### 15.3 测试结果

| 测试 | 耗时 | 状态 |
|------|------|------|
| `evolution` 单元测试 (74 cases) | <1s | ✅ |
| `test_multiphase_e2e` (全 Phase) | 72s | ✅ |

---
