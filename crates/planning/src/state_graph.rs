use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event_stream::EventStream;
use crate::todo_attention::TodoAttention;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelTier { Flash, Pro }

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
    node_order: Vec<Vec<String>>,
    nodes: HashMap<String, GraphNode>,
    edges: Vec<(String, String)>,
    #[allow(dead_code)]
    conditional_edges: Vec<ConditionalEdge>,
    checkpoints: HashSet<String>,
}

impl CompiledGraph {
    /// Get the execution wave order (for testing and visualization).
    pub fn waves(&self) -> &[Vec<String>] {
        &self.node_order
    }

    #[allow(dead_code)]
    fn route(&self, from: &str, state: &GraphState) -> Vec<String> {
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
    pub async fn execute(
        &self,
        mut state: GraphState,
        cancel: CancellationToken,
        flash: &dyn miniagent_provider::traits::LlmProvider,
        pro: &dyn miniagent_provider::traits::LlmProvider,
    ) -> Result<GraphState, GraphError> {
        let mut event_stream = EventStream::new(&state.work_dir);
        let mut todo = TodoAttention::new(&state.work_dir);

        for (wave_idx, wave) in self.node_order.iter().enumerate() {
            if cancel.is_cancelled() { return Err(GraphError::Cancelled); }
            if state.iteration >= state.budget.max_iterations { return Err(GraphError::BudgetExhausted); }

            // Execute all nodes in this wave in parallel (if multiple)
            let results = if wave.len() == 1 {
                // Single node — no need for tokio overhead
                let node_name = &wave[0];
                state.current_node = node_name.clone();
                event_stream.task_started(node_name, &format!("wave {wave_idx}"));

                let result = self.execute_node(
                    node_name, &state, &cancel, flash, pro, &mut event_stream, &mut todo,
                ).await;

                vec![(node_name.clone(), result)]
            } else {
                // Multiple nodes — execute in parallel with true concurrency
                event_stream.push(crate::event_stream::AgentEvent {
                    timestamp: chrono::Utc::now(),
                    agent: "graph".into(),
                    kind: crate::event_stream::EventKind::IterationStarted,
                    details: format!("wave {wave_idx}: {} nodes in parallel", wave.len()),
                    file_refs: vec![],
                    success: true,
                });

                // Spawn each node with its own cloned state and event stream
                #[allow(clippy::type_complexity)]
                let mut futures: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = (
                    String, Result<NodeOutput, GraphError>, EventStream,
                )> + '_>>> = Vec::new();

                for node_name in wave {
                    if cancel.is_cancelled() { break; }
                    let node_name = node_name.clone();
                    let mut node_state = state.clone();
                    node_state.current_node = node_name.clone();
                    let cancel_token = cancel.child_token();
                    let mut node_events = event_stream.clone();
                    let mut node_todo = todo.clone();

                    node_events.task_started(&node_name, &format!("wave {wave_idx}"));

                    futures.push(Box::pin(async move {
                        let result = self.execute_node(
                            &node_name, &node_state, &cancel_token,
                            flash, pro, &mut node_events, &mut node_todo,
                        ).await;
                        (node_name, result, node_events)
                    }));
                }

                // Run all nodes concurrently and collect results
                let raw_results = futures_util::future::join_all(futures).await;

                // Merge per-node events back into the main stream
                let mut results = Vec::new();
                for (name, result, node_events) in raw_results {
                    for ev in node_events.iter() {
                        event_stream.push(ev.clone());
                    }
                    results.push((name, result));
                }
                results
            };

            // Process results
            for (node_name, result) in results {
                state.iteration += 1;

                match result {
                    Ok(output) => {
                        state.step_outputs.insert(node_name.clone(), output.content.clone());
                        state.messages.push(GraphMessage::new(&node_name, &output.content));
                        event_stream.task_completed(&node_name, &output.content, vec![]);
                    }
                    Err(ref e) => {
                        // Preserve error in context (Manus principle: never hide failures)
                        let error_msg = format!("[ERROR:{node_name}] {e}");
                        state.step_outputs.insert(node_name.clone(), error_msg.clone());
                        state.messages.push(GraphMessage::new(&node_name, &error_msg));
                        event_stream.task_failed(&node_name, &e.to_string());
                    }
                }

                // Checkpoint: persist to disk if this is a checkpoint node
                if self.checkpoints.contains(&node_name) {
                    let ckpt = Checkpoint::from_state(&state, &node_name);
                    if let Ok(path) = ckpt.save_to_disk(&state.work_dir) {
                        event_stream.checkpoint_saved(&node_name, &path.to_string_lossy());
                    }
                }
            }

            // Refresh todo attention anchor
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
                eprintln!("\n[HITL] {prompt}");
                eprintln!("   (auto-approved in CLI mode)");
                Ok(NodeOutput {
                    content: format!("approved: {prompt}"),
                    metadata: HashMap::new(), next: None,
                    interrupt: Some(prompt.clone()),
                })
            }
            GraphNode::Parallel { sub_nodes } => {
                // Execute sub-nodes with true concurrency
                let subs = sub_nodes.clone();
                #[allow(clippy::type_complexity)]
                let mut sub_futures: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = (
                    String, Result<NodeOutput, GraphError>, EventStream,
                )> + '_>>> = Vec::new();

                for sub_name in &subs {
                    if cancel.is_cancelled() { break; }
                    let sub_name = sub_name.clone();
                    let sub_state = state.clone();
                    let cancel_token = cancel.child_token();
                    let mut sub_events = event_stream.clone();
                    let mut sub_todo = todo.clone();

                    sub_futures.push(Box::pin(async move {
                        let result = self.execute_node(
                            &sub_name, &sub_state, &cancel_token,
                            flash, pro, &mut sub_events, &mut sub_todo,
                        ).await;
                        (sub_name, result, sub_events)
                    }));
                }

                let raw_results = futures_util::future::join_all(sub_futures).await;

                // Merge events and collect output
                let mut contents = Vec::new();
                for (sub_name, result, sub_events) in raw_results {
                    for ev in sub_events.iter() {
                        event_stream.push(ev.clone());
                    }
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

        // 1. Todo attention anchor (refreshed every iteration)
        context.push_str(&todo.refresh());
        context.push_str("\n\n");

        // 2. Recent events relevant to this role
        let events = event_stream.format_recent(10, Some(node_name));
        if !events.contains("no recent events") {
            context.push_str(&format!("## Recent Activity\n{events}\n\n"));
        }

        // 3. Step outputs — last 3 only, older ones as file references
        let outputs: Vec<_> = state.step_outputs.iter().collect();
        if !outputs.is_empty() {
            context.push_str("## Previous Steps\n");

            // Older outputs: reference only
            if outputs.len() > 3 {
                context.push_str(&format!("({} earlier steps: see {}/)\n",
                    outputs.len() - 3,
                    state.work_dir.join("checkpoints").display()));
            }

            // Last 3 outputs: include content (bounded)
            for (name, content) in outputs.iter().rev().take(3) {
                let content: &str = content.as_str();
                let truncated: String = if content.len() > 500 {
                    format!("{}...(see {name}/last_output.json)", &content[..500])
                } else {
                    content.to_string()
                };
                context.push_str(&format!("[{name}]: {truncated}\n"));
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
        Self { id: uuid::Uuid::new_v4(), node_name: node_name.to_string(),
               state: state.clone(), timestamp: chrono::Utc::now() }
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
