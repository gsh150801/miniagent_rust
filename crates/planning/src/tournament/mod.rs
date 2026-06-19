pub mod elo;
pub mod debate;
pub mod arena;
pub mod convergence;

pub use elo::{EloEngine, PlayerRating, MatchOutcome};
pub use debate::{DebateRubricScores, DebateSession, Verdict};
pub use arena::{TournamentArena, TournamentPhase, DebateResult};
pub use convergence::NashEquilibriumDetector;
