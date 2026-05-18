//! Loading and resolving harness/extension configuration on startup.
//!
//! Owns the resolved-configuration types ([`Config`], [`CoreConfig`],
//! [`CoreMode`], [`ExtensionConfig`]), and the resolver that turns
//! [`tau_config::settings::HarnessSettings`] into something the harness can
//! spawn. The wire schema and built-in defaults for `harness.ncl` live in
//! `tau-config`.

use std::collections::BTreeMap;
use std::fmt;

use tau_config::settings::{ExtensionEntry, HarnessSettings, LoadedHarnessSettings};
use tau_proto::CborValue;

/// The resolved harness configuration handed to the daemon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// Core harness mode selection.
    pub core: CoreConfig,
    /// Resolved extensions keyed by configured extension name.
    pub extensions: BTreeMap<String, ExtensionConfig>,
}

/// Resolved core configuration values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoreConfig {
    /// Whether the harness runs embedded in the current process or as a daemon.
    pub mode: CoreMode,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            mode: CoreMode::Embedded,
        }
    }
}

/// Minimal runtime mode selection for the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreMode {
    /// Run the harness in-process.
    Embedded,
    /// Connect to a long-running daemon.
    Daemon,
}

/// One configured extension process, after merging built-in defaults
/// and user overrides. Ready to spawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionConfig {
    /// Configured extension name.
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to [`Self::command`].
    pub args: Vec<String>,
    /// Optional extension role tag, such as `provider` or `tool`.
    pub role: Option<String>,
    /// CBOR-compatible config object handed to the extension via
    /// `LifecycleConfigure`. Defaults to an empty object so
    /// extensions always see a value.
    pub config: CborValue,
}

/// Error returned by [`resolve_extensions`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveExtensionsError {
    /// An enabled extension entry has no `command` and no `suffix`
    /// to piggyback on the running tau executable.
    EmptyCommand(String),
}

impl fmt::Display for ResolveExtensionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCommand(name) => write!(
                f,
                "extension {name:?} has no `command` set; entries must specify an executable or a tau subcommand suffix",
            ),
        }
    }
}

impl std::error::Error for ResolveExtensionsError {}

/// Resolve configured `extensions` entries into a flat list of
/// [`ExtensionConfig`]s ready for the harness to spawn.
///
/// Built-in extension defaults are ordinary entries in
/// `tau-config/config/built-in.harness.ncl`; Nickel layering has already
/// applied user overrides before this function runs.
///
/// Per-entry rules:
/// - Entries with a resolved `enable: false` are dropped.
/// - `enable` defaults to `true`.
/// - `config` defaults to an empty CBOR map.
/// - Entries with an absent `command` but a non-empty `suffix` piggyback on the
///   currently running tau executable. This is how built-ins select in-binary
///   extension subcommands without hard-coding an install path in Nickel.
/// - Entries with a `command` use only that command; `suffix` is ignored.
///   Include any additional arguments directly in `command`.
/// - Entries with no `command` and no `suffix` are rejected because there is no
///   executable to spawn.
///
/// Output is sorted by extension name for deterministic behavior; built-ins
/// have no separate semantic declaration order.
pub fn resolve_extensions(
    settings: &HarnessSettings,
) -> Result<Vec<ExtensionConfig>, ResolveExtensionsError> {
    let tau_binary = current_tau_executable();
    let mut out = Vec::new();

    let mut names: Vec<&String> = settings.extensions.keys().collect();
    names.sort();
    for name in names {
        let entry: &ExtensionEntry = &settings.extensions[name];
        if !entry.enable.unwrap_or(true) {
            continue;
        }

        let command = match entry.command.clone() {
            Some(command) => command,
            None => {
                let suffix = entry.suffix.clone().unwrap_or_default();
                if suffix.is_empty() {
                    return Err(ResolveExtensionsError::EmptyCommand(name.clone()));
                }
                let mut argv = entry.prefix.clone().unwrap_or_default();
                argv.push(tau_binary.clone());
                argv.extend(suffix);
                let (program, args) = argv
                    .split_first()
                    .expect("argv contains at least the tau binary");
                out.push(ExtensionConfig {
                    name: name.clone(),
                    command: program.clone(),
                    args: args.to_vec(),
                    role: entry.role.clone(),
                    config: entry.config.clone().unwrap_or_else(empty_cbor_map),
                });
                continue;
            }
        };

        let mut argv = entry.prefix.clone().unwrap_or_default();
        argv.extend(command);
        let (program, args) = match argv.split_first() {
            Some((first, rest)) => (first.clone(), rest.to_vec()),
            None => return Err(ResolveExtensionsError::EmptyCommand(name.clone())),
        };
        out.push(ExtensionConfig {
            name: name.clone(),
            command: program,
            args,
            role: entry.role.clone(),
            config: entry.config.clone().unwrap_or_else(empty_cbor_map),
        });
    }
    Ok(out)
}

fn current_tau_executable() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "tau".to_owned())
}

fn empty_cbor_map() -> CborValue {
    CborValue::Map(Vec::new())
}

pub(crate) fn load_harness_settings_or_warn(
    dirs: &tau_config::settings::TauDirs,
) -> Result<HarnessSettings, tau_config::settings::SettingsError> {
    tau_config::settings::load_harness_settings_in(dirs)
}

pub(crate) fn load_harness_settings_with_source_or_warn(
    dirs: &tau_config::settings::TauDirs,
) -> Result<LoadedHarnessSettings, tau_config::settings::SettingsError> {
    tau_config::settings::load_harness_settings_with_source_in(dirs)
}

#[must_use]
pub fn default_config() -> Config {
    // `resolve_extensions` is fallible only for enabled entries without both
    // `command` and `suffix`. Tau's built-in harness config gives every
    // enabled built-in a suffix, so the failure path is unreachable.
    let extensions = match resolve_extensions(&HarnessSettings::built_in()) {
        Ok(extensions) => extensions,
        Err(err) => unreachable!("built-in extensions resolve cleanly: {err}"),
    };

    Config {
        core: CoreConfig {
            mode: CoreMode::Embedded,
        },
        extensions: extensions
            .into_iter()
            .map(|extension| (extension.name.clone(), extension))
            .collect(),
    }
}

pub(crate) fn resolve_config(
    _explicit_path: Option<&std::path::Path>,
) -> Result<Config, Box<dyn std::error::Error>> {
    // Extensions live in `harness.ncl` under `extensions = { ... }`. Built-in
    // provider/tool defaults are part of tau-config's built-in harness Nickel;
    // malformed Nickel fails startup rather than silently discarding user
    // configuration.
    let settings = load_harness_settings_or_warn(&tau_config::settings::TauDirs::default())?;
    let extensions = resolve_extensions(&settings)?;
    Ok(Config {
        core: CoreConfig {
            mode: CoreMode::Embedded,
        },
        extensions: extensions
            .into_iter()
            .map(|extension| (extension.name.clone(), extension))
            .collect(),
    })
}

#[cfg(test)]
mod tests;
