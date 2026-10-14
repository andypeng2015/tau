//! Generic, transient activity ownership for configured extension incarnations.

use tau_proto::{
    ActivityLeaseAction, ActivityLeaseOutcome, ExtActivityLeaseRequest, ExtActivityLeaseResult,
};

use super::*;

#[cfg(test)]
mod tests;

/// Bounded operation lifecycle for one connected extension. Retired IDs prevent
/// delayed duplicate acquisition from resurrecting completed work.
#[derive(Default)]
pub(super) struct ActivityLeases {
    /// Operation IDs mapped to whether they still hold activity.
    operations: HashMap<String, bool>,
}

impl ActivityLeases {
    /// Apply one idempotent transition without interpreting the operation ID.
    fn apply(&mut self, request: &ExtActivityLeaseRequest) -> ActivityLeaseOutcome {
        let existing = self.operations.get(&request.operation_id).copied();
        if existing.is_none() && self.operations.len() >= 4096 {
            return ActivityLeaseOutcome::Rejected {
                reason: "activity operation capacity exhausted".to_owned(),
            };
        }
        match request.action {
            ActivityLeaseAction::Release => {
                self.operations.insert(request.operation_id.clone(), false);
                ActivityLeaseOutcome::Released
            }
            ActivityLeaseAction::Acquire if existing == Some(false) => {
                ActivityLeaseOutcome::Rejected {
                    reason: "activity operation already retired".to_owned(),
                }
            }
            ActivityLeaseAction::Acquire => {
                if existing.is_none()
                    && self.operations.values().filter(|active| **active).count() >= 256
                {
                    return ActivityLeaseOutcome::Rejected {
                        reason: "active lease capacity exhausted".to_owned(),
                    };
                }
                self.operations.insert(request.operation_id.clone(), true);
                ActivityLeaseOutcome::Acquired
            }
        }
    }

    /// Whether this owner has any unsettled work.
    pub(super) fn is_busy(&self) -> bool {
        self.operations.values().any(|active| *active)
    }
}

impl Harness {
    /// Mutate ownership only after ordinary request publication commits, using
    /// the authenticated pre-interception extension incarnation.
    pub(super) fn process_committed_activity_lease_request(
        &mut self,
        peer: &interception::PeerPublicationContext,
        request: &ExtActivityLeaseRequest,
    ) {
        let Some(extension) = peer.extension.as_ref() else {
            return;
        };
        let current = self
            .extensions
            .entries
            .get(&extension.source)
            .is_some_and(|entry| {
                extension.admission.session_id == self.session_runtime.current_session_id
                    && extension.admission.session_generation
                        == self.session_runtime.current_session_generation
                    && entry.instance_id == extension.instance_id
                    && entry.name == extension.publisher
                    && entry.state != ExtensionState::Disconnected
            });
        if !current {
            return;
        }
        let outcome = if request.session_id != self.session_runtime.current_session_id
            || request.operation_id.is_empty()
            || request.operation_id.len() > 256
        {
            ActivityLeaseOutcome::Rejected {
                reason: "invalid activity lease request".to_owned(),
            }
        } else {
            self.extensions
                .activity_leases
                .entry(extension.source.clone())
                .or_default()
                .apply(request)
        };
        self.publish_event(
            Some(super::harness_connection_id()),
            Event::ExtActivityLeaseResult(ExtActivityLeaseResult {
                session_id: request.session_id.clone(),
                publisher: extension.publisher.clone(),
                connection_id: extension.source.clone(),
                operation_id: request.operation_id.clone(),
                action: request.action,
                outcome,
            }),
        );
    }
}
