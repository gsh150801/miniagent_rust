# Miniagent 设计文档

基于 2025-2026 年 GitHub Rust Agent 生态和学术前沿的广泛调研总结。

| 文档 | 内容 |
|------|------|
| [00-overall-architecture](./00-overall-architecture.md) | 总体架构、技术选型、Crate 拆分、实现路线图 |
| [01-stateless-agent-loop](./01-stateless-agent-loop.md) | 为什么选择无状态 Agent Loop、六大需求分析、核心 API |
| [02-memory-system](./02-memory-system.md) | 四层记忆架构、五大核心机制、遗忘曲线、Consolidation 三段式 |
| [03-self-improvement](./03-self-improvement.md) | 双层自改进架构、Step-Reflection、Q-Router、Lifecycle Guard、Experience Graph |
| [04-knowledge-graph-hypothesis](./04-knowledge-graph-hypothesis.md) | KG 构建、链路预测混合评分、GIVE 外推、LLM 假设生成、多维排序 |
| [05-multi-agent-comparison](./05-multi-agent-comparison.md) | 多智能体框架对比与选型 |
| [06-multi-agent-redesign](./06-multi-agent-redesign.md) | Loop 管线重设计（Explore→Plan→Dispatch→Evaluate→Repair） |
| [07-mlevolve-integration](./07-mlevolve-integration.md) | MLEvolve 进化机制集成设计 |
| [08-frontend-redesign](./08-frontend-redesign.md) | Web UI 演进（操作卡/多标签预览/假说卡片） |
| [optimization-roadmap](./optimization-roadmap.md) / [optimization-changelog](./optimization-changelog.md) | 优化路线图与变更记录 |
| **[11-architecture-review](./11-architecture-review.md)** | **架构评审：优势/不足分析、功能状态矩阵（✅/⚠️/❌）、下一步建议——了解项目现状从这里开始** |
