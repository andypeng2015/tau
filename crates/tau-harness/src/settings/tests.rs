use std::collections::HashMap;

use tau_config::settings::{ExtensionEntry, HarnessSettings, load_harness_settings_in};
use tempfile::TempDir;

use super::*;

fn cbor_text(value: &str) -> CborValue {
    CborValue::Text(value.to_owned())
}

fn cbor_map(entries: Vec<(&str, CborValue)>) -> CborValue {
    CborValue::Map(
        entries
            .into_iter()
            .map(|(key, value)| (cbor_text(key), value))
            .collect(),
    )
}

fn empty_config() -> CborValue {
    cbor_map(Vec::new())
}

fn extension(suffix_arg: &str, role: &str, enable: bool, config: CborValue) -> ExtensionEntry {
    ExtensionEntry {
        suffix: Some(vec!["ext".into(), suffix_arg.into()]),
        role: Some(role.into()),
        enable: Some(enable),
        config: Some(config),
        ..Default::default()
    }
}

fn settings_with_test_extensions() -> HarnessSettings {
    let mut settings = HarnessSettings::built_in();
    settings.extensions = HashMap::from([
        (
            "provider-openai".to_owned(),
            extension("ext-provider-openai", "provider", true, empty_config()),
        ),
        (
            "core-shell".to_owned(),
            extension("ext-shell", "tool", true, empty_config()),
        ),
        (
            "test-dummy".to_owned(),
            extension("ext-test-dummy", "tool", false, empty_config()),
        ),
        (
            "std-notifications".to_owned(),
            extension(
                "ext-std-notifications",
                "tool",
                true,
                cbor_map(vec![
                    ("idle_seconds", CborValue::Integer(60.into())),
                    ("idle_agent_summary", CborValue::Bool(false)),
                ]),
            ),
        ),
    ]);
    settings
}

#[test]
fn resolve_extensions_returns_builtins_when_user_config_empty() {
    let s = settings_with_test_extensions();
    let resolved = resolve_extensions(&s).expect("resolve");
    assert_eq!(resolved.len(), 3);
    let names: Vec<&str> = resolved.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["core-shell", "provider-openai", "std-notifications"]
    );
    let provider = resolved
        .iter()
        .find(|e| e.name == "provider-openai")
        .expect("provider");
    assert!(!provider.command.is_empty());
    assert_eq!(provider.args, vec!["ext", "ext-provider-openai"]);
    assert_eq!(provider.role.as_deref(), Some("provider"));
}

#[test]
fn resolve_extensions_builtin_can_start_disabled() {
    let s = settings_with_test_extensions();
    let resolved = resolve_extensions(&s).expect("resolve");
    assert!(resolved.iter().all(|e| e.name != "test-dummy"));
}

#[test]
fn resolve_extensions_disable_drops_entry() {
    let mut s = settings_with_test_extensions();
    s.extensions.insert(
        "core-shell".into(),
        ExtensionEntry {
            enable: Some(false),
            ..Default::default()
        },
    );
    let resolved = resolve_extensions(&s).expect("resolve");
    assert_eq!(resolved.len(), 2);
    let names: Vec<&str> = resolved.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["provider-openai", "std-notifications"]);
}

#[test]
fn resolve_extensions_prefix_wraps_builtin_command() {
    let mut s = settings_with_test_extensions();
    s.extensions
        .get_mut("provider-openai")
        .expect("provider")
        .prefix = Some(vec!["ssh".into(), "user@host".into()]);
    let resolved = resolve_extensions(&s).expect("resolve");
    let provider = resolved
        .iter()
        .find(|e| e.name == "provider-openai")
        .expect("provider");
    // argv[0] is the wrapper; original command moves into args.
    assert_eq!(provider.command, "ssh");
    assert_eq!(provider.args[0], "user@host");
    assert!(!provider.args[1].is_empty());
    assert_eq!(&provider.args[2..], ["ext", "ext-provider-openai"]);
}

#[test]
fn resolve_extensions_user_command_replaces_builtin_command() {
    let mut s = settings_with_test_extensions();
    let provider_entry = s.extensions.get_mut("provider-openai").expect("provider");
    provider_entry.command = Some(vec!["/usr/local/bin/my-provider".into(), "--flag".into()]);
    let resolved = resolve_extensions(&s).expect("resolve");
    let provider = resolved
        .iter()
        .find(|e| e.name == "provider-openai")
        .expect("provider");
    assert_eq!(provider.command, "/usr/local/bin/my-provider");
    assert_eq!(provider.args, vec!["--flag"]);
    // Role is preserved from the built-in default.
    assert_eq!(provider.role.as_deref(), Some("provider"));
}

#[test]
fn resolve_extensions_user_command_from_nickel_ignores_builtin_suffix_field() {
    // Regression test for built-ins folded into the native Nickel merge: the
    // built-in `suffix` is fixed fallback data, not arguments to
    // append after a replacement command.
    let s = load_settings_from_harness_ncl(
        r#"{
                extensions = { "provider-openai" = { command = ["/my/provider"] } },
            }"#,
    );
    let resolved = resolve_extensions(&s).expect("resolve");
    let provider = resolved
        .iter()
        .find(|e| e.name == "provider-openai")
        .expect("provider");
    assert_eq!(provider.command, "/my/provider");
    assert!(provider.args.is_empty());
}

#[test]
fn resolve_extensions_user_command_from_nickel_ignores_suffix() {
    // `command` is a complete argv replacement. Because built-in `suffix` is
    // now represented as normal fixed `suffix` values after Nickel layering,
    // the resolver cannot reliably distinguish them from user-forced suffixes.
    // Users who need extra arguments with a command should include them in the
    // command vector itself.
    let s = load_settings_from_harness_ncl(
        r#"{
                extensions = {
                    "provider-openai" = {
                        command = ["/my/provider"],
                        suffix | force = ["--ignored-suffix"],
                    },
                },
            }"#,
    );
    let resolved = resolve_extensions(&s).expect("resolve");
    let provider = resolved
        .iter()
        .find(|e| e.name == "provider-openai")
        .expect("provider");
    assert_eq!(provider.command, "/my/provider");
    assert!(provider.args.is_empty());
}

#[test]
fn resolve_extensions_adds_user_extension_keys() {
    let mut s = settings_with_test_extensions();
    s.extensions.insert(
        "mything".into(),
        ExtensionEntry {
            command: Some(vec!["/usr/local/bin/mything".into()]),
            ..Default::default()
        },
    );
    let resolved = resolve_extensions(&s).expect("resolve");
    assert_eq!(resolved.len(), 4);
    let mything = resolved
        .iter()
        .find(|e| e.name == "mything")
        .expect("mything");
    assert_eq!(mything.command, "/usr/local/bin/mything");
    assert!(mything.role.is_none());
}

#[test]
fn resolve_extensions_empty_entry_does_not_re_enable_disabled_builtin() {
    // `extensions = { "test-dummy" = {} }` MUST leave the
    // built-in's `enable = false` intact after Nickel has layered
    // user config over built-in defaults.
    let s = load_settings_from_harness_ncl(
        r#"{
                extensions = { "test-dummy" = {} },
            }"#,
    );
    let resolved = resolve_extensions(&s).expect("resolve");
    assert!(resolved.iter().all(|e| e.name != "test-dummy"));
}

#[test]
fn resolve_extensions_user_extension_without_command_errors() {
    let mut s = settings_with_test_extensions();
    s.extensions.insert(
        "broken".into(),
        ExtensionEntry {
            ..Default::default()
        },
    );
    let err = resolve_extensions(&s).expect_err("must err");
    match err {
        ResolveExtensionsError::EmptyCommand(name) => assert_eq!(name, "broken"),
    }
}

fn load_settings_from_harness_ncl(text: &str) -> HarnessSettings {
    let td = TempDir::new().expect("tempdir");
    let dir = td.path();
    std::fs::write(dir.join("harness.ncl"), text).expect("write");

    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(dir.to_owned()),
        state_dir: None,
    };
    load_harness_settings_in(&dirs).expect("load")
}

#[test]
fn load_harness_settings_user_role_override_for_builtin_conflicts() {
    // Built-in extension roles are fixed harness-owned classifications. Unlike
    // defaults, a user-provided different role should be rejected by Nickel's
    // native non-default merge conflict handling.
    let td = TempDir::new().expect("tempdir");
    let dir = td.path();
    std::fs::write(
        dir.join("harness.ncl"),
        r#"{
                extensions = { "provider-openai" = { role = "tool" } },
            }"#,
    )
    .expect("write");

    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(dir.to_owned()),
        state_dir: None,
    };
    let err = load_harness_settings_in(&dirs).expect_err("role conflict");
    assert!(
        err.to_string().contains("non mergeable terms"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_extensions_loads_from_nickel() {
    // End-to-end: a realistic harness.ncl round-trips through the
    // tau-config loader into the tau-harness resolver.
    let td = TempDir::new().expect("tempdir");
    let dir = td.path();
    std::fs::write(
        dir.join("harness.ncl"),
        r#"{
                extensions = {
                    "core-shell" = { enable = false },
                    "test-dummy" = { enable = true },
                    "provider-openai" = { prefix = ["ssh", "host"] },
                    mything = { command = ["/bin/foo"] },
                },
            }"#,
    )
    .expect("write");

    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(dir.to_owned()),
        state_dir: None,
    };
    let s = load_harness_settings_in(&dirs).expect("load");
    let resolved = resolve_extensions(&s).expect("resolve");
    let names: Vec<&str> = resolved.iter().map(|e| e.name.as_str()).collect();
    // core-shell dropped (disable). test-dummy enabled. provider-openai
    // kept (prefix-wrapped). Output is sorted by extension name, not by
    // built-in declaration order.
    assert_eq!(
        names,
        vec![
            "core-delegate",
            "mything",
            "provider-openai",
            "std-notifications",
            "std-websearch-exa",
            "test-dummy",
        ]
    );
    let provider = resolved
        .iter()
        .find(|e| e.name == "provider-openai")
        .expect("provider");
    assert_eq!(provider.command, "ssh");
    assert_eq!(provider.args[0], "host");
    assert!(!provider.args[1].is_empty());
    assert_eq!(&provider.args[2..], ["ext", "ext-provider-openai"]);
}

/// Force a parse of built-in harness Nickel extension defaults so malformed
/// extension config blows up here rather than at user startup.
#[test]
fn built_in_harness_ncl_extension_defaults_parse() {
    // Built-in extension config is deserialized straight to `CborValue`, so
    // lifecycle configure can send CBOR without a JSON conversion step.
    let settings = HarnessSettings::built_in();
    let notifications = settings
        .extensions
        .get("std-notifications")
        .expect("std-notifications");
    assert_eq!(
        notifications.config,
        Some(cbor_map(vec![
            ("idle_seconds", CborValue::Integer(60.into())),
            ("idle_agent_summary", CborValue::Bool(false)),
        ]))
    );

    let resolved = resolve_extensions(&settings).expect("resolve");
    let notifications = resolved
        .iter()
        .find(|extension| extension.name == "std-notifications")
        .expect("std-notifications");
    assert_eq!(
        notifications.config,
        cbor_map(vec![
            ("idle_seconds", CborValue::Integer(60.into())),
            ("idle_agent_summary", CborValue::Bool(false)),
        ])
    );
}
