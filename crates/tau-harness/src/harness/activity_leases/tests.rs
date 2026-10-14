use super::*;

/// Active and retired records are bounded without sacrificing release capacity
/// or evicting IDs that could allow delayed acquisitions to resurrect effects.
#[test]
fn activity_lease_capacity_preserves_release_and_tombstones() {
    let mut leases = ActivityLeases::default();
    let mut request = ExtActivityLeaseRequest {
        session_id: "session".parse().expect("session"),
        operation_id: String::new(),
        action: ActivityLeaseAction::Acquire,
    };
    for id in 0..256 {
        request.operation_id = id.to_string();
        assert_eq!(leases.apply(&request), ActivityLeaseOutcome::Acquired);
    }
    request.operation_id = "overflow".to_owned();
    assert!(matches!(
        leases.apply(&request),
        ActivityLeaseOutcome::Rejected { .. }
    ));
    request.action = ActivityLeaseAction::Release;
    for id in 0..4096 {
        request.operation_id = id.to_string();
        assert_eq!(leases.apply(&request), ActivityLeaseOutcome::Released);
    }
    assert!(!leases.is_busy());
    request.operation_id = "overflow".to_owned();
    assert!(matches!(
        leases.apply(&request),
        ActivityLeaseOutcome::Rejected { .. }
    ));
    request.operation_id = "0".to_owned();
    assert_eq!(leases.apply(&request), ActivityLeaseOutcome::Released);
    request.action = ActivityLeaseAction::Acquire;
    assert!(matches!(
        leases.apply(&request),
        ActivityLeaseOutcome::Rejected { .. }
    ));
}
