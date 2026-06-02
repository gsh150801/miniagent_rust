# 多智能体架构重设计方案

## 设计目标

1. **输出不丢失** — 文件系统即上下文 (Manus)，所有中间结果持久化
2. **真正并行** — 修复 state_graph.rs 中 Parallel 节点线性执行 bug
3. **检查点可恢复** — 检查点持久化到磁盘而非 drop
4. **层级分解结构化** — 替换脆弱的字符串解析
5. **上下文不爆炸** — 替换 O(n²) 上下文拼接，改为增量文件读取
6. **KV-cache 友好** — 稳定 prompt 前缀 + append-only 上下文
7. **错误保留学习** — 保留失败历史，不隐藏错误
8. **注意力控制** — todo.md 机制防止长程漂移

## 架构概览

```
┌──────────────────────────────────────────────────────────────┐
│                     Supervisor (监督者)                        │
│  任务分解 → 委派 → 监控进度 → 合成结果                         │
│  持有: task.md, plan.json, progress.md                        │
├──────────────────────────────────────────────────────────────┤
│  ┌─────────┐ ┌─────────┐ ┌──────────┐ ┌─────────────────┐  │
│  │ Planner │ │Researcher│ │ Executor │ │   Reviewer      │  │
│  │ 规划者  │ │ 研究者   │ │ 执行者   │ │   审查者        │  │
│  └────┬────┘ └────┬────┘ └────┬─────┘ └────┬────────────┘  │
│       │           │           │             │               │
│  ┌────┴────┐ ┌────┴────┐ ┌───┴─────┐ ┌────┴────────────┐  │
│  │  Critic │ │Synthesiz│ │  Writer  │ │   Evaluator     │  │
│  │ 批评者  │ │ 合成者   │ │ 写作者   │ │   评估者        │  │
│  └─────────┘ └─────────┘ └──────────┘ └─────────────────┘  │
├──────────────────────────────────────────────────────────────┤
│                   Shared Workspace (共享工作空间)              │
│  blackboard.json │ todo.md │ events.log │ artifacts/        │
│  每个 agent 有独立的读写目录，可读取所有目录                     │
└──────────────────────────────────────────────────────────────┘
```

## 核心 10 个角色

| 角色 | 文件 | 职责 | 模式 | 关键产出 |
|------|------|------|------|---------|
| Supervisor | `supervisor.rs` | 任务分解、委派、进度跟踪 | Pro | plan.json, progress.md |
| Planner | `planner.rs` | 生成/更新执行计划 | Pro | plan.json, steps/ |
| Researcher | `researcher.rs` | 信息检索、事实提取、证据收集 | Flash | findings.json, sources.md |
| Critic | `critic.rs` | 评估质量、发现缺陷、提出改进 | Pro | critique.json |
| Synthesizer | `synthesizer.rs` | 多源信息融合、矛盾解决 | Pro | synthesis.json |
| Executor | `executor.rs` | 工具调用、代码执行、文件操作 | Flash | output.json, artifacts/ |
| Writer | `writer.rs` | 报告撰写、格式化输出 | Pro | report.md, draft.json |
| Reviewer | `reviewer.rs` | 最终质量审查、标准检查 | Pro | review.json |
| Evaluator | `evaluator.rs` | 结果评估、评分、迭代建议 | Pro | evaluation.json |
| Observer | `observer.rs` | 事件记录、上下文压缩、状态快照 | Flash | events.log, context.md |

## 核心设计变更

### 1. FileContext — 文件系统即上下文 (Manus 启发)

```rust
/// 每个 Role 的输出必须写入文件系统。
/// 压缩时：丢弃内容，保留文件路径，需要时重新加载。
trait FileContext: AgentRole {
    /// 角色的工作目录 (读写)
    fn workspace_dir(&self) -> &Path;
    /// 列出该角色产出的所有文件
    fn artifacts(&self) -> Vec<PathBuf>;
    /// 从文件系统恢复上一次执行结果
    fn restore_from_disk(&self) -> Option<RoleOutput>;
    /// 将输出持久化到磁盘
    fn persist(&self, output: &RoleOutput) -> Result<(), io::Error>;
}
```

### 2. EventStream — 事件溯源 (OpenHands 启发)

```rust
enum AgentEvent {
    TaskStarted { task_id: String, agent: String, timestamp: DateTime<Utc> },
    ToolInvoked { agent: String, tool: String, input: Value },
    ToolResult { agent: String, tool: String, output: Value, success: bool },
    OutputProduced { agent: String, file: String, size_bytes: usize },
    ErrorOccurred { agent: String, error: String, recoverable: bool },
    TaskCompleted { task_id: String, agent: String, duration: Duration },
    CheckpointSaved { path: String, state_hash: String },
}
```

所有事件追加写入 `events.log`，角色通过读取事件流获取上下文，而非拼接所有消息。

### 3. TodoAttention — 注意力控制 (Manus 启发)

```rust
struct TodoAttention {
    tasks: Vec<TodoItem>,  // 当前目标清单
    max_items: usize,      // 限制条目防止膨胀
}

impl TodoAttention {
    /// 每轮迭代调用：重写 todo.md 到上下文末尾
    fn refresh(&mut self, blackboard: &Blackboard) -> String;

    /// 从文件系统恢复 (重启后)
    fn load_from_disk(work_dir: &Path) -> Self;
}
```

### 4. ParallelExecutor — 真正并行执行

```rust
async fn execute_parallel(
    agents: &[Arc<dyn FileContext>],
    task: &str,
    blackboard: &Blackboard,
    cancel: CancellationToken,
) -> Vec<Result<RoleOutput, AgentError>> {
    let handles: Vec<_> = agents.iter().map(|agent| {
        let agent = agent.clone();
        let task = task.to_string();
        let cancel = cancel.child_token();
        tokio::spawn(async move {
            let result = agent.execute(&task, &blackboard, cancel).await;
            // 无论成功失败，都持久化到磁盘
            if let Ok(ref output) = result {
                agent.persist(output).ok();
            }
            result
        })
    }).collect();

    // 等待所有完成，收集结果
    join_all(handles).await
}
```

### 5. 结构化任务委派 (替代字符串解析)

```rust
#[derive(Serialize, Deserialize)]
struct Delegation {
    task_id: String,
    agent: String,
    description: String,
    dependencies: Vec<String>,  // 依赖的前置任务 ID
    input_files: Vec<String>,   // 需要读取的文件
    expected_output: String,    // 预期产出描述
    priority: u8,
}

// Supervisor 输出 JSON 而非 "AGENT: xxx | TASK: yyy"
```

### 6. Checkpoint 持久化

```rust
// 修复: 检查点写入磁盘而非 drop
impl Checkpoint {
    pub fn save_to_disk(&self, work_dir: &Path) -> Result<PathBuf, io::Error> {
        let dir = work_dir.join("checkpoints");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("ckpt_{}_{}.json", self.node_name, self.id));
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)?;
        Ok(path)
    }
}
```

### 7. 上下文增量加载 (替代 O(n²) 拼接)

```rust
// 旧: 每个节点拼接所有历史消息 → O(n²)
// 新: 角色只读取相关文件 + 最近 N 条事件
fn build_context_for_role(
    role: &str,
    blackboard: &Blackboard,
    event_stream: &EventStream,
    max_events: usize,
) -> String {
    // 1. 读取 todo.md (注意力锚点)
    let todo = fs::read_to_string(blackboard.work_dir.join("todo.md"))
        .unwrap_or_default();

    // 2. 读取角色专属文件
    let role_files = read_role_artifacts(&blackboard.work_dir, role);

    // 3. 最近 N 条相关事件
    let recent_events = event_stream.last(max_events)
        .filter(|e| e.is_relevant_to(role));

    format!("{todo}\n\n{role_files}\n\n## Recent Activity\n{recent_events}")
}
```

## 角色间协作模式

### 模式 A: Sequential Pipeline (串行流水线)
```
Researcher → Critic → Synthesizer → Writer → Reviewer
```
适用于: 标准科研报告流程

### 模式 B: Orchestrator-Workers (中心分解)
```
Supervisor → [Researcher, Executor, Writer] (并行)
         ← 汇总
Supervisor → Evaluator → [如有需要再迭代]
```
适用于: 复杂多步骤任务

### 模式 C: Evaluator-Optimizer (评估优化循环)
```
Executor → Evaluator → (不满足) → Planner (调整) → Executor → ...
                                 → (满足) → Writer
```
适用于: 需要迭代改进的任务

### 模式 D: Debate Triad (辩论三角)
```
Proposer → Opponent → Judge → (REVISE) → Proposer → ...
                              → (ACCEPT/REJECT) → Writer
```
适用于: 科学假设验证

## 上下文压缩策略 (Manus 最佳实践)

1. **保留错误** — 失败的尝试保留摘要，不删除
2. **文件路径替代表述** — 长内容替换为 "See: researcher/findings.json (2.3KB)"
3. **增量摘要** — Observer 角色定期生成 context.md 摘要
4. **KV-cache 友好** — system prompt 固定不变，用户消息 append-only
5. **掩码工具** — 工具集固定，通过 logit mask 控制可用性
6. **结构化变异** — 事件格式引入微小随机变化，防止 few-shot 陷阱

## 实现计划

### Phase 1: 基础重构
- [x] 创建 `roles/` 目录结构
- [x] 实现 `FileContext` trait
- [x] 实现 `EventStream`
- [x] 实现 `TodoAttention`
- [x] 重写 10 个角色文件 (实际 13 个)

### Phase 2: 编排引擎
- [x] 重写 `StateGraph` — 修复并行执行 bug (使用 join_all 真正并行)
- [x] 重写 `Orchestrator` — 结构化委派 (JSON Delegation)
- [x] 重写 `ControlShell` — 基于事件流驱动
- [x] Checkpoint 持久化到磁盘

### Phase 3: 上下文管理
- [x] 增量上下文加载
- [x] Observer 角色实现
- [x] 压缩策略实现

### Phase 4: 集成测试
- [x] 端到端工作流测试 (7 个 E2E 测试: chain, parallel, hierarchical, debate, graph并行, 全流程共享状态, 断点恢复)
- [x] 断点恢复测试 (checkpoint_save_and_load, checkpoint_list_and_find_latest)
- [x] 并行执行正确性测试 (state_graph_parallel_waves)
