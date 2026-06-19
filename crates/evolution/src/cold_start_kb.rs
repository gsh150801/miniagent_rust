use serde::{Deserialize, Serialize};

/// Cold-start knowledge base: pre-loaded domain templates for common task types.
///
/// This solves the "cold start" problem in MLEvolve's Retrospective Memory:
/// when no historical experiences exist yet, the system still has useful priors
/// about which tools, roles, and strategies work for each task category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdStartKnowledgeBase {
    pub entries: Vec<DomainTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainTemplate {
    /// Task type identifier (matches keyword matching).
    pub task_type: String,
    /// Human-readable description of this domain.
    pub description: String,
    /// Tools that historically have high success rates for this domain.
    pub typical_tools: Vec<String>,
    /// Tools that tend to fail for this domain (to avoid or use carefully).
    pub avoid_tools: Vec<String>,
    /// Recommended agent roles for this domain, in priority order.
    pub typical_roles: Vec<String>,
    /// Typical number of parallel waves for this domain.
    pub typical_waves: usize,
    /// Keywords that trigger this template.
    pub keywords: Vec<String>,
    /// Suggested exploration focus.
    pub exploration_focus: String,
}

impl ColdStartKnowledgeBase {
    pub fn new(entries: Vec<DomainTemplate>) -> Self {
        Self { entries }
    }

    /// Create with the default set of domain templates covering the most
    /// common Loop Pipeline task types.
    pub fn with_defaults() -> Self {
        Self::new(vec![
            DomainTemplate::code_generation(),
            DomainTemplate::research(),
            DomainTemplate::report_writing(),
            DomainTemplate::data_analysis(),
            DomainTemplate::general_qna(),
        ])
    }

    /// Match a task description to the best domain template.
    ///
    /// Uses keyword scoring with **specificity-weighted** tie-breaking:
    /// longer/more-specific keywords count more than generic ones like "write".
    pub fn match_task(&self, task: &str) -> Option<&DomainTemplate> {
        let task_lower = task.to_lowercase();

        let mut best: Option<(&DomainTemplate, f64)> = None;

        for template in &self.entries {
            // Weighted score: count keyword matches, but penalize generic keywords
            let score: f64 = template.keywords.iter()
                .filter_map(|kw| {
                    if task_lower.contains(kw.as_str()) {
                        // Specificity weight: longer keywords are more discriminative
                        let weight = if kw.len() >= 8 { 2.0 }
                            else if kw.len() >= 5 { 1.5 }
                            else { 1.0 };
                        Some(weight)
                    } else {
                        None
                    }
                })
                .sum();

            if score > 0.0 {
                // Use >= so ties prefer LATER templates (more specific ones like report_writing
                // come after generic ones like code_generation)
                if best.is_none() || score > best.unwrap().1 {
                    best = Some((template, score));
                }
            }
        }

        best.map(|(t, _)| t)
    }

    /// Get all templates (for debugging / inspection).
    pub fn all(&self) -> &[DomainTemplate] {
        &self.entries
    }
}

impl DomainTemplate {
    /// Code generation tasks: implement functions, write scripts, build projects.
    pub fn code_generation() -> Self {
        Self {
            task_type: "code_generation".into(),
            description: "Writing, editing, or executing code in any language".into(),
            typical_tools: vec![
                "bash".into(),
                "write".into(),
                "edit".into(),
                "read".into(),
                "glob".into(),
                "grep".into(),
                "git".into(),
            ],
            avoid_tools: vec![
                "web_search".into(),
                "web_fetch".into(),
            ],
            typical_roles: vec!["executor".into(), "writer".into(), "critic".into()],
            typical_waves: 2,
            keywords: vec![
                "code".into(), "implement".into(), "function".into(), "script".into(),
                "python".into(), "rust".into(), "javascript".into(), "typescript".into(),
                "program".into(), "class".into(), "module".into(),
                "write".into(), "create".into(), "build".into(), "develop".into(),
            ],
            exploration_focus: "Focus on existing codebase structure, dependencies, and required interfaces before coding.".into(),
        }
    }

    /// Research tasks: search the web, gather information, synthesize findings.
    pub fn research() -> Self {
        Self {
            task_type: "research".into(),
            description: "Web research, literature review, information gathering".into(),
            typical_tools: vec![
                "web_search".into(),
                "web_fetch".into(),
                "pubmed_search".into(),
                "patent_search".into(),
                "clinical_trials_search".into(),
                "read".into(),
            ],
            avoid_tools: vec![
                "bash".into(),
                "write".into(),
                "edit".into(),
            ],
            typical_roles: vec!["researcher".into(), "analyst".into(), "synthesizer".into(), "writer".into()],
            typical_waves: 3,
            keywords: vec![
                "research".into(), "find".into(), "search".into(), "investigate".into(),
                "analyze".into(), "literature".into(), "papers".into(), "study".into(),
                "survey".into(), "review".into(), "compare".into(), "trends".into(),
            ],
            exploration_focus: "Cast a wide net across multiple sources, verify claims with primary sources, track URLs.".into(),
        }
    }

    /// Report / document writing tasks.
    pub fn report_writing() -> Self {
        Self {
            task_type: "report_writing".into(),
            description: "Writing reports, documents, summaries, or articles".into(),
            typical_tools: vec![
                "read".into(),
                "write".into(),
                "edit".into(),
                "web_search".into(),
            ],
            avoid_tools: vec![
                "bash".into(),
                "grep".into(),
                "glob".into(),
            ],
            typical_roles: vec!["researcher".into(), "writer".into(), "critic".into(), "synthesizer".into()],
            typical_waves: 2,
            keywords: vec![
                "write".into(), "report".into(), "document".into(), "summarize".into(),
                "article".into(), "paper".into(), "essay".into(), "blog".into(),
                "draft".into(), "compose".into(),
            ],
            exploration_focus: "Gather comprehensive source material first, then structure the report with clear sections and citations.".into(),
        }
    }

    /// Data analysis tasks: process data, generate charts, compute statistics.
    pub fn data_analysis() -> Self {
        Self {
            task_type: "data_analysis".into(),
            description: "Data processing, statistical analysis, visualization".into(),
            typical_tools: vec![
                "bash".into(),
                "read".into(),
                "write".into(),
                "edit".into(),
                "glob".into(),
                "grep".into(),
            ],
            avoid_tools: vec![
                "web_search".into(),
                "web_fetch".into(),
            ],
            typical_roles: vec!["executor".into(), "analyst".into(), "writer".into()],
            typical_waves: 2,
            keywords: vec![
                "data".into(), "analyze".into(), "analysis".into(), "statistics".into(),
                "chart".into(), "graph".into(), "visualization".into(), "csv".into(),
                "dataset".into(), "metrics".into(), "process".into(),
            ],
            exploration_focus: "Understand data format and schema first, then identify the right processing tools.".into(),
        }
    }

    /// General Q&A / simple tasks that don't fit other categories.
    pub fn general_qna() -> Self {
        Self {
            task_type: "general_qna".into(),
            description: "General questions, simple tasks, or mixed-type requests".into(),
            typical_tools: vec![
                "web_search".into(),
                "web_fetch".into(),
                "read".into(),
                "write".into(),
            ],
            avoid_tools: vec![],
            typical_roles: vec!["researcher".into(), "executor".into(), "writer".into()],
            typical_waves: 1,
            keywords: vec![
                "help".into(), "what".into(), "how".into(), "explain".into(),
                "describe".into(), "summarize".into(), "list".into(), "show".into(),
            ],
            exploration_focus: "Understand the exact intent, then use the minimum set of tools to answer efficiently.".into(),
        }
    }
}

impl Default for ColdStartKnowledgeBase {
    fn default() -> Self {
        Self::with_defaults()
    }
}
