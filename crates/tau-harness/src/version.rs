//! Build metadata exposed to spawned processes (e.g. the shell
//! extension's child commands).
//!
//! The harness binary is compiled from the same workspace as
//! `tau-cli`, so it can read its own `built` snapshot rather than
//! relying on the parent CLI to forward version info via env vars.
//! [`export_to_env`] populates the `TAU_VERSION` / `TAU_BUILD` /
//! `TAU_LAST_MODIFIED` variables in the harness process's own
//! environment so subsequent `Command::new` calls inherit them.

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

/// Git revision the harness was built from. Suffixed with
/// `-modified` when the working tree was dirty at build time.
#[must_use]
pub fn build_revision() -> String {
    match (built_info::GIT_COMMIT_HASH_SHORT, built_info::GIT_DIRTY) {
        (Some(hash), Some(true)) => format!("{hash}-modified"),
        (Some(hash), _) => hash.to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// `YYYY-MM-DD HH:MM` of the build, with a Nix-packaging override
/// (`TAU_LAST_MODIFIED` at compile time) taking precedence over the
/// `built` timestamp. Returns `None` if no usable value is available.
#[must_use]
pub fn build_last_modified() -> Option<String> {
    option_env!("TAU_LAST_MODIFIED")
        .filter(|date| !date.is_empty())
        .map(str::to_owned)
        .or_else(|| short_built_time(built_info::BUILT_TIME_UTC))
        .filter(|date| date != "1980-01-01 00:00")
}

fn short_built_time(_time: &str) -> Option<String> {
    // The `time` crate isn't a dependency of tau-harness; we surface
    // the raw RFC2822 string when no Nix override is available. Code
    // paths that need formatted output (like the CLI banner) read
    // their own `built::BUILT_TIME_UTC` and format it locally.
    None
}

/// Publish version metadata into the current process's environment so
/// children spawned by extensions (e.g. shell commands) inherit it.
/// Existing values are preserved — the env-var-only invocation path
/// (`tau ext harness` launched without a parent CLI) still benefits
/// from values set externally, e.g. by integration tests.
pub fn export_to_env() {
    set_env_if_absent("TAU_VERSION", env!("CARGO_PKG_VERSION"));
    set_env_if_absent("TAU_BUILD", &build_revision());
    if let Some(date) = build_last_modified() {
        set_env_if_absent("TAU_LAST_MODIFIED", &date);
    }
}

fn set_env_if_absent(key: &str, value: &str) {
    if std::env::var_os(key).is_some() {
        return;
    }
    // Safety: called from `run_component` during single-threaded
    // startup before any extension subprocesses are spawned.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var(key, value);
    }
}
