use std::collections::{HashSet, VecDeque};
use std::sync::mpsc;
use std::time::Duration;

use tau_config::settings::ModelRegistry;
use tau_proto::{Event, Frame, SessionId, SessionPromptId, UiCancelPrompt};
use tau_provider::storage::AuthStore;

use crate::responses::pool::WsPoolStats;
use crate::{RetryContext, SleepOutcome, compute_ws_pool_delta};

#[test]
fn no_config_resolves_none() {
    let models = ModelRegistry::default();
    let mut auth = AuthStore::default();
    assert!(tau_provider::resolve(&"fake/model".into(), &models, &mut auth).is_none());
}

#[test]
fn ws_pool_delta_subtracts_each_counter() {
    let before = WsPoolStats {
        upgrades: 3,
        silent_reconnects: 1,
        chain_strips_on_fresh: 1,
    };
    let after = WsPoolStats {
        upgrades: 5,
        silent_reconnects: 2,
        chain_strips_on_fresh: 4,
    };
    let delta = compute_ws_pool_delta(before, after);
    assert_eq!(delta.upgrades, 2);
    assert_eq!(delta.silent_reconnects, 1);
    assert_eq!(delta.chain_strips_on_fresh, 3);
}

#[test]
fn ws_pool_delta_saturates_on_counter_reset() {
    // Defensive against a counter reset (pool was rebuilt mid-life
    // — shouldn't happen, but the saturating-sub fence keeps the
    // wire payload sane if it ever does).
    let before = WsPoolStats {
        upgrades: 10,
        silent_reconnects: 0,
        chain_strips_on_fresh: 0,
    };
    let after = WsPoolStats {
        upgrades: 2,
        silent_reconnects: 0,
        chain_strips_on_fresh: 0,
    };
    let delta = compute_ws_pool_delta(before, after);
    assert_eq!(delta.upgrades, 0);
}

#[test]
fn ws_pool_delta_clamps_u64_to_u32() {
    let before = WsPoolStats::default();
    let after = WsPoolStats {
        upgrades: u64::from(u32::MAX) + 5,
        silent_reconnects: 0,
        chain_strips_on_fresh: 0,
    };
    let delta = compute_ws_pool_delta(before, after);
    assert_eq!(delta.upgrades, u32::MAX);
}

fn cancel_event(target: Option<&str>) -> Frame {
    Frame::Event(Event::UiCancelPrompt(UiCancelPrompt {
        session_id: SessionId::from("s-test"),
        session_prompt_id: target.map(SessionPromptId::from),
    }))
}

#[test]
fn sleep_or_abort_targeted_cancel_for_other_spid_does_not_abort() {
    // Regression for the bug where a side-conv preempt cancel
    // aborted an in-flight user/delegate retry. The cancel for a
    // *different* spid should be parked in `canceled_spids` and
    // the sleep should complete normally so the current attempt
    // finishes.
    let (tx, rx) = mpsc::channel::<Frame>();
    let mut deferred = VecDeque::new();
    let mut canceled = HashSet::new();
    let mut ctx = RetryContext {
        frame_rx: &rx,
        deferred: &mut deferred,
        canceled_spids: &mut canceled,
    };

    tx.send(cancel_event(Some("other-spid")))
        .expect("send cancel event");
    let outcome = ctx.sleep_or_abort(Duration::from_millis(50), "current-spid");
    assert!(matches!(outcome, SleepOutcome::Elapsed));
    assert!(
        deferred.is_empty(),
        "non-matching cancel should not be deferred"
    );
    assert!(canceled.contains(&SessionPromptId::from("other-spid")));
}

#[test]
fn sleep_or_abort_targeted_cancel_for_current_spid_aborts() {
    let (tx, rx) = mpsc::channel::<Frame>();
    let mut deferred = VecDeque::new();
    let mut canceled = HashSet::new();
    let mut ctx = RetryContext {
        frame_rx: &rx,
        deferred: &mut deferred,
        canceled_spids: &mut canceled,
    };

    tx.send(cancel_event(Some("current-spid")))
        .expect("send cancel event");
    let outcome = ctx.sleep_or_abort(Duration::from_secs(5), "current-spid");
    assert!(matches!(outcome, SleepOutcome::Aborted));
    assert_eq!(
        deferred.len(),
        1,
        "matching cancel should be deferred for the main loop"
    );
    assert!(canceled.is_empty());
}

#[test]
fn sleep_or_abort_broadcast_cancel_aborts() {
    // Legacy `/cancel` (session_prompt_id: None) is a broadcast —
    // abort whatever's in flight regardless of spid.
    let (tx, rx) = mpsc::channel::<Frame>();
    let mut deferred = VecDeque::new();
    let mut canceled = HashSet::new();
    let mut ctx = RetryContext {
        frame_rx: &rx,
        deferred: &mut deferred,
        canceled_spids: &mut canceled,
    };

    tx.send(cancel_event(None)).expect("send cancel event");
    let outcome = ctx.sleep_or_abort(Duration::from_secs(5), "current-spid");
    assert!(matches!(outcome, SleepOutcome::Aborted));
    assert_eq!(deferred.len(), 1);
    assert!(canceled.is_empty());
}
