pub mod debate;
pub mod generator;
pub mod ranking;
pub mod validation;

pub use debate::{
    persist_debate_report, ContradictionPair, CrossComparison, DebateOutcome, HypothesisDebater,
    HypothesisVerdict, Verdict,
};
pub use generator::{ExperimentDesign, Hypothesis, HypothesisGenerator, HypothesisNovelty};
pub use ranking::HypothesisRanker;
pub use validation::{AnalysisVariables, DataAnalysisTask, DatasetSource, ValidationPlan, WetLabProtocol};
