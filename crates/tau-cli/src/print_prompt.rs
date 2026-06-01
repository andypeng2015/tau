use std::io::Write;

use tau_harness::SessionLaunchStatus;

use crate::daemon::{DaemonCliOverrides, daemon_output_for_session, resolve_daemon};
use crate::{CliError, mint_short_id};

pub(crate) fn run_print_prompt(
    role: &str,
    role_cli_overrides: &[tau_config::settings::RoleCliOverride],
    extension_cli_overrides: &[tau_config::settings::ExtensionCliOverride],
    harness_config_overrides: &[tau_config::settings::HarnessConfigCliOverride],
) -> Result<(), CliError> {
    let session_id = mint_short_id("print-prompt");
    let output = daemon_output_for_session(&session_id)?;
    let daemon = resolve_daemon(
        false,
        &session_id,
        SessionLaunchStatus::New,
        Some(output),
        Some(role),
        DaemonCliOverrides {
            role: role_cli_overrides,
            extension: extension_cli_overrides,
            harness_config: harness_config_overrides,
        },
    )?;

    let prompt = tau_harness::get_daemon_rendered_system_prompt(daemon.socket_path(), role)?;

    let mut stdout = std::io::stdout().lock();
    stdout.write_all(prompt.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
