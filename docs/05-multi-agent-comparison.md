# 2025 主流长程任务智能体项目对比分析

## 1. Manus — 通用自主智能体

**设计原则:**
- **文件系统即上下文** — 所有中间结果写入文件，压缩仅丢弃内容保留文件路径，随时可恢复
- **todo.md 注意力操控** — 每轮迭代重写目标清单到上下文末尾，防止 "lost-in-the-middle" 漂移
- **保留错误在上下文中** — 不隐藏失败，让模型从错误中学习
- **KV-cache 命中率优化** — 稳定 prompt 前缀、append-only 上下文、确定性序列化、显式 cache 断点
- **掩码而非移除工具** — 使用 logit masking 而非动态增删工具，保持 cache 稳定性
- **避免被 few-shot 陷阱** — 引入结构化变异，防止模型陷入固定行动模式

**智能体角色:** 单体智能体，无多角色设计。通过工具集扩展能力。

**消息机制:** 无显式消息系统，所有状态通过文件系统传递。

**工具系统:** 文件读写、Shell 执行、浏览器操作、代码编辑。

**技能系统:** 无独立技能系统，依赖工具组合。

---

## 2. LangChain — 多智能体架构模式

**设计原则:**
- **四种多智能体模式对比:**
  - Subagents: 中心化编排，主智能体委派子任务
  - Skills: 渐进式工具披露，按需展示能力
  - Handoffs: 状态驱动转移，智能体间传递控制权
  - Router: 并行分派 + 综合，任务路由到专家
- **性能数据:** Subagents 模式在复杂任务中表现最佳 (结构化委派 > 自由转移)

**智能体角色:** 灵活定义，通过 role/goal/backstory 描述角色。

**消息机制:** 基于 LangGraph 的状态图，节点间通过共享状态传递消息。

**工具系统:** 丰富的工具生态，支持自定义工具注册。

**技能系统:** LangGraph 的 StateGraph 提供条件边、循环、持久化检查点。

---

## 3. Anthropic — AI 智能体工作流模式

**设计原则:**
- **六种工作流模式:**
  1. Prompt Chaining — 串行流水线，每步独立验证
  2. Routing — 输入分类路由到不同处理器
  3. Parallelization (Sectioning + Voting) — 并行分片或投票
  4. Orchestrator-Workers — 中心分解 + 并行执行
  5. Evaluator-Optimizer — 双循环评估优化
  6. Autonomous Agent — 自主决策循环
- **ACI (Agent-Computer Interface) 设计原则:** 智能体-计算机接口应像 HCI 一样精心设计
- **多智能体研究系统:** Opus 4 主导 + Sonnet 4 子智能体，比单智能体 Opus 4 性能提升 90.2%

**智能体角色:** Lead Agent (总体规划) + Sub-agents (专业执行)

**消息机制:** 结构化委派：主智能体创建子任务描述，子智能体返回结构化结果。

**工具系统:** 模型上下文协议 (MCP)，标准化工具接口。

**技能系统:** 无独立技能层，通过 prompt 模板 + 工具组合实现。

---

## 4. CrewAI — 角色驱动多智能体框架

**设计原则:**
- **角色三元组:** Role (职责) + Goal (目标) + Backstory (背景故事)
- **每个智能体推荐 3-5 个工具** — 过多工具降低选择质量
- **顺序/层级/定制流程** — 预定义编排模式

**智能体角色:** Researcher, Writer, Analyst, Manager 等，通过 YAML 配置。

**消息机制:** 任务链式传递，前一个任务的输出作为下一个的输入。

**工具系统:** 装饰器注册工具，支持 LangChain 工具生态。

**技能系统:** 通过 Crew + Task 组合定义流程，支持记忆和缓存。

---

## 5. AutoGen / AG2 — 对话驱动多智能体

**设计原则:**
- **对话即编排** — 智能体间通过自然语言对话协调
- **人类参与循环 (Human-in-the-loop)** — 可在任意节点插入人类审批
- **可组合智能体** — 通过组合 AssistantAgent、UserProxyAgent 等构建系统

**智能体角色:** Assistant (执行), UserProxy (人类代理), GroupChatManager (协调)

**消息机制:** 基于对话的消息传递，支持群聊模式 (round-robin/auto speaker selection)

**工具系统:** 函数调用注册，支持代码执行沙箱。

**技能系统:** 通过 Teachable 增量学习，保存经验到向量库。

---

## 6. OpenAI Agents SDK — 轻量多智能体框架

**设计原则:**
- **极简设计** — 最少抽象，三个核心概念: Agent, Handoff, Guardrail
- **Handoff 原语** — 智能体间显式传递控制权
- **Guardrail** — 输入/输出安全护栏

**智能体角色:** 灵活定义，通过 instructions + tools 配置。

**消息机制:** Handoff 机制，智能体可调用 `transfer_to_agent` 转交。

**工具系统:** 函数工具注册，支持并行工具调用。

**技能系统:** 无独立技能层。

---

## 7. Google ADK — Agent Development Kit

**设计原则:**
- **模型无关** — 支持 Gemini, Claude, Llama 等
- **内置评估框架** — 自动化智能体质量评估
- **双向音频流** — 支持实时对话智能体

**智能体角色:** Agent 基类，通过 instruction + tools 定制。

**消息机制:** 事件驱动，支持流式响应。

**工具系统:** 内置 Google 生态工具 (搜索、地图、邮件)，支持自定义。

**技能系统:** 通过 Session Service 管理长期状态。

---

## 8. OpenHands — 开源 AI 软件工程师

**设计原则:**
- **事件流架构** — 每个 prompt/response/tool-call 都是类型化事件
- **沙箱化工作空间** — Docker 隔离执行环境
- **Agent-Computer Interface** — 类似 SWE-agent 的 ACI 设计

**智能体角色:** CodeActAgent (代码行动), PlannerAgent (规划)

**消息机制:** 事件流 (EventStream)，所有操作记录为事件。

**工具系统:** 文件编辑、Shell 执行、浏览器操作，在 Docker 沙箱内。

**技能系统:** 通过 condenser 压缩历史，保留关键事件。

---

## 9. SWE-agent — 软件工程智能体

**设计原则:**
- **Agent-Computer Interface (ACI)** — 精心设计 LLM 与软件环境的交互接口
- **搜索+编辑分离** — 先定位再修改，降低错误率
- **自定义命令** — 为 LLM 设计专用的文件浏览/编辑命令

**智能体角色:** 单体智能体，无多角色设计。

**消息机制:** 命令-响应循环。

**工具系统:** 自定义 ACI 命令 (find_file, open_file, edit, search_dir)

**技能系统:** 无独立技能系统。

---

## 10. Devin — 自主软件工程师

**设计原则:**
- **全栈自主** — 规划→编码→测试→部署端到端
- **浏览器+终端+编辑器集成** — 模拟人类开发环境
- **长期记忆** — 跨会话保持项目上下文

**智能体角色:** 单体智能体，通过工具扩展能力。

**消息机制:** 与用户的自然语言交互。

**工具系统:** 浏览器、终端、编辑器、文件系统。

**技能系统:** 从历史任务中学习，积累项目特定知识。

---

## 关键洞察总结

| 维度 | 最佳实践 | 来源 |
|------|----------|------|
| 上下文管理 | 文件系统即上下文，压缩可恢复 | Manus |
| 注意力控制 | todo.md 持续重写目标 | Manus |
| 缓存优化 | 稳定前缀 + append-only + 掩码工具 | Manus |
| 多智能体模式 | Subagents (结构化委派) 最优 | LangChain, Anthropic |
| 角色设计 | role/goal/backstory + 3-5工具 | CrewAI |
| 事件溯源 | 所有操作记录为类型化事件 | OpenHands |
| 安全隔离 | 沙箱化执行 | OpenHands, SWE-agent |
| 控制转移 | 显式 Handoff 原语 | OpenAI Agents SDK |
| 评估优化 | Evaluator-Optimizer 双循环 | Anthropic |
| 层级分解 | Orchestrator-Workers 模式 | Anthropic |
