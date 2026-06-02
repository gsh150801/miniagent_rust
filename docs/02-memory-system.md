# 超长上下文记忆系统设计

## 问题定义

连续阅读总结 100+ 篇论文时，纯上下文窗口方案的双重退化：

```
维度A: 上下文容量
  Paper 1-20  → 完整保留
  Paper 21-50 → 开始压缩摘要
  Paper 51-100 → 早期论文完全丢失 ("lost in the beginning")
  Paper 100+  → 即使最近论文也只是碎片化印象

维度B: 记忆质量
  4小时后  → 上下文 70% 被 LLM 生成的摘要占据
  8小时后  → 摘要的摘要 → 信息二次失真
  结论:    "不记得论文说了什么，只记得摘要说它很重要"
```

## 业界方案综述

| 系统 | 核心路线 | 关键指标 |
|------|---------|---------|
| Letta (MemGPT) | OS 虚拟内存隐喻，Agent 自主管理 memory block | LoCoMo 74%, 16.7K GitHub stars |
| Hindsight | 四网络结构化记忆 (World/Bank/Opinion/Observation) | LongMemEval 91.4% |
| PRISM | 多关系记忆图 + 层级束搜索 | 0.831 acc @ 2K tokens (比 26K full-context 高 35 分) |
| Memex | Indexed Summary + 显式解引用 | 比纯摘要方案信息损失更低 |
| CALMem | 情景记忆+语义记忆双通道，token 预算自适应注入 | 纯应用层，提供者无关 |
| delta-mem | 4.87M 参数矩阵在线压缩历史 | 参数仅占 backbone 0.12% |

## 四层记忆架构 (OS 隐喻 + 认知科学)

```
┌─────────────────────────────────────────────────────────────┐
│ L0: 工作记忆 (Working Memory) — 上下文窗口，~64K tokens      │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Persona | 任务指令 | L1 索引表 | L3 技能引用 | 当前论文   │ │
│ │ Agent 主动管理: core_memory_replace() / recall()         │ │
│ └─────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────┤
│ L1: 情景记忆 (Episodic) — SQLite FTS5，结构化记忆记录         │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ 每篇论文 = 一条结构化记录:                                 │ │
│ │ { id, title, authors, year,                              │ │
│ │   structured_summary: { background, method, findings,    │ │
│ │     limitations, contributions },                        │ │
│ │   relations: [{type: "contradicts|extends|uses_method",  │ │
│ │     target_id, evidence}],                               │ │
│ │   importance_score: 0.87,   // 动态更新                  │ │
│ │   last_recalled: timestamp,                              │ │
│ │   access_count: 12,                                      │ │
│ │   decay_rate: 0.05 }                                     │ │
│ └─────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────┤
│ L2: 语义记忆 (Semantic) — LanceDB 向量 + 知识图谱            │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ 论文全文 chunk embeddings (BGE-m3, 1024d)                │ │
│ │ 知识图谱: 实体 + 关系 + 证据链                             │ │
│ │ 跨论文概念图谱 + 领域宏观认知                               │ │
│ │ 混合检索: FTS5 + Vector + Graph → RRF 融合                │ │
│ └─────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────┤
│ L3: 技能记忆 (Procedural) — SKILL.md 仓库 + Meta-Skill       │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ 复用的分析模板 ("系统综述方法")                            │ │
│ │ 成功的检索策略 ("用 MeSH term 搜索 PubMed")              │ │
│ │ 实验设计模式 ("case-control 更适合这类问题")              │ │
│ │ 自改进的 Meta-Skill: 指导如何创建/评估 Skill              │ │
│ └─────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## 五大核心机制

### 1. 结构化摘要 + 索引指针 (借鉴 Memex/PRISM)

不把全文塞进上下文，而是：
- 每篇论文 → 固定格式的结构化摘要 (L1 记录)
- L0 上下文只保留 **论文索引表** (标题 + 一行概要 + L1 指针)
- 需要细节时通过 `recall(paper_id, aspect)` 精准检索

```
上下文占比对比:
  传统: [Paper1全文 8K][Paper2全文 7K]...[Paper50摘要 800B]  ← ≈8 篇覆盖
  结构化: [索引表 200篇 × 50B = 10K] + [当前论文 8K]         ← 200 篇追踪
```

### 2. 跨论文关系图 (借鉴 Hindsight + PRISM)

```rust
struct PaperRelation {
    source_id: PaperId,
    target_id: PaperId,
    relation_type: RelationType,
    // Contradicts | Extends | UsesMethod | Supports | CitesAsEvidence
    strength: f32,
    evidence: String,    // 引用文本作为证据
    created_at: DateTime,
}

// 图查询示例
// "找出所有被 paper_042 的方法论挑战的论文"
memory.query_relations(paper_042, RelationType::Contradicts {
    direction: Direction::Incoming,
    max_depth: 3,
}).await?;
```

### 3. Ebbinghaus 遗忘曲线 + 动态重要性 (借鉴 Engram-RS)

```rust
struct MemoryDecay {
    base_decay_rate: f64,    // factual: 0.01 / method: 0.005 / hypothesis: 0.02
    activation_boost: f64,   // +0.3 on recall (use-it-or-lose-it)
    retention_floor: f64,    // 0.01 (永不完全遗忘)
}

// 当前记忆强度 = floor + (1-floor) × exp(-decay_rate × days_since_last_access)
// 每次被检索时 activation += boost
// Sleeptime consolidation 对强度 < 阈值的记忆做二次摘要
```

### 4. 三段式 Consolidation (借鉴 Letta Sleeptime + EngramAI)

```
实时层 (inline):
  └── 每篇论文读完后立即: 写 L1 结构化摘要 → L2 向量 embedding → Link 关系

会话结束层 (episode-end):
  └── 去重(simhash > 0.95) → 矛盾检测 → 弱记忆二次摘要 → 更新重要性评分

空闲层 (sleeptime, 每30分钟):
  └── 全局关系发现(Louvain社区检测) → 跨论文主题聚类 → 生成 Field-Level 摘要
      → 退役无用记忆 → 更新遗忘曲线参数 → 重建检索索引
```

### 5. 上下文组装策略 (借鉴 CALMem MOIM)

```rust
struct ContextAllocation {
    persona: 0.05,           // 5%
    task_instruction: 0.05,  // 5%
    paper_index: 0.10,       // 10% ← 200篇索引表
    current_paper: 0.30,     // 30% ← 当前处理论文
    recalled_context: 0.30,  // 30% ← 检索到的相关上下文
    tool_outputs: 0.15,      // 15% ← 最新工具调用结果
    reserve: 0.05,           // 5%  ← 弹性空间
}

// 根据任务类型动态调整
// Complexity::High → recalled_context ×1.3, current_paper ×0.7
```

## 核心设计原则

- **结构化优先**: 不是"存文本"而是"存 schema"
- **索引优于全量**: L0 存指针，L1/L2 存全量，按需检索
- **遗忘是功能不是 bug**: 模拟人类遗忘曲线，自动退化不重要信息
- **三层 Consolidation 节奏**: inline → episode-end → sleeptime，频率越低任务越重
- **关系图比独立记录更有价值**: 一个跨论文的矛盾发现 > 100 篇独立的摘要
