use super::*;
use crate::{Event, EventName};

/// Lease ownership is transient protocol state; wire names and serde tags must
/// agree without accidentally opting either side into journal persistence.
#[test]
fn activity_lease_wire_names_and_transience() {
    let request = ExtActivityLeaseRequest {
        session_id: "session".parse().expect("session"),
        operation_id: "opaque".to_owned(),
        action: ActivityLeaseAction::Acquire,
    };
    let result = ExtActivityLeaseResult {
        session_id: request.session_id.clone(),
        publisher: "worker".parse().expect("publisher"),
        connection_id: "connection".parse().expect("connection"),
        operation_id: request.operation_id.clone(),
        action: request.action,
        outcome: ActivityLeaseOutcome::Acquired,
    };
    for (event, name) in [
        (
            Event::ExtActivityLeaseRequest(request),
            EventName::EXT_ACTIVITY_LEASE_REQUEST,
        ),
        (
            Event::ExtActivityLeaseResult(result),
            EventName::EXT_ACTIVITY_LEASE_RESULT,
        ),
    ] {
        assert_eq!(event.name(), name);
        assert!(!event.defaults_to_persist());
        let json = serde_json::to_value(&event).expect("serialize");
        assert_eq!(json["event"], name.to_string());
        let decoded: Event = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, event);
    }
}
