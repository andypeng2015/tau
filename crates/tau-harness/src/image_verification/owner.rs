use tau_core::PersistenceLease;
use tau_proto::{AgentId, ConnectionId, ExtensionInstanceId, SessionId};

use crate::harness::SessionGeneration;

/// Exact runtime and durable authority captured before off-loop image
/// admission.
#[derive(Clone)]
pub(crate) struct ImageVerificationOwner {
    /// Loaded runtime that owns the tool call.
    pub(crate) cid: AgentId,
    /// Monotonic loaded-runtime incarnation.
    pub(crate) runtime_incarnation: u64,
    /// Immutable session identity at admission.
    pub(crate) session_id: SessionId,
    /// Session binding generation at admission.
    pub(crate) session_generation: SessionGeneration,
    /// Exact configured source connection.
    pub(crate) connection: ConnectionId,
    /// Configured source incarnation captured from the committed report.
    pub(crate) instance_id: ExtensionInstanceId,
    /// Revocable exact durable Agent generation, not physical custody itself.
    pub(crate) lease: PersistenceLease,
}

impl ImageVerificationOwner {
    /// Matches a returned request to its independently retained runtime owner.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.cid == other.cid
            && self.runtime_incarnation == other.runtime_incarnation
            && self.session_id == other.session_id
            && self.session_generation == other.session_generation
            && self.connection == other.connection
            && self.instance_id == other.instance_id
            && self.lease.is_same_generation_as(&other.lease)
    }
}
