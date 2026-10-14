//! Transient configured-extension ownership of background activity.

use serde::{Deserialize, Serialize};

use crate::{ConnectionId, ExtensionName, SessionId};

#[cfg(test)]
mod tests;

/// Change a process-local activity hold. Acquire must be acknowledged before
/// starting effects; release follows all completion and context settlement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtActivityLeaseRequest {
    /// Current bound session; historical requests never restore holds.
    pub session_id: SessionId,
    /// Opaque, nonempty operation ID of at most 256 UTF-8 bytes.
    pub operation_id: String,
    /// Requested ownership transition.
    pub action: ActivityLeaseAction,
}

/// Lifecycle transition for one extension-incarnation-owned operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLeaseAction {
    /// Hold the session busy until release or owner disconnect.
    Acquire,
    /// Retire the operation; the same ID cannot acquire again.
    Release,
}

/// Confirmed disposition, not a guarantee against explicit shutdown or crash.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityLeaseOutcome {
    /// The hold exists, including an idempotent duplicate acquisition.
    Acquired,
    /// The operation is retired, including an idempotent duplicate release.
    Released,
    /// No requested transition occurred.
    Rejected {
        /// Bounded admission diagnostic.
        reason: String,
    },
}

/// Harness-authenticated acknowledgement. Silence is not acquisition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtActivityLeaseResult {
    /// Bound session of the request.
    pub session_id: SessionId,
    /// Configured owner, captured before interception.
    pub publisher: ExtensionName,
    /// Exact owner connection incarnation; a replacement cannot inherit holds.
    pub connection_id: ConnectionId,
    /// Opaque operation ID from the request.
    pub operation_id: String,
    /// Requested transition, preserved for correlation.
    pub action: ActivityLeaseAction,
    /// Actual process-local disposition.
    pub outcome: ActivityLeaseOutcome,
}
