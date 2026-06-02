# 无状态 Agent Loop 设计

## 核心概念

```
有状态 (Stateful)                     无状态 (Stateless)
┌─────────────────┐                   ┌─────────────────┐
│   Agent          │                   │   Agent          │
│  ┌─────────────┐ │                   │  run(history) →  │
│  │ 隐藏内部状态  │ │                   │    delta         │
│  │ messages[]   │ │  ← 你无法控制      │                 │
│  │ scratchpad   │ │                   └────────┬────────┘
│  │ tool_results  │ │                           │
│  └─────────────┘ │                   ┌────────┴────────┐
│       ↓          │                   │  Caller 拥有     │
│  run(msg) → text │                   │  history[]       │
└─────────────────┘                   │  checkpoints     │
                                       │  forks           │
                                       └─────────────────┘
```

## 对比 Rust 生态中的两种范式

| 维度 | 有状态 (atomr-agents) | 无状态 (tkach) |
|------|----------------------|----------------|
| 状态归属 | Agent 内部持有 | 调用者持有 |
| Fork 分叉 | 需要框架支持 | `history.clone()` 天然支持 |
| 检查点 | 框架接管序列化 | 调用者控制检查点粒度 |
| 重试/回溯 | 需框架实现状态回滚 | `history.truncate(pos)` |
| 可审计性 | 需从 Agent 导出轨迹 | 历史即完整审计日志 |
| 多模型路由 | 框架内切换 | 调用者按 strategy 选择 |
| 测试 | 需构造 Agent 完整状态 | 纯函数式，给定输入必有确定输出 |

## 长程科研任务的六大需求 → 无状态如何满足

### 1. Fork 分叉探索

科研中，读完 100 篇论文后可能产生多个竞争假设，需要并行验证：

```
history @ step_42 ──┬── fork_A: "假设X：机制A→B" → 实验设计A
                     ├── fork_B: "假设Y：机制C→D" → 实验设计B
                     └── fork_C: "假设Z：混杂因素E" → 实验设计C
```

有状态 Agent 内部状态纠缠，分叉困难。无状态下 `history.clone()` 3 份，各自继续。

### 2. 检查点/断点续传

总结 200 篇论文的数小时长程任务，进程崩溃不能从头开始。无状态设计下，`history` 和 `checkpoint` 就是完整的可序列化状态：

```rust
// 每完成一篇论文总结，自动写入检查点
checkpoint.save(Checkpoint {
    step_id: 42,
    history: current_history.clone(),
    progress: Progress { papers_done: 37, papers_total: 150 },
    intermediate_results: vec![kg_snapshot],
}).await?;
// 崩溃 → 重启 → 从 step_42 继续
```

### 3. 重试/回溯

LLM 在某一步产生幻觉或低质量推理。有状态 Agent 难以"撤销"。无状态下 `history.truncate(last_good_step)` 重新运行。

### 4. 可审计性

科研要求每一步可追溯、可复现。无状态设计天然提供完整不可变轨迹：每一步的输入、输出、工具调用、模型选择。

### 5. 多模型路由

简单任务用 Flash，复杂推理用 Pro。路由决策在调用者手中：

```rust
let model = if step.requires_deep_reasoning() {
    ProviderRouter::Pro
} else {
    ProviderRouter::Flash
};
agent.run_with_provider(&history, model, cancel).await?;
```

### 6. 可组合的子Agent

分解大型任务为子Agent并行执行，每个子Agent独立运行——无共享可变状态，天然适合并行。

## 核心 API 设计

```rust
/// AgentDelta — Agent 只返回增量，不持有状态
pub struct AgentDelta {
    pub new_messages: Vec<Message>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub tool_results: Vec<ToolResult>,
}

/// Agent — 完全无状态
pub struct Agent {
    provider_router: ProviderRouter,
    tool_executor: ToolExecutor,
    policy_engine: PolicyEngine,
    memory_manager: MemoryManager,
    checkpoint_manager: CheckpointManager,
    self_improver: OnlineSelfImprover,   // Step-Reflection + Q-Learn + Guard
}

impl Agent {
    pub async fn run(
        &self,
        history: &[Message],          // 调用者拥有
        context: &RunContext,         // 预算、工具、策略
        cancel: CancellationToken,    // 协作取消
    ) -> Result<AgentDelta> {
        // 1. 上下文组装：注入相关记忆、裁剪到 token 预算
        let assembled = self.memory_manager
            .assemble_context(history, context).await?;

        // 2. 模型路由：根据任务复杂度选择 Flash/Pro
        let provider = self.provider_router.select(context);

        // 3. LLM 调用
        let delta = provider.complete(
            &assembled.system_prompt,
            &assembled.messages,
            &context.tools,
            &context.inference_config,
            cancel.child(),
        ).await?;

        // 4. 工具调度：只读并行(tokio::join_all)，写入串行
        let tool_results = self.tool_executor
            .execute_batch(&delta.tool_calls, &cancel)
            .await?;

        // 5. Step-Level Reflection (在线自改进)
        self.self_improver
            .reflect_on_step(history, &delta, &tool_results)
            .await?;

        // 6. 检查点（由调用者配置频率）
        self.checkpoint_manager
            .maybe_save(history, &delta)
            .await?;

        Ok(AgentDelta { new_messages, stop_reason, usage, tool_results })
    }
}
```

## 总结

无状态 Agent Loop 将"控制权"从框架还给调用者。对于长程科研任务，分叉探索、断点续传、可审计、可回溯、可组合——这些能力不是架构附加功能，而是无状态设计的天然后果。
