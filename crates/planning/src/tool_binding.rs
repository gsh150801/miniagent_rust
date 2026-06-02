use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ── IO Type System ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IoType {
    SearchQuery,    // "CRISPR off-target detection"
    PaperList,      // [{pmid, title, year}]
    AbstractText,   // plain text abstract
    FullText,       // full paper text
    Entities,       // [{name, type}]
    Relations,      // [{from, to, type}]
    KnowledgeGraph, // full KG
    DataFrame,      // tabular data
    Hypothesis,     // text hypothesis
    ExperimentDesign,
    CodeScript,     // executable code
    FilePath,       // path on disk
    Report,         // markdown/latex report
    Any,
}

// ── Tool Descriptor ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub category: ToolCategory,
    pub input_type: IoType,
    pub output_type: IoType,
    pub safety: SafetyLevel,
    pub cost_estimate_ms: u64,
    pub fallback: Option<String>,
    pub depends_on: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCategory {
    Literature,       // pubmed_search, web_search
    DataRetrieval,    // web_fetch, read
    DataAnalysis,     // python, bash (stats)
    CodeGeneration,   // write, edit
    FileSystem,       // read, write, glob, grep, bash
    Visualization,    // matplotlib, plot
    Experiment,       // lab equipment
    Communication,    // agent-to-agent
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyLevel {
    ReadOnly,
    Mutating,
    NetworkAccess,
    Sandboxed,
}

// ── Tool Registry ──────────────────────────────────────────────

pub struct ToolRegistry {
    tools: HashMap<String, ToolDescriptor>,
    /// Forward edges: tool_A.output_type matches tool_B.input_type
    compat_graph: HashMap<String, Vec<String>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new(), compat_graph: HashMap::new() }
    }

    pub fn register(&mut self, tool: ToolDescriptor) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Build compatibility graph: tool_A → tool_B if output_type matches input_type
    pub fn build_compat_graph(&mut self) {
        self.compat_graph.clear();
        let names: Vec<String> = self.tools.keys().cloned().collect();

        for a_name in &names {
            let a_out = self.tools[a_name].output_type.clone();
            let mut compat = Vec::new();
            for b_name in &names {
                if a_name == b_name { continue; }
                let b_in = self.tools[b_name].input_type.clone();
                if a_out == b_in || b_in == IoType::Any || a_out == IoType::Any {
                    compat.push(b_name.clone());
                }
            }
            self.compat_graph.insert(a_name.clone(), compat);
        }
    }

    /// Find all tools in a given category
    pub fn by_category(&self, category: ToolCategory) -> Vec<&ToolDescriptor> {
        self.tools.values().filter(|t| t.category == category).collect()
    }

    /// Find tool chain: from start_type to end_type, BFS shortest path
    pub fn find_chain(&self, from: &IoType, to: &IoType, max_depth: usize) -> Option<Vec<String>> {
        use std::collections::{VecDeque, HashSet};

        // Find start tools
        let starts: Vec<String> = self.tools.iter()
            .filter(|(_, t)| t.input_type == *from || t.input_type == IoType::Any)
            .map(|(n, _)| n.clone()).collect();

        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        let mut parent: HashMap<String, String> = HashMap::new();

        for start in starts {
            queue.push_back(start.clone());
            visited.insert(start.clone());
        }

        while let Some(current) = queue.pop_front() {
            let tool = &self.tools[current.as_str()];
            if tool.output_type == *to || tool.output_type == IoType::Any {
                // Reconstruct path
                let mut path = vec![current.clone()];
                let mut node = current.clone();
                while let Some(p) = parent.get(node.as_str()) {
                    path.push(p.clone());
                    node = p.clone();
                }
                path.reverse();
                return if path.len() <= max_depth { Some(path) } else { None };
            }

            if let Some(neighbors) = self.compat_graph.get(current.as_str()) {
                for next in neighbors {
                    if !visited.contains(next) {
                        visited.insert(next.clone());
                        parent.insert(next.clone(), current.clone());
                        queue.push_back(next.clone());
                    }
                }
            }
        }
        None
    }

    /// Find fallback chain if a tool fails
    pub fn find_fallback_chain(&self, tool_name: &str, _target_output: &IoType) -> Vec<String> {
        let tool = match self.tools.get(tool_name) {
            Some(t) => t,
            None => return vec![],
        };

        // Try explicit fallback first
        if let Some(ref fallback) = tool.fallback
            && self.tools.contains_key(fallback) {
                return vec![fallback.clone()];
            }

        // Try same-category alternatives
        self.by_category(tool.category).iter()
            .filter(|t| t.name != tool_name && t.output_type == tool.output_type)
            .map(|t| t.name.clone())
            .take(3)
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&ToolDescriptor> { self.tools.get(name) }

    pub fn len(&self) -> usize { self.tools.len() }
    pub fn is_empty(&self) -> bool { self.tools.is_empty() }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}

// ── Built-in Registry ──────────────────────────────────────────

pub fn default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    let tools = vec![
        ToolDescriptor { name: "pubmed_search".into(), category: ToolCategory::Literature,
            input_type: IoType::SearchQuery, output_type: IoType::PaperList,
            safety: SafetyLevel::ReadOnly, cost_estimate_ms: 500,
            fallback: Some("web_search".into()), depends_on: vec![],
            description: "Search PubMed for biomedical papers".into() },
        ToolDescriptor { name: "web_search".into(), category: ToolCategory::Literature,
            input_type: IoType::SearchQuery, output_type: IoType::PaperList,
            safety: SafetyLevel::NetworkAccess, cost_estimate_ms: 300,
            fallback: None, depends_on: vec![],
            description: "Search the web via Serper/Tavily".into() },
        ToolDescriptor { name: "web_fetch".into(), category: ToolCategory::DataRetrieval,
            input_type: IoType::PaperList, output_type: IoType::AbstractText,
            safety: SafetyLevel::NetworkAccess, cost_estimate_ms: 1000,
            fallback: None, depends_on: vec![],
            description: "Fetch paper abstracts from URLs".into() },
        ToolDescriptor { name: "read".into(), category: ToolCategory::FileSystem,
            input_type: IoType::FilePath, output_type: IoType::Any,
            safety: SafetyLevel::ReadOnly, cost_estimate_ms: 5,
            fallback: None, depends_on: vec![],
            description: "Read file contents".into() },
        ToolDescriptor { name: "write".into(), category: ToolCategory::CodeGeneration,
            input_type: IoType::Any, output_type: IoType::FilePath,
            safety: SafetyLevel::Mutating, cost_estimate_ms: 10,
            fallback: None, depends_on: vec![],
            description: "Write content to file".into() },
        ToolDescriptor { name: "bash".into(), category: ToolCategory::DataAnalysis,
            input_type: IoType::CodeScript, output_type: IoType::Any,
            safety: SafetyLevel::Sandboxed, cost_estimate_ms: 5000,
            fallback: None, depends_on: vec![],
            description: "Execute shell commands".into() },
        ToolDescriptor { name: "python".into(), category: ToolCategory::DataAnalysis,
            input_type: IoType::DataFrame, output_type: IoType::Any,
            safety: SafetyLevel::Sandboxed, cost_estimate_ms: 3000,
            fallback: Some("bash".into()), depends_on: vec![],
            description: "Run Python data analysis".into() },
    ];

    for tool in tools { reg.register(tool); }
    reg.build_compat_graph();
    reg
}
