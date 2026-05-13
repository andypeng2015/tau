use tau_config::settings::ModelRegistry;
use tau_provider::storage::AuthStore;

use crate::compute_ws_pool_delta;
use crate::responses::pool::WsPoolStats;

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
