use super::*;

#[test]
fn daemon_command_sets_and_clears_harness_config_override_env() {
    let override_ = tau_config::settings::HarnessConfigCliOverride {
        key: "session_retention_days".to_owned(),
        raw_value: "3".to_owned(),
    };
    let with_override = build_daemon_command(DaemonCommandSpec {
        tau_binary: Path::new("tau"),
        session_id: "session-1",
        session_status: SessionLaunchStatus::New,
        stdout: Stdio::null(),
        stderr: Stdio::null(),
        stdin: Stdio::null(),
        startup_role: None,
        cli_overrides: DaemonCliOverrides {
            role: &[],
            extension: &[],
            harness_config: std::slice::from_ref(&override_),
        },
    });
    assert!(with_override.get_envs().any(|(key, value)| {
        key == tau_harness::HARNESS_CONFIG_CLI_OVERRIDES_ENV && value.is_some()
    }));

    let without_override = build_daemon_command(DaemonCommandSpec {
        tau_binary: Path::new("tau"),
        session_id: "session-1",
        session_status: SessionLaunchStatus::New,
        stdout: Stdio::null(),
        stderr: Stdio::null(),
        stdin: Stdio::null(),
        startup_role: None,
        cli_overrides: DaemonCliOverrides {
            role: &[],
            extension: &[],
            harness_config: &[],
        },
    });
    assert!(without_override.get_envs().any(|(key, value)| {
        key == tau_harness::HARNESS_CONFIG_CLI_OVERRIDES_ENV && value.is_none()
    }));
}
