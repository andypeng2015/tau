use tau_proto::HarnessInfoLevel;

use super::*;

/// Scan the harness event log for an `Important` `HarnessInfo`
/// containing `needle` and return its message. The startup paths emit
/// these synchronously before the constructor returns, so by the time
/// the test inspects the log every check_*_parses event is already
/// committed — no need to pump the bus.
fn find_important_info(h: &Harness, needle: &str) -> Option<String> {
    let mut seq = 0;
    while let Some(entry) = h.event_log.get_next_from(seq) {
        seq = entry.seq + 1;
        if let Event::HarnessInfo(info) = &entry.event
            && info.level == HarnessInfoLevel::Important
            && info.message.contains(needle)
        {
            return Some(info.message.clone());
        }
    }
    None
}

#[test]
fn selected_effort_is_model_specific_and_clamped() {
    let td = TempDir::new().expect("tempdir");
    let config_dir = td.path().join("config");
    let state_dir = td.path().join("state");
    std::fs::create_dir_all(&config_dir).expect("mkdir config");
    std::fs::create_dir_all(&state_dir).expect("mkdir state");
    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(config_dir.clone()),
        state_dir: Some(state_dir.clone()),
    };

    std::fs::write(
        config_dir.join("harness.json5"),
        r#"{
            default_efforts: {
                "openai/gpt-4.1": "high",
                "local/llama": "high",
            },
        }"#,
    )
    .expect("write harness config");
    std::fs::write(
        config_dir.join("models.json5"),
        r#"{
            providers: {
                local: {
                    compat: { supportsReasoningEffort: false },
                    models: [{ id: "llama" }],
                },
                openai: {
                    compat: { supportsReasoningEffort: true },
                    models: [{ id: "gpt-4.1" }],
                },
            },
        }"#,
    )
    .expect("write models");
    std::fs::write(
        state_dir.join("harness.json5"),
        r#"{
            "last_selected_model": "openai/gpt-4.1",
            "last_efforts": {
                "openai/gpt-4.1": "minimal",
                "local/llama": "high"
            }
        }"#,
    )
    .expect("write state");

    let harness_settings =
        tau_config::settings::load_harness_settings_in(&dirs).expect("load harness settings");
    let model_registry = tau_config::settings::load_models_in(&dirs).expect("load models");

    assert_eq!(
        selected_effort_for_model(&dirs, &harness_settings, &model_registry, "openai/gpt-4.1",),
        tau_proto::Effort::High
    );
    assert_eq!(
        selected_effort_for_model(&dirs, &harness_settings, &model_registry, "local/llama"),
        tau_proto::Effort::Off
    );
}

/// First-time users (no per-model entry in `default_efforts`, no
/// persisted `last_efforts`) get the middle of the available
/// reasoning levels, not the lowest. For the standard
/// reasoning-supporting list (`[Off, Minimal, Low, Medium, High]`)
/// that's `Low`. Non-reasoning providers stay at `Off`.
#[test]
fn fresh_install_picks_middle_effort_when_no_history() {
    let td = TempDir::new().expect("tempdir");
    let config_dir = td.path().join("config");
    let state_dir = td.path().join("state");
    std::fs::create_dir_all(&config_dir).expect("mkdir config");
    std::fs::create_dir_all(&state_dir).expect("mkdir state");
    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(config_dir.clone()),
        state_dir: Some(state_dir.clone()),
    };

    // No harness.json5: default settings, empty default_efforts.
    std::fs::write(
        config_dir.join("models.json5"),
        r#"{
            providers: {
                local: {
                    compat: { supportsReasoningEffort: false },
                    models: [{ id: "llama" }],
                },
                openai: {
                    compat: { supportsReasoningEffort: true },
                    models: [{ id: "gpt-4.1" }],
                },
            },
        }"#,
    )
    .expect("write models");
    // No harness.json5: fresh install.

    let harness_settings =
        tau_config::settings::load_harness_settings_in(&dirs).expect("load harness settings");
    let model_registry = tau_config::settings::load_models_in(&dirs).expect("load models");

    assert_eq!(
        selected_effort_for_model(&dirs, &harness_settings, &model_registry, "openai/gpt-4.1"),
        tau_proto::Effort::Low,
    );
    assert_eq!(
        selected_effort_for_model(&dirs, &harness_settings, &model_registry, "local/llama"),
        tau_proto::Effort::Off,
    );
}

/// A malformed `models.json5` must surface in the UI as an `Important`
/// `HarnessInfo`, including the raw parser error. Without this, the
/// only symptom of a borked file is an empty model list with no
/// indication of why — easy to miss because stderr is hidden once the
/// TUI takes over.
#[test]
fn borked_models_json5_emits_important_info() {
    let td = TempDir::new().expect("tempdir");
    let config_dir = td.path().join("config");
    let state_dir = td.path().join("state");
    std::fs::create_dir_all(&config_dir).expect("mkdir config");
    std::fs::create_dir_all(&state_dir).expect("mkdir state");
    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(config_dir.clone()),
        state_dir: Some(state_dir.clone()),
    };

    // Syntactically invalid JSON5 — missing closing brace.
    std::fs::write(
        config_dir.join("models.json5"),
        "{ providers: { local: { models: [ { id: \"llama\" } ] }",
    )
    .expect("write borked models");

    let h = echo_harness_with_dirs("s1", state_dir, dirs).expect("harness");
    let message = find_important_info(&h, "models.json5")
        .expect("expected Important HarnessInfo about models.json5");
    assert!(
        message.contains("failed to parse"),
        "message should explain what happened, got: {message}"
    );
    assert!(
        message.contains("ignored"),
        "message should call out that the file is being ignored, got: {message}"
    );
}

/// A malformed `harness.json5` must surface the same way. This path
/// already worked but had no test coverage; lock it in alongside the
/// new models.json5 path so a future refactor that drops one will
/// drop both, not just the easy one.
#[test]
fn borked_harness_json5_emits_important_info() {
    let td = TempDir::new().expect("tempdir");
    let config_dir = td.path().join("config");
    let state_dir = td.path().join("state");
    std::fs::create_dir_all(&config_dir).expect("mkdir config");
    std::fs::create_dir_all(&state_dir).expect("mkdir state");
    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(config_dir.clone()),
        state_dir: Some(state_dir.clone()),
    };

    std::fs::write(
        config_dir.join("harness.json5"),
        "{ extensions: { foo: { command: [ \"echo\" ",
    )
    .expect("write borked harness");

    let h = echo_harness_with_dirs("s1", state_dir, dirs).expect("harness");
    let message = find_important_info(&h, "harness.json5")
        .expect("expected Important HarnessInfo about harness.json5");
    assert!(
        message.contains("failed to parse"),
        "message should explain what happened, got: {message}"
    );
}

/// `efforts_for_model` appends `XHigh` for models that opt in (either
/// the built-in whitelist of known OpenAI IDs, or an explicit
/// `supportsXhigh: true` in models.json5), and omits it for the
/// rest. Pinning the set so a future tweak to the whitelist still
/// surfaces here.
#[test]
fn efforts_for_model_includes_xhigh_for_supported_models_only() {
    use tau_proto::Effort as L;

    let td = TempDir::new().expect("tempdir");
    let dir = td.path();
    std::fs::write(
        dir.join("models.json5"),
        r#"{
            providers: {
                openai: {
                    api: "openai-chat",
                    auth: "api-key",
                    apiKey: "test",
                    compat: { supportsReasoningEffort: true },
                    models: [
                        { id: "gpt-5.5" },
                        { id: "gpt-5.4-mini" },
                        { id: "weird-custom", supportsXhigh: true },
                        { id: "gpt-5.5-pinned-off", supportsXhigh: false },
                    ],
                },
                local: {
                    compat: { supportsReasoningEffort: false },
                    models: [{ id: "llama" }],
                },
            },
        }"#,
    )
    .expect("write models");
    let dirs = tau_config::settings::TauDirs {
        config_dir: Some(dir.to_path_buf()),
        state_dir: None,
    };
    let registry = tau_config::settings::load_models_in(&dirs).expect("load");

    let with_xhigh = [L::Off, L::Minimal, L::Low, L::Medium, L::High, L::XHigh];
    let without_xhigh = [L::Off, L::Minimal, L::Low, L::Medium, L::High];

    assert_eq!(
        efforts_for_model(&registry, "openai/gpt-5.5"),
        with_xhigh,
        "whitelisted OpenAI model gets xhigh",
    );
    assert_eq!(
        efforts_for_model(&registry, "openai/gpt-5.4-mini"),
        without_xhigh,
        "mini variant excluded by whitelist",
    );
    assert_eq!(
        efforts_for_model(&registry, "openai/weird-custom"),
        with_xhigh,
        "explicit supportsXhigh=true opts in",
    );
    assert_eq!(
        efforts_for_model(&registry, "openai/gpt-5.5-pinned-off"),
        without_xhigh,
        "explicit supportsXhigh=false opts out",
    );
    assert_eq!(
        efforts_for_model(&registry, "local/llama"),
        vec![L::Off],
        "non-reasoning provider stays at Off-only",
    );
    assert!(
        efforts_for_model(&registry, "openai/unknown-id").last() == Some(&L::High),
        "unknown id falls back to the canonical 5-level set",
    );
    assert!(
        efforts_for_model(&registry, "").is_empty(),
        "empty model id yields no choices",
    );
}

/// `clamp_effort` must degrade `XHigh` to `High` (Pi-style) when the
/// model doesn't expose it, rather than silently dropping all the
/// way to `Off`. `Off` remains the fallback for other unsupported
/// levels so users with a no-reasoning provider don't get pinned to
/// a level the model can't handle.
#[test]
fn clamp_effort_degrades_xhigh_to_high_when_unsupported() {
    use tau_proto::Effort as L;
    let without_xhigh = [L::Off, L::Minimal, L::Low, L::Medium, L::High];

    assert_eq!(clamp_effort(L::XHigh, &without_xhigh), L::High);
    // Sanity: when xhigh IS allowed, no demotion.
    let with_xhigh = [L::Off, L::Minimal, L::Low, L::Medium, L::High, L::XHigh];
    assert_eq!(clamp_effort(L::XHigh, &with_xhigh), L::XHigh);
    // Other unsupported requests still fall to Off.
    assert_eq!(clamp_effort(L::Minimal, &[L::Off]), L::Off);
    // No Off in the allowed set: degrade to the first entry.
    assert_eq!(clamp_effort(L::High, &[L::Medium, L::Low]), L::Medium);
}
