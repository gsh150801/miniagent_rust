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
mod ask_user;
mod notebook_edit;
mod geo_search;
mod opentargets;
mod enrichr;
mod uniprot;
mod citation_check;

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
pub use geo_search::GeoSearchTool;
pub use opentargets::OpenTargetsTool;
pub use enrichr::EnrichrTool;
pub use uniprot::UniprotTool;
pub use citation_check::CitationCheckTool;

use crate::registry::ToolRegistry;
use ask_user::AskUserTool;
use notebook_edit::NotebookEditTool;

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
        .register(ClinicalTrialsTool::new())
        .register(AskUserTool::new())
        .register(NotebookEditTool::new())
        .register(GeoSearchTool::new())
        .register(OpenTargetsTool::new())
        .register(EnrichrTool::new())
        .register(UniprotTool::new())
        .register(CitationCheckTool::new());
    registry
}


