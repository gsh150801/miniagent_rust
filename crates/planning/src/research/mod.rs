pub mod scheduler;
pub mod pi;
pub mod tournament_master;
pub mod evidence_accumulator;
pub mod synthesis_judge;
pub mod alzheimers;
pub mod profiles;

pub use scheduler::{AgentSpec, AgentTemplate, DynamicAgent, AgentRegistry, AgentStatus, SchedulerRole};
pub use pi::PrincipalInvestigatorRole;
pub use tournament_master::TournamentMasterRole;
pub use evidence_accumulator::EvidenceAccumulatorRole;
pub use synthesis_judge::SynthesisJudgeRole;
pub use alzheimers::{build_alzheimers_pipeline, AlzheimersConfig};
pub use profiles::AlzheimersHypothesisProfile;
