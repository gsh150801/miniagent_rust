mod read;
mod write;
mod edit;
mod glob;
mod grep;
mod bash;
mod web_fetch;
mod web_search;
mod pubmed;
mod git_tool;
mod conda_tool;
mod patent_search;
mod clinical_trials;
mod kg_tools;

pub use read::ReadTool;
pub use write::WriteTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use bash::BashTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use pubmed::PubMedTool;
pub use git_tool::GitTool;
pub use conda_tool::CondaTool;
pub use patent_search::PatentSearchTool;
pub use clinical_trials::ClinicalTrialsTool;
pub use kg_tools::{KgHandle, KgQueryTool, KgAddTool, HypothesisSuggestTool};

use crate::registry::ToolRegistry;

pub fn defaults() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(ReadTool::new())
        .register(WriteTool::new())
        .register(EditTool::new())
        .register(GlobTool::new())
        .register(GrepTool::new())
        .register(BashTool::new())
        .register(WebFetchTool::new())
        .register(WebSearchTool::new())
        .register(PubMedTool::new())
        .register(GitTool::new())
        .register(CondaTool::new())
        .register(PatentSearchTool::new())
        .register(ClinicalTrialsTool::new());
    registry
}

/// 构造带知识图谱能力的默认工具集。
///
/// `handle` 在所有 KG 工具间共享同一份 `KnowledgeGraph`。
/// 若 `provider` 为 `Some`，`hypothesis_suggest` 会进一步用 LLM 生成
/// 完整假设；否则仅返回基于 link prediction 的结构化候选。
pub fn defaults_with_kg(
    handle: KgHandle,
    provider: Option<std::sync::Arc<dyn miniagent_provider::traits::LlmProvider>>,
) -> ToolRegistry {
    let mut registry = defaults();
    registry.register(KgQueryTool::new(handle.clone()));
    registry.register(KgAddTool::new(handle.clone()));
    registry.register(match provider {
        Some(p) => HypothesisSuggestTool::with_provider(handle, p),
        None => HypothesisSuggestTool::new(handle),
    });
    registry
}
