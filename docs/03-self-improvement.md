# Agent 自改进系统设计

## 业界现状

### 已有方案的三大问题

| 方案 | 问题 | 证据 |
|------|------|------|
| Naive Self-Critique | 简单任务上正确率从 98% → 57% | Snorkel AI, "The Self-Critique Paradox" |
| LLM 自动生成 Skill | 自编技能 +0.0pp vs 人工策划 +16.2pp | Ratchet (ArXiv 2605.22148) |
| End-of-rollout Reflection | 错误已传播多步，reflection 说 "try something different" 但无具体指导 | Aarhus Univ 分析: 70% 错误源自 Verification 阶段 |

### 关键参考文献

| 论文 | 核心贡献 | 年份 |
|------|---------|------|
| Agent-R | MCTS 构建纠正轨迹，首个错误步骤即修正 | Jan 2025 |
| GUI-Reflection | 端到端 "犯错→反思→修正" 闭环 | Jun 2025 |
| Ratchet | 技能生命周期管理 > 技能生成；Outcome-driven retirement + Meta-Skill | May 2026 |
| EXG | 经验图 (Experience Graph) — 结构化成功/失败关系 | May 2026 |
| AEL | 两时间尺度: Thompson Sampling 选策略 + LLM 反思注入因果 | Apr 2026 |
| MetaClaw | 零停机持续元学习: 失败→技能合成→LoRA微调→RL-PRM | Mar 2026 |
| Evolving-RL | 联合优化经验提取 + 经验利用，98.7% 相对提升 | May 2026 |

## 双层自改进架构

```
┌──────────────────────────────────────────────────────────────┐
│                     Self-Improvement 系统                     │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  Layer 1: 在线快速改进 (In-Session, Flash 模型)                │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │ Step-Level Reflection → 每步执行后即时诊断               │ │
│  │ Q-Learning Router    → 实时学习最优策略选择               │ │
│  │ Tool Reliability     → 跟踪工具成功率/延迟/误用           │ │
│  │ Lifecycle Guard      → 防止性能退化 (Ratchet 不退化定理)  │ │
│  └─────────────────────────────────────────────────────────┘ │
│                         ↓ 经验写入                            │
│  Layer 2: 离线深度改进 (Sleeptime, Pro 模型)                   │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │ Experience Graph     → 结构化成功/失败经验图              │ │
│  │ Skill Lifecycle      → 创建/评估/退役 Skill               │ │
│  │ Meta-Skill Update    → 改进如何创建 Skill 的指导           │ │
│  │ Sleeptime Consolidate→ Decay→Dedup→Triage→Cluster→Rebuild│ │
│  └─────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

## Layer 1: 在线快速改进

### 1.1 Step-Level Reflection (借鉴 Agent-R + GUI-Reflection)

```rust
struct StepReflection {
    step_id: StepId,
    action: Action,
    outcome: Outcome,
    self_score: f32,           // 0.0-1.0
    error_detected: bool,
    error_root_cause: Option<String>,
    should_retry: bool,
    retry_with_changes: Option<String>,
}

impl Agent {
    async fn reflect_on_step(&self, step: &Step) -> StepReflection {
        // Flash 快速反思 → 低成本
        let reflection = self.flash.complete(&REFLECTION_PROMPT, ...).await?;

        // 如果 self_score < 0.5 且任务复杂 → Pro 深度审查
        if reflection.self_score < 0.5 && step.complexity > THRESHOLD {
            reflection = self.pro
                .with_thinking(4000)
                .complete(&DEEP_REFLECTION_PROMPT, ...).await?;
        }
        reflection
    }
}
```

**关键设计**:
- 每步执行后立即反思，不等整个任务结束
- Flash 做快速检查，Pro 做深度审查（节省成本）
- 错误定位具体到某一步和根本原因，而非笼统反馈

### 1.2 Q-Learning Router (借鉴 supostat/engram)

```rust
struct QLearningRouter {
    // 状态: (task_type, complexity, memory_available, tool_set)
    // 动作: (model_choice, search_strategy, retrieval_depth, proactivity_level)
    q_table: HashMap<StateAction, f64>,
    alpha: f64,    // 学习率
    gamma: f64,    // 折扣因子
    epsilon: f64,  // ε-greedy 探索
}

// Q(s,a) ← Q(s,a) + α[r + γ·max_a' Q(s',a') - Q(s,a)]
// 奖励 = 任务成功率 × 0.5 + 用户满意度 × 0.2 + token效率 × 0.15 + 时间效率 × 0.15
```

**四个路由级别**:
1. 搜索策略选择 (FTS5/Vector/Graph/Hybrid)
2. LLM 模型选择 (Flash/Pro/Thinking-Budget)
3. 上下文化深度 (shallow overview vs deep recall)
4. 主动性级别 (主动 vs 被动信息获取)

### 1.3 Lifecycle Guard (借鉴 Ratchet 不退化定理)

```rust
struct LifecycleGuard {
    baseline_score: f64,          // 无优化时的基线
    active_skill_cap: usize,      // 活跃技能数量上限 = 10
    retirement_threshold: f64,    // 评分 < 0.3 → 退役
}

impl LifecycleGuard {
    async fn guard(&self, skill: &Skill, result: &ActionResult) -> GuardDecision {
        // 引入后性能下降 → 回滚
        if result.score < self.baseline_score * 0.95 {
            return GuardDecision::Rollback { skill_id: skill.id };
        }
        // 活跃数超限 → 退役最低分
        if active_count > self.active_skill_cap {
            return GuardDecision::RetireWeakest;
        }
        GuardDecision::Accept
    }
}
```

**核心保证**: bounded cap + retirement threshold → 期望性能永不跌破无技能基线 (Ratchet 不退化命题)

## Layer 2: 离线深度改进

### 2.1 Experience Graph (借鉴 EXG)

```rust
struct ExperienceGraph {
    nodes: Vec<ExperienceNode>,
    edges: Vec<ExperienceEdge>,
}

struct ExperienceNode {
    node_type: NodeType,    // SuccessPattern | FailurePattern | EdgeCase | Insight
    task_signature: Vec<u8>,// 任务特征向量 (用于相似任务匹配)
    context: ExperienceContext,
    outcome: Outcome,
    lessons: Vec<String>,
    confidence: f64,
}

struct ExperienceEdge {
    edge_type: EdgeType,    // CausedBy | SimilarTo | GeneralizesTo | PreventsFrom
    weight: f64,
}

// 新任务时:
// 1. 在图中搜索相似情境
// 2. 提取成功模式 → 复用
// 3. 检查失败模式 → 避坑
// 4. 检查边缘案例 → 增加防御逻辑
```

### 2.2 Skill Lifecycle 管理 (借鉴 Ratchet)

```rust
struct SkillManager {
    skills: Vec<Skill>,
    meta_skill: MetaSkill,   // "如何创建和评估 Skill" 的元技能
    config: SkillConfig {
        active_cap: 10,            // 活跃上限
        retirement_threshold: 0.3,  // 连续 20 次使用平均 < 0.3 → 退役
        evaluation_window: 20,
    },
}

struct Skill {
    id: SkillId,
    content: String,              // SKILL.md 格式
    performance_history: VecDeque<PerformanceRecord>,
    created_from: Vec<ExperienceId>,
    status: SkillStatus,          // Draft | Active | Deprecated
    version: u32,
}

// 关键机制:
// 1. Outcome-driven retirement: 表现不达标 → 自动退役
// 2. Bounded active-cap: 新技能进入必须淘汰旧技能
// 3. Meta-skill guidance: 由 Meta-Skill 模板引导创建 (而非 LLM 凭空生成)
// 4. Git versioning: 每次变更可回滚
```

### 2.3 Sleeptime Consolidation Pipeline

```
触发: Agent 空闲 > 5 分钟 或 每 30 分钟周期

Pipeline:
  1. Decay      → 对所有 L1 记忆施加时间衰减
  2. Dedup      → SimHash 检测 >0.95 相似记忆 → 合并
  3. Triage     → 强度 < 阈值的记忆 → 二次摘要后归档
  4. Cluster    → 向量聚类 → LLM 命名主题 → 生成跨论文洞察
  5. Reconcile  → 检测矛盾 → 标记置信度 → 尝试消解
  6. Skill-Eval → 评估活跃 Skill 的近 N 次表现 → 退役不达标者
  7. Meta-Update→ 根据近期经验更新 Meta-Skill
  8. Rebuild    → 重建 FTS5 + HNSW 索引
```

## 完整闭环

```
Task → Agent → Outcome
         │         │
    ┌────┘         └────┐
    ▼                    ▼
Step-Reflection    Experience Graph
    │                    │
    ▼                    ▼
Q-Learning Router   Skill Lifecycle
    │                    │
    ▼                    ▼
Lifecycle Guard ← Sleeptime Consolidation
    │                    │
    └────────┬───────────┘
             ▼
        Meta-Skill Update → 下一轮任务
```

## 核心设计原则

1. **最少机制 + 最大卫生**: 加机制不如管好已有的 (Ratchet 洞见)
2. **两层分工**: Layer1(Flash, 在线) 负责速度，Layer2(Pro, 离线) 负责深度
3. **防退化优先于学习**: LifecycleGuard 确保"不会越学越差"
4. **Skill 生命周期 > Skill 生成**: 退役、评估、版本管理比凭空创造更重要
5. **Step-level 而非 Episode-level**: 立即纠错比为时已晚的总结更有效
6. **结构化经验**: 散落的失败案例 → Experience Graph 的关系节点
