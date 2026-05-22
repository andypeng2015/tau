use super::*;

fn publish(
    store: &mut SessionContextStore,
    session: &str,
    key: &str,
    contributor: &str,
    extension_name: &str,
    value: serde_json::Value,
) {
    store.publish(
        SessionId::from(session),
        tau_proto::SessionContextKey::from(key),
        tau_proto::ConnectionId::from(contributor),
        extension_name.to_owned(),
        tau_proto::SessionContextValue(value),
    );
}

/// Contributions are isolated by `(session, key, contributor)` so one
/// extension can publish multiple keys without overwriting another.
#[test]
fn publish_stores_per_session_key_and_contributor() {
    let mut store = SessionContextStore::default();
    publish(
        &mut store,
        "s1",
        "skills",
        "c1",
        "alpha",
        serde_json::json!([1]),
    );
    publish(
        &mut store,
        "s1",
        "project",
        "c1",
        "alpha",
        serde_json::json!({"root": "/repo"}),
    );
    publish(
        &mut store,
        "s1",
        "skills",
        "c2",
        "beta",
        serde_json::json!([2]),
    );

    let visible = store.template_value(&SessionId::from("s1"));

    assert_eq!(visible["skills"].as_array().expect("skills").len(), 2);
    assert_eq!(visible["project"].as_array().expect("project").len(), 1);
}

/// Republishing the same `(session, key, contributor)` replaces the
/// contributor's previous JSON value instead of appending a duplicate.
#[test]
fn same_contributor_replaces_own_value() {
    let mut store = SessionContextStore::default();
    publish(
        &mut store,
        "s1",
        "skills",
        "c1",
        "alpha",
        serde_json::json!(["old"]),
    );
    publish(
        &mut store,
        "s1",
        "skills",
        "c1",
        "alpha",
        serde_json::json!(["new"]),
    );

    let visible = store.template_value(&SessionId::from("s1"));

    assert_eq!(
        visible["skills"],
        serde_json::json!([{ "extension_name": "alpha", "value": ["new"] }])
    );
}

/// Multiple contributors for the same key are exposed as stable wrapper
/// objects sorted by extension name and then connection id.
#[test]
fn multiple_contributors_are_stable_wrappers_under_same_key() {
    let mut store = SessionContextStore::default();
    publish(
        &mut store,
        "s1",
        "skills",
        "c-z",
        "zeta",
        serde_json::json!([3]),
    );
    publish(
        &mut store,
        "s1",
        "skills",
        "c-a",
        "alpha",
        serde_json::json!([1]),
    );
    publish(
        &mut store,
        "s1",
        "skills",
        "c-b",
        "alpha",
        serde_json::json!([2]),
    );

    let visible = store.template_value(&SessionId::from("s1"));

    assert_eq!(
        visible["skills"],
        serde_json::json!([
            { "extension_name": "alpha", "value": [1] },
            { "extension_name": "alpha", "value": [2] },
            { "extension_name": "zeta", "value": [3] },
        ])
    );
}

/// Session context never leaks between sessions, which matters when one
/// daemon serves different working directories over time.
#[test]
fn different_sessions_do_not_leak_context() {
    let mut store = SessionContextStore::default();
    publish(
        &mut store,
        "s1",
        "skills",
        "c1",
        "alpha",
        serde_json::json!(["s1"]),
    );
    publish(
        &mut store,
        "s2",
        "skills",
        "c1",
        "alpha",
        serde_json::json!(["s2"]),
    );

    let s1 = store.template_value(&SessionId::from("s1"));
    let s2 = store.template_value(&SessionId::from("s2"));

    assert_eq!(s1["skills"][0]["value"], serde_json::json!(["s1"]));
    assert_eq!(s2["skills"][0]["value"], serde_json::json!(["s2"]));
}
