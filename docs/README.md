# Miniagent 设计文档

基于 2025-2026 年 GitHub Rust Agent 生态和学术前沿的广泛调研总结。

| 文档 | 内容 |
|------|------|
| [00-overall-architecture](./00-overall-architecture.md) | 总体架构、技术选型、Crate 拆分、实现路线图 |
| [01-stateless-agent-loop](./01-stateless-agent-loop.md) | 为什么选择无状态 Agent Loop、六大需求分析、核心 API |
| [02-memory-system](./02-memory-system.md) | 四层记忆架构、五大核心机制、遗忘曲线、Consolidation 三段式 |
| [03-self-improvement](./03-self-improvement.md) | 双层自改进架构、Step-Reflection、Q-Router、Lifecycle Guard、Experience Graph |
| [04-knowledge-graph-hypothesis](./04-knowledge-graph-hypothesis.md) | KG 构建、链路预测混合评分、GIVE 外推、LLM 假设生成、多维排序 |
