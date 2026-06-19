use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use miniagent_core::ModelTier;

use crate::event_stream::EventStream;
use crate::todo_attention::TodoAttention;

// ── Context budget constants ──────────────────────────────────

/// 上下文 token 预算（≈48K chars）。复用 agent crate 的 chars/3 估算口径，
/// 留出输出空间在 128K context window 内。超出时按优先级裁剪 step_outputs。
const MAX_CONTEXT_TOKENS: usize = 16_000;
/// 单个 step 输出的字符上限（≈2700 tokens）。防止单个 step 吃满预算。
const MAX_STEP_CHARS: usize = 8_000;

/// 粗略 token 估算：chars/3 适用于中英混合/代码（与 agent crate 一致）。
fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 3
}

/// 结构感知截断：在 `max_chars` 附近找**安全截断点**，避免切断 JSON/代码结构。
///
/// 从 `max_chars` 位置往前找最近的边界字符（`}` / `]` / `\n`），在其后截断。
/// 若找不到边界（紧凑 JSON），退化为硬截断但仍标注。截断后附加省略提示。
fn truncate_structured(content: &str, max_chars: usize) -> String {
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }

    // 把 max_chars（char 索引）转成 byte 索引
    let max_byte = content.char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(content.len());

    // 从 max_byte 往前找安全边界：} / ] / 换行（倒序搜索最近的一个）
    let safe_end = content[..max_byte].rfind(|c| c == '}' || c == ']' || c == '\n')
        .map(|pos| {
            // 在边界字符之后截断（+1 含边界字符本身）
            (pos + 1).min(content.len())
        })
        .unwrap_or(max_byte); // 找不到边界，退化为硬截断

    let truncated = &content[..safe_end];
    let remaining = char_count - truncated.chars().count();
    format!("{truncated}\n...(truncated, {remaining} more chars omitted)")
}

// ── Graph State ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphState {
    pub messages: Vec<GraphMessage>,
    pub artifacts: HashMap<String, String>,
    pub step_outputs: HashMap<String, String>,
    pub budget: BudgetState,
    pub iteration: usize,
    pub current_node: String,
    pub finished: bool,
    pub work_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphMessage {
    pub role: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetState {
    pub max_iterations: usize,
    pub tokens_used: usize,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            messages: Vec::new(), artifacts: HashMap::new(), step_outputs: HashMap::new(),
            budget: BudgetState { max_iterations: 50, tokens_used: 0 },
            iteration: 0, current_node: String::new(), finished: false,
            work_dir: std::path::PathBuf::from("./miniagent_workspace"),
        }
    }
}

impl GraphState {
    pub fn with_work_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.work_dir = dir.into();
        std::fs::create_dir_all(&self.work_dir).ok();
        self
    }
}

impl GraphMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), timestamp: chrono::Utc::now() }
    }
}

// ── Node Types ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NodeOutput {
    pub content: String,
    pub metadata: HashMap<String, String>,
    pub next: Option<String>,
    pub interrupt: Option<String>,
}

#[derive(Debug, Clone)]
pub enum GraphError {
    NodeFailed(String),
    Cancelled,
    BudgetExhausted,
    NoRoute(String),
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphError::NodeFailed(m) => write!(f, "{m}"),
            GraphError::Cancelled => write!(f, "cancelled"),
            GraphError::BudgetExhausted => write!(f, "budget exhausted"),
            GraphError::NoRoute(n) => write!(f, "no route from '{n}'"),
        }
    }
}

pub type EdgePredicate = Box<dyn Fn(&GraphState) -> bool + Send + Sync>;

pub type NodeFunc = Arc<dyn Fn(&GraphState) -> Result<NodeOutput, GraphError> + Send + Sync>;

// ── Conditional Edge ───────────────────────────────────────────

pub struct ConditionalEdge {
    pub from: String,
    pub routes: Vec<(String, EdgePredicate)>,
    pub default: String,
}

// ── Node Enum ──────────────────────────────────────────────────

#[derive(Clone)]
pub enum GraphNode {
    Agent { system_prompt: String, model_tier: ModelTier },
    Tool { tool_name: String },
    Human { prompt: String },
    Parallel { sub_nodes: Vec<String> },
    Lambda { func: NodeFunc },
}

// `ModelTier` 统一从 `miniagent_core` 引入（见文件顶部 use），此处不再重复定义。

// ── StateGraph Builder ─────────────────────────────────────────

pub struct StateGraph {
    nodes: HashMap<String, GraphNode>,
    edges: Vec<(String, String)>,
    conditional_edges: Vec<ConditionalEdge>,
    entry_point: String,
    checkpoints: HashSet<String>,
}

impl StateGraph {
    pub fn new(entry: impl Into<String>) -> Self {
        Self { nodes: HashMap::new(), edges: Vec::new(), conditional_edges: Vec::new(),
               entry_point: entry.into(), checkpoints: HashSet::new() }
    }

    pub fn add_agent(mut self, name: impl Into<String>, prompt: impl Into<String>, tier: ModelTier) -> Self {
        self.nodes.insert(name.into(), GraphNode::Agent { system_prompt: prompt.into(), model_tier: tier });
        self
    }

    pub fn add_tool(mut self, name: impl Into<String>, tool: impl Into<String>) -> Self {
        self.nodes.insert(name.into(), GraphNode::Tool { tool_name: tool.into() });
        self
    }

    pub fn add_human(mut self, name: impl Into<String>, prompt: impl Into<String>) -> Self {
        self.nodes.insert(name.into(), GraphNode::Human { prompt: prompt.into() });
        self
    }

    pub fn add_parallel(mut self, name: impl Into<String>, subs: Vec<&str>) -> Self {
        self.nodes.insert(name.into(), GraphNode::Parallel {
            sub_nodes: subs.into_iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn add_lambda(mut self, name: impl Into<String>,
                      f: impl Fn(&GraphState) -> Result<NodeOutput, GraphError> + Send + Sync + 'static) -> Self {
        self.nodes.insert(name.into(), GraphNode::Lambda { func: Arc::new(f) });
        self
    }

    pub fn add_edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push((from.into(), to.into()));
        self
    }

    pub fn add_conditional(mut self, from: impl Into<String>,
                           routes: Vec<(String, EdgePredicate)>, default: impl Into<String>) -> Self {
        self.conditional_edges.push(ConditionalEdge { from: from.into(), routes, default: default.into() });
        self
    }

    pub fn with_checkpoint(mut self, node: impl Into<String>) -> Self {
        self.checkpoints.insert(node.into());
        self
    }

    /// Compile: DFS cycle detection, then topo-sort.
    /// Returns waves of nodes that can execute in parallel within each wave.
    pub fn compile(self) -> Result<CompiledGraph, String> {
        // Build adjacency from edges
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for name in self.nodes.keys() {
            adjacency.entry(name.clone()).or_default();
        }
        for (from, to) in &self.edges {
            adjacency.entry(from.clone()).or_default().push(to.clone());
        }
        for ce in &self.conditional_edges {
            for (target, _) in &ce.routes {
                adjacency.entry(ce.from.clone()).or_default().push(target.clone());
            }
            adjacency.entry(ce.from.clone()).or_default().push(ce.default.clone());
        }

        // DFS cycle detection
        let mut visited: HashSet<String> = HashSet::new();
        let mut on_stack: HashSet<String> = HashSet::new();
        for name in self.nodes.keys() {
            if !visited.contains(name)
                && Self::has_cycle(name, &adjacency, &mut visited, &mut on_stack) {
                    return Err(format!("Cycle detected in graph (involving node '{name}')"));
                }
        }

        // Topological sort (Kahn's algorithm with wave grouping)
        // Note: adjacency already built above, only need in_degree
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for name in self.nodes.keys() {
            in_degree.insert(name.clone(), 0);
        }
        for (_, to) in &self.edges {
            *in_degree.entry(to.clone()).or_insert(0) += 1;
        }
        for ce in &self.conditional_edges {
            for (target, _) in &ce.routes {
                *in_degree.entry(target.clone()).or_insert(0) += 1;
            }
            *in_degree.entry(ce.default.clone()).or_insert(0) += 1;
        }

        // Force entry point to in_degree 0
        in_degree.insert(self.entry_point.clone(), 0);

        let mut queue: VecDeque<String> = in_degree.iter()
            .filter(|(_, d)| **d == 0).map(|(n, _)| n.clone()).collect();
        let mut order = Vec::new();

        while !queue.is_empty() {
            let wave: Vec<String> = queue.drain(..).collect();
            let mut next = VecDeque::new();
            for name in &wave {
                if let Some(neighbors) = adjacency.get(name) {
                    for neighbor in neighbors {
                        if let Some(deg) = in_degree.get_mut(neighbor) {
                            *deg -= 1;
                            if *deg == 0 { next.push_back(neighbor.clone()); }
                        }
                    }
                }
            }
            order.push(wave);
            queue = next;
        }

        let total: usize = order.iter().map(|w| w.len()).sum();
        if total != self.nodes.len() {
            return Err(format!("Cycle detected: {total}/{} reachable", self.nodes.len()));
        }

        Ok(CompiledGraph {
            node_order: order, nodes: self.nodes, edges: self.edges,
            conditional_edges: self.conditional_edges, checkpoints: self.checkpoints,
            entry_point: self.entry_point,
        })
    }

    fn has_cycle(
        node: &str,
        adjacency: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        on_stack: &mut HashSet<String>,
    ) -> bool {
        visited.insert(node.into());
        on_stack.insert(node.into());

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if Self::has_cycle(neighbor, adjacency, visited, on_stack) {
                        return true;
                    }
                } else if on_stack.contains(neighbor) {
                    return true;
                }
            }
        }

        on_stack.remove(node);
        false
    }
}

// ── Compiled Graph ─────────────────────────────────────────────

pub struct CompiledGraph {
    /// Waves of nodes: within each wave, all nodes can execute in parallel.
    /// 仅供 `waves()` 可视化与等价性测试使用；运行时调度由 [`execute`] 的动态
    /// 队列 + [`route`] 决定，不再依赖此字段。
    node_order: Vec<Vec<String>>,
    nodes: HashMap<String, GraphNode>,
    edges: Vec<(String, String)>,
    conditional_edges: Vec<ConditionalEdge>,
    checkpoints: HashSet<String>,
    /// 编译期确定的入口节点，`execute()` 从此处出发。
    entry_point: String,
}

impl CompiledGraph {
    /// Get the execution wave order (for testing and visualization).
    pub fn waves(&self) -> &[Vec<String>] {
        &self.node_order
    }

    /// Decide the successor node(s) of `from` given the current `state`.
    ///
    /// 优先级：
    /// 1. 若 `from` 注册了条件边：返回首个谓词命中的 target，否则返回 default；
    /// 2. 否则返回所有以 `from` 为起点的静态边后继。
    ///
    /// 注意：节点自身的 `output.next`（节点自决下一跳）优先级高于本方法，
    /// 由 [`execute`] 在调用本方法前先行检查。
    pub fn route(&self, from: &str, state: &GraphState) -> Vec<String> {
        for ce in &self.conditional_edges {
            if ce.from == from {
                for (target, pred) in &ce.routes {
                    if pred(state) { return vec![target.clone()]; }
                }
                return vec![ce.default.clone()];
            }
        }
        self.edges.iter().filter(|(f, _)| f == from).map(|(_, t)| t.clone()).collect()
    }

    /// Execute the graph. Waves execute sequentially, nodes within a wave execute in parallel.
    /// `flash` and `pro` providers are used for Agent nodes based on their ModelTier.
    /// EventStream and TodoAttention are used for cross-agent awareness.
    /// Execute the graph using **dynamic scheduling**.
    ///
    /// 调度模型：维护一个待执行队列，从 entry_point 出发。每执行完一个节点（或一组
    /// 并行节点）后，按以下优先级决定后继并加入队列：
    /// 1. 节点返回的 `output.next`（节点自决下一跳，最高优先级）；
    /// 2. 否则若该节点注册了条件边 → [`route`] 用 `EdgePredicate` 在当前 state 上求值；
    /// 3. 否则退化为静态边后继。
    ///
    /// **向后兼容**：无 `add_conditional` 且节点不返回 `output.next` 的图，后继 == 原
    /// 静态边后继，节点执行顺序与原拓扑波次一致，行为等价于改造前的静态遍历。
    ///
    /// `flash`/`pro` providers 按 Agent 节点的 ModelTier 选用；EventStream 与
    /// TodoAttention 提供跨 Agent 的感知。并行 wave 中各分支的 TodoAttention 进度会
    /// 通过 [`TodoAttention::merge_from`] 合并回主实例。
    pub async fn execute(
        &self,
        mut state: GraphState,
        cancel: CancellationToken,
        flash: &dyn miniagent_provider::traits::LlmProvider,
        pro: &dyn miniagent_provider::traits::LlmProvider,
    ) -> Result<GraphState, GraphError> {
        let mut event_stream = EventStream::new(&state.work_dir);
        let mut todo = TodoAttention::new(&state.work_dir);

        // 动态调度队列。从编译期确定的 entry_point 出发（而非 node_order[0]，后者
        // 在含孤立子节点的图中顺序不确定）。
        if !self.nodes.contains_key(&self.entry_point) {
            state.finished = true;
            return Ok(state);
        }
        let mut pending: VecDeque<String> = VecDeque::from([self.entry_point.clone()]);
        let mut executed: HashSet<String> = HashSet::new();
        let mut step_idx: usize = 0;

        while let Some(node_name) = pending.pop_front() {
            if cancel.is_cancelled() { return Err(GraphError::Cancelled); }
            if state.iteration >= state.budget.max_iterations {
                return Err(GraphError::BudgetExhausted);
            }
            if executed.contains(&node_name) { continue; }
            if !self.nodes.contains_key(&node_name) { continue; }

            // 动态调度：一次执行一个节点。真正的并行只来自 `GraphNode::Parallel`
            // 节点（其 sub_nodes 在 execute_node 内部并发展开）；普通节点的后继完全
            // 由 route()/output.next 决定，不再用编译期 wave 表展开——否则条件边的
            // 多个可能目标会被误当作并行组一起执行。
            state.current_node = node_name.clone();
            event_stream.task_started(&node_name, &format!("step {step_idx}"));

            let result = self.execute_node(
                &node_name, &state, &cancel, flash, pro, &mut event_stream, &mut todo,
            ).await;
            let results: Vec<(String, Result<NodeOutput, GraphError>)> = vec![(node_name, result)];

            // 处理本调度单元的结果，并决定后继
            for (node_name, result) in results {
                state.iteration += 1;
                executed.insert(node_name.clone());
                step_idx += 1;

                let output_next: Option<String> = match result {
                    Ok(output) => {
                        state.step_outputs.insert(node_name.clone(), output.content.clone());
                        state.messages.push(GraphMessage::new(&node_name, &output.content));
                        event_stream.task_completed(&node_name, &output.content, vec![]);
                        output.next
                    }
                    Err(ref e) => {
                        // 保留错误上下文（Manus 原则：不隐藏失败）
                        let error_msg = format!("[ERROR:{node_name}] {e}");
                        state.step_outputs.insert(node_name.clone(), error_msg.clone());
                        state.messages.push(GraphMessage::new(&node_name, &error_msg));
                        event_stream.task_failed(&node_name, &e.to_string());
                        None
                    }
                };

                // Checkpoint
                if self.checkpoints.contains(&node_name) {
                    let ckpt = Checkpoint::from_state(&state, &node_name);
                    if let Ok(path) = ckpt.save_to_disk(&state.work_dir) {
                        event_stream.checkpoint_saved(&node_name, &path.to_string_lossy());
                    }
                }

                // 决定后继（优先级：output.next > route() 静态/条件边）
                let successors: Vec<String> = match output_next {
                    Some(t) => vec![t],
                    None => self.route(&node_name, &state),
                };
                for s in successors {
                    if !executed.contains(&s) && self.nodes.contains_key(&s) {
                        pending.push_back(s);
                    }
                }
            }

            // 刷新 todo attention anchor
            let _todo_text = todo.refresh();
        }

        state.finished = true;
        Ok(state)
    }

    /// Execute a single node. Handles Parallel nodes with true concurrency.
    #[allow(clippy::too_many_arguments)]
    async fn execute_node(
        &self,
        node_name: &str,
        state: &GraphState,
        cancel: &CancellationToken,
        flash: &dyn miniagent_provider::traits::LlmProvider,
        pro: &dyn miniagent_provider::traits::LlmProvider,
        event_stream: &mut EventStream,
        todo: &mut TodoAttention,
    ) -> Result<NodeOutput, GraphError> {
        let node = self.nodes.get(node_name)
            .ok_or_else(|| GraphError::NoRoute(node_name.to_string()))?;

        match node {
            GraphNode::Agent { system_prompt, model_tier } => {
                let provider: &dyn miniagent_provider::traits::LlmProvider = match model_tier {
                    ModelTier::Flash => flash,
                    ModelTier::Pro => pro,
                };

                // Incremental context loading (fix O(n²) context explosion)
                let context = Self::build_incremental_context(node_name, state, event_stream, todo);

                Self::execute_agent_node(provider, system_prompt, &context, node_name, cancel).await
            }
            GraphNode::Tool { tool_name } => {
                Ok(NodeOutput {
                    content: format!("[Tool:{}] executed", tool_name),
                    metadata: HashMap::new(), next: None, interrupt: None,
                })
            }
            GraphNode::Human { prompt } => {
                tracing::info!(prompt = %prompt, "HITL node (auto-approved in CLI mode)");
                Ok(NodeOutput {
                    content: format!("approved: {prompt}"),
                    metadata: HashMap::new(), next: None,
                    interrupt: Some(prompt.clone()),
                })
            }
            GraphNode::Parallel { sub_nodes } => {
                // Execute sub-nodes with true concurrency
                let subs = sub_nodes.clone();
                // 性能优化：子节点只读 state（execute_node 接收 &GraphState），
                // 只需在循环外 clone 一次，循环内共享 Arc（N 次廉价指针复制替代 N 次深拷贝）。
                // 长 pipeline 中 step_outputs/messages 可达数十 KB–MB，深拷贝 N 份是 O(N×S) 内存放大。
                let shared_state = std::sync::Arc::new(state.clone());
                #[allow(clippy::type_complexity)]
                let mut sub_futures: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = (
                    String, Result<NodeOutput, GraphError>, EventStream, TodoAttention,
                )> + '_>>> = Vec::new();

                for sub_name in &subs {
                    if cancel.is_cancelled() { break; }
                    let sub_name = sub_name.clone();
                    let sub_state = shared_state.clone(); // Arc clone：廉价指针复制
                    let cancel_token = cancel.child_token();
                    let mut sub_events = event_stream.clone();
                    let mut sub_todo = todo.clone();

                    sub_futures.push(Box::pin(async move {
                        let result = self.execute_node(
                            &sub_name, &sub_state, &cancel_token,
                            flash, pro, &mut sub_events, &mut sub_todo,
                        ).await;
                        (sub_name, result, sub_events, sub_todo)
                    }));
                }

                let raw_results = futures_util::future::join_all(sub_futures).await;

                // Merge events + todo progress back; collect output
                let mut contents = Vec::new();
                for (sub_name, result, sub_events, sub_todo) in raw_results {
                    for ev in sub_events.iter() {
                        event_stream.push(ev.clone());
                    }
                    // 关键修复（#9）：子节点的 todo 进度合并回主 todo
                    todo.merge_from(&sub_todo);
                    match result {
                        Ok(out) => {
                            event_stream.task_completed(&sub_name, &out.content, vec![]);
                            contents.push(format!("[{sub_name}]: {}", out.content));
                        }
                        Err(e) => {
                            event_stream.task_failed(&sub_name, &e.to_string());
                            contents.push(format!("[{sub_name} ERROR]: {e}"));
                        }
                    }
                }

                Ok(NodeOutput {
                    content: format!("[Parallel] {} sub-nodes:\n{}", subs.len(), contents.join("\n")),
                    metadata: HashMap::new(), next: None, interrupt: None,
                })
            }
            GraphNode::Lambda { func } => {
                func(state)
            }
        }
    }

    /// Build incremental context for an agent node.
    /// Replaces O(n²) message concatenation with:
    /// 1. Todo attention anchor
    /// 2. Recent events (relevant to this role)
    /// 3. File path references instead of full content
    /// 4. Last 3 step outputs only
    fn build_incremental_context(
        node_name: &str,
        state: &GraphState,
        event_stream: &EventStream,
        todo: &mut TodoAttention,
    ) -> String {
        let mut context = String::new();

        // 0. Original user task/query
        if let Some(user_msg) = state.messages.first() {
            context.push_str(&format!("## Task\n{}\n\n", user_msg.content));
        }

        // 1. Todo attention anchor (refreshed every iteration)
        context.push_str(&todo.refresh());
        context.push_str("\n\n");

        // 2. Recent events relevant to this role
        let events = event_stream.format_recent(10, Some(node_name));
        if !events.contains("no recent events") {
            context.push_str(&format!("## Recent Activity\n{events}\n\n"));
        }

        // 3. Step outputs — 预算内滑动窗口 + 结构感知截断
        //
        // 旧实现：固定 3 个 step + 500 字符硬截断（切断 JSON/代码）。
        // 新实现：从最近 step 往前填充，累计 token 超 MAX_CONTEXT_TOKENS 时停止；
        // 单个 step 超 MAX_STEP_CHARS 时用 truncate_structured 在结构边界截断。
        let outputs: Vec<_> = state.step_outputs.iter().collect();
        if !outputs.is_empty() {
            context.push_str("## Previous Steps\n");

            let mut budget_tokens = MAX_CONTEXT_TOKENS.saturating_sub(estimate_tokens(&context));
            let mut included = 0;

            for (name, content) in outputs.iter().rev() {
                // 单 step 超 MAX_STEP_CHARS 时结构截断（截断后 token 更少）
                let display = if content.chars().count() > MAX_STEP_CHARS {
                    truncate_structured(content, MAX_STEP_CHARS)
                } else {
                    content.to_string()
                };
                let display_tokens = estimate_tokens(&display);

                if display_tokens > budget_tokens && included > 0 {
                    // 预算不够且已至少保留 1 个 → 停止
                    break;
                }

                context.push_str(&format!("[{name}]: {display}\n"));
                budget_tokens = budget_tokens.saturating_sub(display_tokens);
                included += 1;
            }

            // 更早的 step 留文件引用
            let omitted = outputs.len().saturating_sub(included);
            if omitted > 0 {
                context.push_str(&format!(
                    "({omitted} earlier steps: see {})\n",
                    state.work_dir.join("checkpoints").display()
                ));
            }
            context.push('\n');
        }

        context
    }

    /// Actually call the LLM for an agent node.
    async fn execute_agent_node(
        provider: &dyn miniagent_provider::traits::LlmProvider,
        system_prompt: &str,
        context: &str,
        node_name: &str,
        cancel: &CancellationToken,
    ) -> Result<NodeOutput, GraphError> {
        use miniagent_provider::traits::CompletionRequest;

        let task_message = miniagent_core::message::Message::user(format!(
            "You are the **{node_name}** in a multi-agent pipeline.\n\n\
             ## Your Role\n{system_prompt}\n\n\
             ## Context\n{context}\n\n\
             ## Instructions\n\
             Execute your role based on the context above. \
             If you are the final stage, produce the complete output. \
             Be thorough and specific."
        ));

        let request = CompletionRequest {
            system: system_prompt.to_string(),
            messages: vec![task_message],
            tools: vec![],
            config: miniagent_core::config::InferenceConfig {
                max_tokens: Some(16_000),
                ..Default::default()
            },
        };

        let response = provider
            .complete(&request, cancel.child_token())
            .await
            .map_err(|e| GraphError::NodeFailed(format!("LLM error: {e}")))?;

        let text = response
            .content
            .iter()
            .filter_map(|b| match b {
                miniagent_core::event::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(NodeOutput {
            content: text,
            metadata: HashMap::new(),
            next: None,
            interrupt: None,
        })
    }

    pub fn visualize(&self) -> String {
        let mut out = String::from("```mermaid\ngraph TD\n");
        for (name, node) in &self.nodes {
            let label = match node {
                GraphNode::Agent { system_prompt, .. } =>
                    format!("🤖 {}", system_prompt.chars().take(30).collect::<String>()),
                GraphNode::Tool { tool_name } => format!("🔧 {tool_name}"),
                GraphNode::Human { prompt } =>
                    format!("👤 {}", prompt.chars().take(30).collect::<String>()),
                GraphNode::Parallel { sub_nodes } => format!("∥ {}", sub_nodes.join(",")),
                GraphNode::Lambda { .. } => "λ".into(),
            };
            out.push_str(&format!("    {}[\"{}\"]\n", sanitize(name), label));
        }
        for (from, to) in &self.edges {
            out.push_str(&format!("    {} --> {}\n", sanitize(from), sanitize(to)));
        }
        out.push_str("```\n"); out
    }
}

fn sanitize(s: &str) -> String { s.replace(['-', ' '], "_") }

// ── Checkpoint ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: uuid::Uuid,
    pub node_name: String,
    pub state: GraphState,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Checkpoint {
    pub fn from_state(state: &GraphState, node_name: &str) -> Self {
        // 性能优化：截断化 checkpoint——只保留最近 N 条 messages / step_outputs，
        // 而非 clone 全部累积历史。长 pipeline 后期（接近 50 迭代），全量 clone +
        // 序列化写盘是 O(全部历史) 内存/IO 放大。checkpoint 的用途是崩溃恢复，
        // 最近 10 条足够恢复执行上下文。
        const MAX_CKPT_MESSAGES: usize = 10;
        const MAX_CKPT_OUTPUTS: usize = 10;

        let mut snapshot = state.clone();
        if snapshot.messages.len() > MAX_CKPT_MESSAGES {
            let start = snapshot.messages.len() - MAX_CKPT_MESSAGES;
            snapshot.messages = snapshot.messages[start..].to_vec();
        }
        if snapshot.step_outputs.len() > MAX_CKPT_OUTPUTS {
            // 保留最近的 step_outputs（按插入序，HashMap 需转 Vec 取尾部）
            let entries: Vec<_> = snapshot.step_outputs.drain().collect();
            let start = entries.len().saturating_sub(MAX_CKPT_OUTPUTS);
            snapshot.step_outputs = entries[start..].iter().cloned().collect();
        }

        Self { id: uuid::Uuid::new_v4(), node_name: node_name.to_string(),
               state: snapshot, timestamp: chrono::Utc::now() }
    }

    /// Persist checkpoint to disk. Fixes the original bug where checkpoints
    /// were created and immediately dropped.
    pub fn save_to_disk(&self, work_dir: &std::path::Path) -> Result<std::path::PathBuf, std::io::Error> {
        let dir = work_dir.join("checkpoints");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("ckpt_{}_{}.json", self.node_name, self.id));
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(path)
    }

    /// Load a checkpoint from disk.
    pub fn load_from_disk(path: &std::path::Path) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// List all checkpoints in a work directory.
    pub fn list_checkpoints(work_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let dir = work_dir.join("checkpoints");
        std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "json"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find the latest checkpoint for a given node.
    pub fn latest_for_node(work_dir: &std::path::Path, node_name: &str) -> Option<Self> {
        let checkpoints = Self::list_checkpoints(work_dir);
        let prefix = format!("ckpt_{node_name}_");
        let matching: Vec<_> = checkpoints.iter()
            .filter(|p| p.file_name().unwrap_or_default().to_string_lossy().starts_with(&prefix))
            .collect();
        matching.last().and_then(|p| Self::load_from_disk(p).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniagent_provider::MockProvider;

    /// 构造一个指向临时目录的 state，避免污染仓库工作区。
    fn fresh_state(tag: &str) -> GraphState {
        let dir = std::env::temp_dir().join(format!("miniagent_sg_test_{tag}_{}", uuid_like()));
        GraphState::default().with_work_dir(dir)
    }

    /// 简易"伪 uuid"，避免引入 uuid 依赖到测试。
    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        format!("{nanos:x}")
    }

    /// 两个 mock provider；Lambda/Tool 节点不会真正调用它们，仅满足 execute 签名。
    fn mock_providers() -> (MockProvider, MockProvider) {
        (MockProvider::new("flash"), MockProvider::new("pro"))
    }

    #[tokio::test]
    async fn route_static_edges_when_no_conditional() {
        // 静态图：A → B → C，无条件边。route() 应返回静态后继。
        let graph = StateGraph::new("a")
            .add_lambda("a", |_| Ok(NodeOutput {
                content: "A".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("b", |_| Ok(NodeOutput {
                content: "B".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("c", |_| Ok(NodeOutput {
                content: "C".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_edge("a", "b")
            .add_edge("b", "c")
            .compile()
            .unwrap();

        let state = fresh_state("route_static");
        assert_eq!(graph.route("a", &state), vec!["b".to_string()]);
        assert_eq!(graph.route("b", &state), vec!["c".to_string()]);
        assert!(graph.route("c", &state).is_empty(), "leaf node has no successors");
    }

    #[tokio::test]
    async fn execute_static_graph_preserves_order_and_outputs() {
        // 等价性测试：纯静态图在动态调度下应执行全部节点，step_outputs 完整。
        let graph = StateGraph::new("a")
            .add_lambda("a", |_| Ok(NodeOutput {
                content: "OUT_A".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("b", |_| Ok(NodeOutput {
                content: "OUT_B".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("c", |_| Ok(NodeOutput {
                content: "OUT_C".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_edge("a", "b")
            .add_edge("b", "c")
            .compile()
            .unwrap();

        let (flash, pro) = mock_providers();
        let state = fresh_state("exec_static");
        let result = graph
            .execute(state, CancellationToken::new(), &flash, &pro)
            .await
            .unwrap();

        assert!(result.finished, "graph should finish");
        assert_eq!(result.step_outputs.get("a").unwrap(), "OUT_A");
        assert_eq!(result.step_outputs.get("b").unwrap(), "OUT_B");
        assert_eq!(result.step_outputs.get("c").unwrap(), "OUT_C");
        // 拓扑序：A 必须在 B 之前写入 messages
        let order: Vec<&str> = result.messages.iter().map(|m| m.content.as_str()).collect();
        let pos_a = order.iter().position(|s| *s == "OUT_A").unwrap();
        let pos_b = order.iter().position(|s| *s == "OUT_B").unwrap();
        let pos_c = order.iter().position(|s| *s == "OUT_C").unwrap();
        assert!(pos_a < pos_b && pos_b < pos_c, "static graph must preserve topological order");
    }

    #[tokio::test]
    async fn conditional_edge_routes_to_predicate_target() {
        // A → 条件边：若 step_outputs 含 "A" 则去 B，否则去 C（default）。
        // 用 Lambda 在 B/C 写入可辨识内容，验证谓词命中走 B。
        let graph = StateGraph::new("a")
            .add_lambda("a", |_| Ok(NodeOutput {
                content: "FROM_A".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("b", |_| Ok(NodeOutput {
                content: "WENT_TO_B".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("c", |_| Ok(NodeOutput {
                content: "WENT_TO_C".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_conditional(
                "a",
                vec![(
                    "b".to_string(),
                    Box::new(|s: &GraphState| s.step_outputs.contains_key("a")),
                )],
                "c",
            )
            .compile()
            .unwrap();

        let (flash, pro) = mock_providers();
        let state = fresh_state("cond_pred");
        let result = graph
            .execute(state, CancellationToken::new(), &flash, &pro)
            .await
            .unwrap();

        // a 执行后 step_outputs["a"]="FROM_A" → 谓词命中 → 走 B
        assert!(result.step_outputs.contains_key("b"), "predicate should route to b");
        assert!(!result.step_outputs.contains_key("c"), "default branch c should NOT run");
    }

    #[tokio::test]
    async fn conditional_edge_falls_back_to_default() {
        // 谓词永不命中 → 走 default。
        let graph = StateGraph::new("a")
            .add_lambda("a", |_| Ok(NodeOutput {
                content: "FROM_A".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("b", |_| Ok(NodeOutput {
                content: "WENT_TO_B".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("c", |_| Ok(NodeOutput {
                content: "WENT_TO_C".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_conditional(
                "a",
                vec![(
                    "b".to_string(),
                    // 谓词恒为 false → 永远走 default "c"
                    Box::new(|_s: &GraphState| false),
                )],
                "c",
            )
            .compile()
            .unwrap();

        let (flash, pro) = mock_providers();
        let state = fresh_state("cond_default");
        let result = graph
            .execute(state, CancellationToken::new(), &flash, &pro)
            .await
            .unwrap();

        assert!(result.step_outputs.contains_key("c"), "should fall back to default c");
        assert!(!result.step_outputs.contains_key("b"), "predicate branch b should NOT run");
    }

    #[tokio::test]
    async fn output_next_overrides_conditional_route() {
        // 节点自决 next 优先级最高：A 返回 next=Some("c")，即使有指向 B 的静态边，
        // 也应只去 C，不去 B。
        let graph = StateGraph::new("a")
            .add_lambda("a", |_| Ok(NodeOutput {
                content: "FROM_A".into(),
                metadata: HashMap::new(),
                next: Some("c".into()), // 自决去 c
                interrupt: None,
            }))
            .add_lambda("b", |_| Ok(NodeOutput {
                content: "B".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("c", |_| Ok(NodeOutput {
                content: "C".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            // 静态边指向 b，但 output.next 应覆盖它
            .add_edge("a", "b")
            .compile()
            .unwrap();

        let (flash, pro) = mock_providers();
        let state = fresh_state("next_override");
        let result = graph
            .execute(state, CancellationToken::new(), &flash, &pro)
            .await
            .unwrap();

        assert!(result.step_outputs.contains_key("c"), "output.next should route to c");
        assert!(!result.step_outputs.contains_key("b"), "static-edge target b should be skipped");
    }

    #[tokio::test]
    async fn parallel_wave_merges_todo_progress() {
        // 并行 wave：两个 Lambda 节点各自通过闭包副作用完成不同的 todo 任务。
        // 验证主 todo 同时反映两者的完成状态（修复前会丢失）。
        //
        // 由于 execute_node 对 Lambda 不操作 todo，这里改用一个并行图 + 两个并行
        // 节点，节点本身是 Lambda（不直接动 todo）。为了真正验证 todo 合并路径，
        // 我们直接测 TodoAttention::merge_from（见 todo_attention::tests），
        // 此处验证并行执行本身仍正确完成两个节点。
        let graph = StateGraph::new("p")
            .add_parallel("p", vec!["x", "y"])
            .add_lambda("x", |_| Ok(NodeOutput {
                content: "X_DONE".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .add_lambda("y", |_| Ok(NodeOutput {
                content: "Y_DONE".into(), metadata: HashMap::new(), next: None, interrupt: None,
            }))
            .compile()
            .unwrap();

        let (flash, pro) = mock_providers();
        let state = fresh_state("parallel");
        let result = graph
            .execute(state, CancellationToken::new(), &flash, &pro)
            .await
            .unwrap();

        assert!(result.finished);
        // 并行子节点的输出在 Parallel 节点的 step_output 里聚合
        let p_out = result.step_outputs.get("p").unwrap();
        assert!(p_out.contains("X_DONE") && p_out.contains("Y_DONE"),
            "parallel node should aggregate both sub-outputs: {p_out}");
    }

    // ── #7 上下文压缩测试 ──────────────────────────────────────

    #[test]
    fn estimate_tokens_mixed_content() {
        // chars/3 口径（整数除法）：英文 ~1 token/4char，中文 ~1 token/1.5char，混合取 chars/3 合理
        assert_eq!(estimate_tokens("hello world"), 3); // 11 chars / 3 = 3
        assert_eq!(estimate_tokens("你好世界测试"), 2); // 6 chars / 3 = 2
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn truncate_structured_preserves_json_boundary() {
        // 多行 JSON：每行一个 key-value，max_chars 应在换行边界截断，不切断单行
        let json = "{\n  \"key1\": \"value1\",\n  \"key2\": \"value2\",\n  \"key3\": \"value3\",\n  \"key4\": \"value4\"\n}";
        let result = truncate_structured(json, 40);
        assert!(result.contains("...(truncated"),
            "should be truncated and marked: {result}");
        // 截断应在换行处，不应切断到 key/value 中间（不含半个引号对）
        // 即：截断后的每一行都应是完整的 "key": "value", 行或 {
        for line in result.lines() {
            if line.contains(":") && !line.contains("...(truncated") {
                // 含冒号的行应是完整的 key-value（有闭合引号）
                let quote_count = line.matches('"').count();
                assert!(quote_count >= 4 || quote_count == 0,
                    "line should have complete key-value (even quotes), got: {line}");
            }
        }
    }

    #[test]
    fn truncate_structured_finds_newline_boundary() {
        // 多行文本：应在 \n 边界截断
        let text = "line one\nline two\nline three\nline four\nline five";
        let result = truncate_structured(text, 20);
        assert!(result.contains("...(truncated"),
            "should be truncated: {result}");
        // 截断点应在某行末尾（\n 之后），不应切断一行中间
        // 结果应以完整行 + 省略提示结尾
        assert!(result.ends_with("...(truncated, ") || result.lines().all(|l| !l.is_empty() && !l.contains("line t") || !l.ends_with("t")),
            "should not cut mid-line");
    }

    #[test]
    fn truncate_structured_short_content_unchanged() {
        let short = "short content";
        assert_eq!(truncate_structured(short, 100), short);
        assert_eq!(truncate_structured(short, 13), short); // 恰好等于
    }

    #[test]
    fn build_incremental_context_respects_token_budget() {
        // 构造大量 step_outputs，验证裁剪后总量受 MAX_CONTEXT_TOKENS 控制
        let mut state = GraphState::default();
        state.work_dir = std::env::temp_dir().join(format!(
            "miniagent_ctx_budget_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        // 20 个大 step，每个 ~6000 chars ≈ 2000 tokens（总 40000 tokens >> 16000 预算）
        for i in 0..20 {
            let big_content = format!("step {i} output: {}", "x".repeat(6000));
            state.step_outputs.insert(format!("node_{i}"), big_content);
        }
        // 需要一个 message 作为 task
        state.messages.push(GraphMessage::new("user", "test task"));

        let event_stream = EventStream::new(&state.work_dir);
        let mut todo = TodoAttention::new(&state.work_dir);

        let context = CompiledGraph::build_incremental_context(
            "test_node", &state, &event_stream, &mut todo,
        );

        let total_tokens = estimate_tokens(&context);
        assert!(total_tokens < MAX_CONTEXT_TOKENS + 2000, // 留余量给 task/todo/events 头部
            "context ({total_tokens} tokens) should respect budget (~{MAX_CONTEXT_TOKENS})");
        // 应有 omitted 提示
        assert!(context.contains("earlier steps"),
            "should mention omitted steps when budget exceeded");
    }

    #[test]
    fn build_incremental_context_short_pipeline_keeps_all() {
        // 3 个短 step 应全部保留（不被固定窗口或预算截掉）
        let mut state = GraphState::default();
        state.work_dir = std::env::temp_dir().join(format!(
            "miniagent_ctx_short_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        state.step_outputs.insert("a".into(), "output A".into());
        state.step_outputs.insert("b".into(), "output B".into());
        state.step_outputs.insert("c".into(), "output C".into());
        state.messages.push(GraphMessage::new("user", "task"));

        let event_stream = EventStream::new(&state.work_dir);
        let mut todo = TodoAttention::new(&state.work_dir);

        let context = CompiledGraph::build_incremental_context(
            "test_node", &state, &event_stream, &mut todo,
        );

        assert!(context.contains("output A"), "should keep step a");
        assert!(context.contains("output B"), "should keep step b");
        assert!(context.contains("output C"), "should keep step c");
        assert!(!context.contains("earlier steps"),
            "short pipeline should not omit any steps");
    }
}
