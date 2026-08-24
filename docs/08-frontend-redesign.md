# 前端优化点与设计方案（v2）

> 对应总体目标：①性能强、可追溯可审计、长程任务；②文献+知识图谱→致病机理假说+辩论精炼；③假说→可执行验证计划（数据分析+湿实验）；④数据分析端到端执行并交付 notebook。
>
> 现状基线：`crates/server/src/static/`（index.html 179 行 / app.js ~1850 行 / styles.css ~414 行），三栏布局（任务侧栏 / 聊天主栏 / Progress+Files 右栏），4 种模式（workflow / loop / debate / research），WebSocket 流式 + marked 渲染 Markdown。

## 一、核心判断

当前前端是一个**通用聊天工作台**，而四个目标需要的是一个**科研过程工作台**。差距不在视觉，而在信息架构：研究管线的 7 个阶段（文献 → KG → 链路预测 → 假说 → 辩论 → 计划 → 分析）各自产出的结构化数据（`papers.json` / `kg.json` / `hypotheses*.json` / `debate_report.json` / `plans/*.json` / `analysis/**.ipynb`）目前全部被压扁成"聊天流里的文本 + 文件列表里的文件名"，用户无法在界面上直接完成"看图谱、比假说、查证据、读 notebook"这四个高频动作。

优化原则：**聊天流负责"过程"，新增的结构化视图负责"结果"；两者通过任务卡片互相跳转。**

## 二、优化点清单（按目标分组）

### A. 目标②③④：研究管线专用视图（优先级最高）

| # | 优化点 | 现状痛点 | 方案 |
|---|---|---|---|
| A1 | **阶段时间线（Pipeline Timeline）** | research 模式的 Progress 栏是通用事件流，无法一眼看出"7 个阶段走到哪、各耗时多少、产出了什么" | 右栏 research 模式下显示专用时间线：每阶段一张卡片（状态图标 / 耗时 / 产物计数如"12 篇文献"、"116 实体/123 关系"、"5 假说"），数据来自 `project.json` 的 `stages[]`；断点续跑（resume）时已完成阶段显示 ↻ 标记 |
| A2 | **知识图谱交互可视化** | `kg.json` 只是一个文件，用户看不到图谱 | 引入 cytoscape.js（或 vis-network）：力导向布局，实体按类型着色（Gene/Protein/Pathway/Disease/Drug…），疾病锚点高亮放大；链路预测候选边（`candidates.json`）以虚线+分数渲染，点击候选边弹出"为什么推荐"（KGE/路径/GIVE 三分量分数）；点击实体显示关联文献 PMID |
| A3 | **假说对比工作台** | 假说散落在聊天文本与 JSON 里 | 假说卡片网格：每张卡片含陈述、机制摘要、新颖度/置信度徽章、支持证据数 vs 反证数；辩论后置信度变化以箭头显示（0.60 → 0.85 ↑）；裁判选出的最强假说加"🏆 最强"角标；假说间矛盾（`comparison.contradictions_between`）以 A↔B 关系列表呈现，合并建议单独区块 |
| A4 | **验证计划视图** | 计划只有 JSON | 两栏：左=数据分析任务表（GEO accession 链接到 ncbi、统计方法、优先级进度条、目标），右=湿实验方案卡（步骤编号清单、试剂、对照、预期结果、周期天数与可行性评分徽章）；每项可勾选"导出为协议文档" |
| A5 | **Notebook 交付视图** | `.ipynb` 只能下载，无法在线查看 | 服务端新增 `GET /api/tasks/{id}/notebook?path=...` 返回 nbformat JSON；前端渲染 cell 列表（markdown cell 渲染、code cell 等宽字体+语法高亮、output 内联图片 base64 直接显示）；顶部显示执行后端徽章（jupyter / python / dry_run）、成功状态、`provenance.json` 摘要（脚本哈希、环境、git commit） |
| A6 | **最终报告全屏阅读视图** | `{brief}.md`（最终报告，含 TL;DR/9 个章节）只能下载后本地打开 | 聊天流在任务完成时嵌入"最终报告卡片"（TL;DR + 关键数字：N 篇文献/KG 规模/假说数/计划数/分析任务数），点击进入全屏阅读模式：服务端渲染（或前端 marked）+ 右侧目录导航（TOC）+ 引用锚点跳转（PMID 链接已在报告内） |

### B. 目标①：可追溯、可审计

| # | 优化点 | 现状痛点 | 方案 |
|---|---|---|---|
| B1 | **审计时间线** | `/api/trace` 已返回 event_log，但前端 Trace 视图信息密度低 | 按 stage 分组的垂直时间线，每条事件可展开：LLM 调用显示模型名/输入输出摘要/token 数/耗时，工具调用显示参数与结果摘要；支持按类型过滤（llm/tool/stage/analysis） |
| B2 | **溯源（Provenance）面板** | `/api/provenance` 端点此前有路径 bug（本次已修复为按任务目录递归查找），前端完全没有入口 | 每个分析任务的 Notebook 视图顶部显示溯源链卡片：输入文件（哈希）→ 脚本（哈希）→ 输出文件（哈希）、执行环境（pip freeze 摘要）、种子、git commit；提供"复现此分析"按钮（用相同输入重跑） |
| B3 | **成本与预算仪表** | token 消耗只在后端日志 | 右栏顶部常驻：本任务累计 token / 按阶段分解的迷你柱状图 / loop 模式成本阈值进度条（`loop_cost_token_threshold` 接近时预警色） |

### C. 目标①：长程任务

| # | 优化点 | 现状痛点 | 方案 |
|---|---|---|---|
| C1 | **断点续跑 UI** | 后端已支持 resume（`project.json` 阶段完成标记 + 产物复用），前端无感知 | 任务详情显示"已完成阶段 / 待执行阶段"；中断任务（服务重启恢复的）在侧栏显示 ↻ 徽章 + "继续运行"按钮（research 模式用 `--project-dir` 语义，同目录重发查询） |
| C2 | **长列表虚拟化与事件折叠** | 长程任务数千条事件全部追加 DOM，聊天流越来越卡 | 事件按阶段分组折叠（默认收起，只显示阶段摘要行）；消息列表引入虚拟滚动或"加载更多"分页；大段流式输出按 chunk 合并渲染 |
| C3 | **并行任务看板** | 多任务同时跑时只能在侧栏切换单看 | 顶栏"运行中"下拉：N 个并发任务的迷你进度（当前阶段名 + 耗时），hover 预览最近事件（服务端 `event_guards` 修复后已支持并发流不串扰） |
| C4 | **取消/暂停语义明确化** | 只有 Stop | Stop 改为两段确认："停止（保留已完成产物，可续跑）"vs"放弃"；停止后任务卡片显示可恢复状态 |

### D. 输入与效率

| # | 优化点 | 现状痛点 | 方案 |
|---|---|---|---|
| D1 | **research 模式参数面板** | `-n/--top-n/--no-debate/--min-year/--data` 等参数只有 CLI 能设 | 模式选成 research 时输入框上方展开参数条：文献数 slider(10–100)、验证假说数、辩论开关、年份下限、本地数据文件（复用现有 upload） |
| D2 | **文献语言提示** | 中文查询的翻译失败会静默回退（本次 e2e 即踩中：中文原样发 PubMed 检回无关文献） | 查询翻译完成后在聊天流显示"检索式：xxx (translated)"气泡；翻译失败时显式警告并建议改用英文检索式 |
| D3 | **文件预览增强** | ipynb/大 JSON 无预览 | A5 的 notebook 渲染 + JSON 折叠树视图（复用 trace 的渲染逻辑） |

## 三、实施方案（三个阶段，均不破坏现有布局）

### 阶段 1：纯前端改造（无后端改动，1–2 天量级）
- A6 报告卡片 + 全屏阅读（数据已有：`finalize_task` 已推送 files 列表，报告即 `{brief}.md`，直接 fetch preview 接口渲染）
- A1 阶段时间线：research 完成事件已含阶段信息；运行中由现有 progress 事件驱动
- C2 事件折叠、D1 参数条、C4 停止确认
- 视觉：阶段色编码统一（文献=蓝 #3b82f6 / KG=紫 #8b5cf6 / 假说=橙 #f59e0b / 辩论=红 #ef4444 / 计划=绿 #22c55e / 分析=青 #06b6d4），与现有暗色主题叠加

### 阶段 2：小后端增量（2–3 天量级）
- `GET /api/tasks/{id}/notebook?path=`（返回 nbformat + outputs，供 A5）
- `GET /api/tasks/{id}/kg`（kg.json + candidates.json 合并 payload，供 A2）
- `GET /api/tasks/{id}/hypotheses`（hypotheses_refined_full + debate_report 合并，供 A3）
- B3 成本事件：drain_progress_channel 聚合 token 用量随 WS 推送
- C1 续跑：POST `/api/tasks/{id}/resume`

### 阶段 3：可视化库引入（3–5 天量级）
- cytoscape.js（CDN 或 vendored，与 marked 同策略）实现 A2
- notebook 渲染器（自研轻量 nbformat→DOM，或 vendor @jupyterlab/nbconvert 的 CSS）
- B1 审计时间线升级（虚拟滚动 + 过滤）

### 不建议做的
- 不引入前端框架（React/Vue）重写：当前 app.js 规模可控、无构建链，改造成本高于收益
- 不做实时协作编辑 notebook：与"可复现交付"目标无关，执行仍走服务端 Jupyter
- 不自绘图库：KG 用 cytoscape.js，不手写 WebGL

## 四、验收标准（对应四个目标）

1. 用户打开一个完成的 research 任务，**10 秒内**能从最终报告卡片跳到：KG 图谱、假说对比、验证计划、任一 notebook（目标②③④的界面闭环）
2. 任意分析结果能一路点到 provenance 的输入哈希与环境快照（目标①可审计）
3. 5000+ 事件的长程任务页面滚动不卡顿；中断任务一键续跑（目标①长程+性能）
4. research 全参数在前端可配置，无需 CLI（目标②易用性）
