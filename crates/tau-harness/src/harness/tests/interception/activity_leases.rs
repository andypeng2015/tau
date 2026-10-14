//! Admission, interception, and incarnation ownership for generic activity.

use tau_proto::{ActivityLeaseAction, ActivityLeaseOutcome, ExtActivityLeaseRequest};

use super::*;

/// Publish one generic operation through configured-peer intake.
fn request(h: &mut Harness, owner: &str, id: &str, action: ActivityLeaseAction) {
    h.handle_extension_event_inner(
        &crate::test_connection_id(owner),
        Event::ExtActivityLeaseRequest(ExtActivityLeaseRequest {
            session_id: h.session_runtime.current_session_id.clone(),
            operation_id: id.to_owned(),
            action,
        }),
    )
    .expect("activity request");
}

/// Duplicate requests never multiply holds, retired IDs cannot resurrect work,
/// and one configured owner cannot release another owner's operation.
#[test]
fn activity_lease_ownership_is_idempotent_and_incarnation_scoped() {
    let tmp = TempDir::new().expect("tempdir");
    let mut h = quiet_provider_harness(tmp.path()).expect("harness");
    for owner in ["writer", "other"] {
        connect_ready_configured_extension(&mut h, owner, owner, tau_proto::ClientKind::Tool);
    }
    request(&mut h, "writer", "job", ActivityLeaseAction::Acquire);
    request(&mut h, "writer", "job", ActivityLeaseAction::Acquire);
    request(&mut h, "other", "job", ActivityLeaseAction::Release);
    let writer = crate::test_connection_id("writer");
    assert!(h.extensions.activity_leases[&writer].is_busy());
    request(&mut h, "writer", "job", ActivityLeaseAction::Release);
    request(&mut h, "writer", "job", ActivityLeaseAction::Release);
    request(&mut h, "writer", "job", ActivityLeaseAction::Acquire);
    assert!(!h.extensions.activity_leases[&writer].is_busy());
    assert!(event_log_events(&h).iter().any(|event| matches!(event,
        Event::ExtActivityLeaseResult(result) if result.operation_id == "job"
            && matches!(result.outcome, ActivityLeaseOutcome::Rejected { .. }))));
    request(&mut h, "writer", "next", ActivityLeaseAction::Acquire);
    h.handle_disconnect(&writer);
    assert!(!h.extensions.activity_leases.contains_key(&writer));
    h.shutdown().expect("shutdown");
}

/// Intercepted/dropped acquire intent must not create a hold or acknowledge it.
#[test]
fn activity_lease_acquisition_waits_for_publication_and_cannot_be_forged() {
    let tmp = TempDir::new().expect("tempdir");
    let mut h = quiet_provider_harness(tmp.path()).expect("harness");
    connect_ready_configured_extension(&mut h, "writer", "writer", tau_proto::ClientKind::Tool);
    connect_test_tool(&mut h, "lease-interceptor");
    h.handle_extension_event(
        "lease-interceptor",
        TestProtocolItem::Message(TestMessage::Intercept(Intercept {
            selectors: vec![EventSelector::Exact(
                tau_proto::EventName::EXT_ACTIVITY_LEASE_REQUEST,
            )],
            priority: InterceptionPriority::new(0),
        })),
    )
    .expect("intercept");
    request(&mut h, "writer", "held", ActivityLeaseAction::Acquire);
    assert!(h.extensions.activity_leases.is_empty());
    assert!(
        !event_log_events(&h)
            .iter()
            .any(|event| matches!(event, Event::ExtActivityLeaseResult(_)))
    );
    h.handle_extension_event(
        "lease-interceptor",
        TestProtocolItem::Message(TestMessage::InterceptReply(InterceptReply {
            action: InterceptAction::Drop,
        })),
    )
    .expect("drop");
    assert!(h.extensions.activity_leases.is_empty());
    assert!(
        !event_log_events(&h)
            .iter()
            .any(|event| matches!(event, Event::ExtActivityLeaseResult(_)))
    );
    h.shutdown().expect("shutdown");
}

/// Results cannot be forged by configured peers, and interception cannot alter
/// either the operation being acquired or the acknowledged disposition.
#[test]
fn activity_lease_correlation_and_acknowledgements_are_immutable() {
    let request = Event::ExtActivityLeaseRequest(ExtActivityLeaseRequest {
        session_id: "session".parse().expect("session"),
        operation_id: "first".to_owned(),
        action: ActivityLeaseAction::Acquire,
    });
    let mut replacement = request.clone();
    if let Event::ExtActivityLeaseRequest(request) = &mut replacement {
        request.operation_id = "different".to_owned();
    }
    assert!(
        crate::harness::interception::immutable_protected_fact_was_modified(&request, &replacement)
    );
    let result = Event::ExtActivityLeaseResult(tau_proto::ExtActivityLeaseResult {
        session_id: "session".parse().expect("session"),
        publisher: "writer".parse().expect("publisher"),
        connection_id: "writer-1".parse().expect("connection"),
        operation_id: "first".to_owned(),
        action: ActivityLeaseAction::Acquire,
        outcome: ActivityLeaseOutcome::Acquired,
    });
    assert!(Harness::is_peer_forbidden_harness_fact(&result));
    let mut replacement = result.clone();
    if let Event::ExtActivityLeaseResult(result) = &mut replacement {
        result.outcome = ActivityLeaseOutcome::Released;
    }
    assert!(
        crate::harness::interception::immutable_protected_fact_was_modified(&result, &replacement)
    );
}
