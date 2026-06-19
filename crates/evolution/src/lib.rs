pub mod memory_router;
pub mod cold_start_kb;
pub mod selection_engine;
pub mod decoupled_executor;
pub mod search_scheduler;

pub use memory_router::{MemoryRouter, RetrievalContext, ExperienceSummary};
pub use cold_start_kb::{ColdStartKnowledgeBase, DomainTemplate};
pub use selection_engine::{CandidatePlan, CandidateSource, MutationOp, SelectionEngine};
pub use decoupled_executor::{EscalationContext, TacticResult};
pub use search_scheduler::{EliteEntry, SearchScheduler, SearchStrategy};

// ── Memory Retriever Trait ─────────────────────────────────────
// Defined at crate root for reliable cross-crate visibility.

use std::future::Future;
use std::pin::Pin;

pub trait MemoryRetriever: Send + Sync {
    fn retrieve<'a>(&'a self, task: &'a str) -> Pin<Box<dyn Future<Output = RetrievalContext> + Send + 'a>>;
    fn record(&self, task: &str, success: bool, quality_score: f64);
}

// Blanket impl for Arc<dyn MemoryRetriever>
impl<T: MemoryRetriever> MemoryRetriever for std::sync::Arc<T> {
    fn retrieve<'a>(&'a self, task: &'a str) -> Pin<Box<dyn Future<Output = RetrievalContext> + Send + 'a>> {
        Box::pin(async move { (**self).retrieve(task).await })
    }
    fn record(&self, task: &str, success: bool, quality_score: f64) {
        (**self).record(task, success, quality_score);
    }
}


