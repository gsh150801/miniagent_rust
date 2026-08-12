# miniagent 系统优化方案

> 基于 22 轮优化的总结与前瞻。2026-06-29。

## 一、项目当前状态

### 健康指标
| 指标 | 数值 |
|------|------|
| 构建 | ✅ 通过 |
| 编译警告 | 32 个（provider crate 既有死字段） |
| lib 测试 | 133 通过 / 0 失败 |
| 优化轮次 | 22 轮已完成 |
| suggestions2 | 12 项中 11 项完成、1 项评估不做 |

### 已完成的优化总览（22 轮）

**架构清理（第 2-9 轮）**：ModelTier 统一、StateGraph 动态调度、Blackboard 内存层、角色工具访问、Orchestrator 死代码删除、Elo 衰减、辩论多轮、上下文压缩、安全路径校验、KG RwLock、Checkpoint 截断化

**loop-pipeline 修复（第 10-11 轮）**：任务级增量复用（已成功任务跳过+产物校验）、结果去重、evaluate 真实进度、客观产物校验（幽灵成功检测）

**Provider 路由（第 12-13 轮）**：CLI + Server 统一 `PROVIDER=stepfun` 路由、skill 浏览端点

**三需求交付（第 14-17 轮）**：统一新流程（explore→ask→plan→dispatch→feedback）、全链路追溯（event_log + /api/trace）、日志只记 error、三角色（Executor/Validator/Arbiter）、双向 ws ask、前端重构

**cc-python-claude 借鉴（第 18-22 轮）**：分层提示词、token bytes/4、bash 安全、错误恢复、MaxTokens 续写、权限三态、外部钩子、记忆提取、AskUser、NotebookEdit、PlanOnly、环境注入

---

## 二、待优化项（按优先级分层）

### P0：核心功能缺口（影响任务完成质量）

| # | 项目 | 价值 | 难度 | 依赖 |
|---|------|------|------|------|
| **1** | **transcript 修复**——崩溃恢复时修补孤立 tool_use | 长任务崩溃后恢复的鲁棒性 | 中 | 无 |
| **2** | **三角色接入 dispatch**——execute_with_roles 实际接入任务执行路径 | 三角色已实现+测试但未接入实际执行 | 中 | 无 |
| **3** | **AgentTool 子智能体**——LLM 运行时自主派生子 agent | 从"编排驱动"迈向"LLM 自主+编排辅助"的关键一步 | 高 | Agent 可嵌套 |

### P1：可靠性 & 可扩展性

| # | 项目 | 价值 | 难度 |
|---|------|------|------|
| **4** | **会话即时持久化**——每条消息即时 JSONL 落盘 | 崩溃零丢失（当前最多丢 5 轮） | 低 |
| **5** | **项目级指令加载**——.miniagent.md | 用户通过项目配置控制 Agent 行为 | 低 |
| **6** | **权限规则配置**——settings.json allow/deny + glob | 用户可配置工具白名单 | 中 |
| **7** | **编译警告清理**——32 个 provider crate 死字段 | 零警告目标 | 低 |

### P2：能力增强

| # | 项目 | 价值 | 难度 |
|---|------|------|------|
| **8** | **MCP 协议**——外部工具服务器 | 零代码扩展工具集 | 高 |
| **9** | **流式工具执行**——边收响应边启动工具 | 多 tool_use 延迟重叠 | 高 |
| **10** | **Slash 命令注册**——自定义工作流 | 快速触发常用操作 | 中 |
| **11** | **LLM 语义摘要**——build_incremental_context 异步摘要 | 比 结构截断更精准 | 中 |

---

## 三、实施路线图

### 第一阶段：核心功能补全（P0）

#### 步骤 1：transcript 修复（1 轮，低风险）
- 在 `/api/resume` 和 `restore_tasks_from_disk` 入口加 `validate_transcript`
- 检测孤立 tool_use → 追加合成 error tool_result

#### 步骤 2：三角色接入 dispatch（1 轮，中风险）
- `handle_run` 的 WorkflowBuilder 执行后，对每个 stage 产物走 `execute_with_roles`
- 或在 `execute_single_task` 内部接入

#### 步骤 3：AgentTool 子智能体（1-2 轮，高风险）
- 重构 `Agent` 使其可嵌套
- 新建 `AgentTool`：LLM 可调用，派生子 agent
- 子 agent 继承父工具（排除 AgentTool 防递归）

### 第二阶段：可靠性加固（P1）

#### 步骤 4：会话即时持久化 + 编译警告清理（1 轮）
#### 步骤 5：项目级指令 + 权限规则配置（1 轮）

### 第三阶段：能力扩展（P2，视需求）
6. MCP 协议 / 流式工具 / Slash 命令 / LLM 语义摘要

---

## 四、设计原则（贯穿所有阶段）

1. **安全第一**——路径校验、输出截断、权限分级、危险操作确认
2. **增量优先**——已成功任务跳过、结果去重、增量合并
3. **可追溯**——全链路 event_log + /api/trace + 日志只记 error
4. **LLM 自主 + 编排辅助**——混合模式
5. **环境感知**——时间/平台/工具/语言注入所有 prompt

---

## 五、核心优势（保持不变）

| 优势 | 详情 |
|------|------|
| 多 Provider 路由 | DeepSeek/StepFun + Flash/Pro/Auto + 复杂度分级 |
| 多智能体编排 | planning 13角色 + loop-pipeline 三角色 + workflow DAG |
| 科研导向工具 | pubmed/patent/clinical_trials/kg/notebook + 假设生成 |
| 四层记忆 | SQLite+FTS5+衰减+关系图 + LLM 自动提取 |
| 安全防护 | 路径校验 + 输出截断 + 权限五级 + 外部钩子 |
| 全链路追溯 | event_log + /api/trace + 工具调用日志 |
