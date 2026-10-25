//! Lifecycle-owned asynchronous semantic journal persistence.
//!
//! This module is the sole mutable filesystem owner for live semantic streams.
//! Stores prevalidate exact records and use a generation-bound lease to enqueue
//! one FIFO frame before completing an exclusive incremental in-memory fold.

mod agent_history_prefix;
mod agent_io_guard;
mod backend;
mod capacity;
pub(crate) mod fold_memory;
mod identity;
mod owner;
mod preparation;
mod worker;

pub use agent_history_prefix::AgentHistoryPrefix;
pub use capacity::PersistenceCapacity;
pub use identity::{PersistenceGeneration, PersistenceLease, StreamIdentity};
#[cfg(any(test, feature = "test-persistence"))]
pub use owner::DurabilityBarrierOutcome;
pub use owner::{
    PersistenceAdmissionError, PersistenceCapacityLimit, PersistenceCapacityPressure,
    PersistenceFailure, PersistenceFailureKind, PersistenceOperationalStatus, PersistenceUsage,
    SemanticPersistenceOwner,
};
pub(crate) use owner::{RetentionCharge, StagedFrame};
pub use preparation::{
    PreparedAgentStream, PreparedSessionStreams, SessionPreparationMode, SessionPreparationStatus,
};
pub(crate) use worker::AgentCheckpointCandidate;

#[cfg(test)]
mod tests;
