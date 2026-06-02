mod read;
mod write;
mod edit;
mod glob;
mod grep;
mod bash;
mod web_fetch;
mod web_search;
mod pubmed;

pub use read::ReadTool;
pub use write::WriteTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use bash::BashTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use pubmed::PubMedTool;

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
        .register(PubMedTool::new());
    registry
}
