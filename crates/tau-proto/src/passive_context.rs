//! Generic extension-owned, non-activating transcript appends.

use serde::{Deserialize, Serialize};

use crate::{AgentId, ExtensionName, SessionId};

/// Maximum UTF-8 body size accepted by the passive append API.
pub const PASSIVE_CONTEXT_BODY_LIMIT: usize = 64 * 1024;

/// Request to append attributed output without scheduling inference.
///
/// The configured publisher owns operation IDs within each agent. Retrying the
/// same ID and body is idempotent, including after harness restart; reusing it
/// with different content fails. This facility never executes the operation
/// that produced the output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtPassiveContextAppendRequest {
    /// Bound harness session expected by the caller.
    pub session_id: SessionId,
    /// Loaded transcript receiving the output.
    pub agent_id: AgentId,
    /// Nonempty publisher-scoped identifier, at most 256 UTF-8 bytes.
    pub operation_id: String,
    /// Untrusted output body, bounded by [`PASSIVE_CONTEXT_BODY_LIMIT`].
    pub body: String,
}

/// Canonical, attributed passive output accepted into an agent journal.
///
/// The harness stamps publisher identity and renders the shared output
/// envelope. This fact never creates an inference activation obligation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentPassiveContextAppended {
    /// Session that admitted the append.
    pub session_id: SessionId,
    /// Transcript receiving the output.
    pub agent_id: AgentId,
    /// Authenticated configured extension, not a payload-local claim.
    pub publisher: ExtensionName,
    /// Idempotency key within this publisher and agent.
    pub operation_id: String,
    /// Complete registered output envelope.
    pub text: String,
}

/// Outcome of one passive append attempt, not of the external operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PassiveContextAppendOutcome {
    /// The fact committed, or an identical operation had already committed.
    Committed,
    /// No new fact was committed by this attempt.
    Failed {
        /// Bounded diagnostic explaining admission or publication failure.
        reason: String,
    },
}

/// Transient harness result, correlated without interpreting extension
/// payloads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtPassiveContextAppendResult {
    /// Requested session.
    pub session_id: SessionId,
    /// Requested transcript.
    pub agent_id: AgentId,
    /// Configured publisher whose operation was attempted.
    pub publisher: ExtensionName,
    /// Operation identifier from the request.
    pub operation_id: String,
    /// Commit or failure result. Silence is an unconfirmed outcome, not
    /// failure.
    pub outcome: PassiveContextAppendOutcome,
}
