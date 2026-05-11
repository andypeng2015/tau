//! Daemon runtime directory management.
//!
//! Each harness daemon gets its own directory under
//! `$XDG_RUNTIME_DIR/tau/{pid}/` containing:
//!
//! - `tau.sock` — Unix socket for client connections
//! - `tau.dir` — project root path (discovery marker)
//! - `tau.pid` — daemon process ID
//! - `tau.session_id` — bound session id (so `tau -a` can resume it)
//!
//! Finding `tau.dir` guarantees the socket is already bound (the marker
//! is written *after* binding the socket).

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

const SOCK_FILENAME: &str = "tau.sock";
const DIR_FILENAME: &str = "tau.dir";
const PID_FILENAME: &str = "tau.pid";
const SESSION_ID_FILENAME: &str = "tau.session_id";

/// Env var used to hand a parent→child readiness pipe fd from `tau` to
/// the spawned `tau ext harness` process. The child writes one byte and
/// closes the fd once the socket is bound and the runtime markers are
/// in place; the parent blocks on `read()` until that byte arrives, or
/// gets EOF if the child exited early.
pub const READY_FD_ENV: &str = "TAU_READY_FD";

/// Send the ready signal to the parent CLI if it passed a [`READY_FD_ENV`]
/// pipe fd. Always closes the fd after writing.
///
/// Returning early without writing causes the parent to see EOF and
/// report the harness as exited.
pub fn signal_ready_to_parent() {
    let Ok(fd_var) = std::env::var(READY_FD_ENV) else {
        return;
    };
    let Ok(fd) = fd_var.parse::<libc::c_int>() else {
        tracing::warn!(target: "tau_harness::startup", value = %fd_var, "ignoring malformed TAU_READY_FD");
        return;
    };
    let byte: u8 = 1;
    // Safety: `byte` is a stack-allocated u8 we read for exactly one
    // byte; `libc::write` does not retain the pointer past the call.
    #[allow(unsafe_code)]
    let written = unsafe { libc::write(fd, std::ptr::addr_of!(byte).cast(), 1) };
    if written != 1 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(target: "tau_harness::startup", fd, %err, "failed to signal ready to parent");
    }
    // Safety: `fd` was passed in by the parent and is owned by this
    // process; closing it once is correct.
    #[allow(unsafe_code)]
    unsafe {
        libc::close(fd);
    }
    // Future spawned processes must not inherit this env var (would
    // make them try to write to a now-closed fd or — worse — to whatever
    // unrelated thing has since taken that number).
    // Safety: called during single-threaded startup, before
    // extensions are spawned, so no concurrent env access can race.
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var(READY_FD_ENV);
    }
}

/// Returns the root runtime directory for all tau daemon instances.
#[must_use]
pub fn root_runtime_dir() -> PathBuf {
    dirs::runtime_dir()
        .map(|dir| dir.join("tau"))
        .unwrap_or_else(|| {
            let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
            PathBuf::from(format!("/tmp/tau-{user}"))
        })
}

/// Returns the socket path within a daemon directory.
#[must_use]
pub fn socket_path(daemon_dir: &Path) -> PathBuf {
    daemon_dir.join(SOCK_FILENAME)
}

/// Metadata for one daemon directory, created before entering the
/// daemon loop.
pub struct DaemonDir {
    path: PathBuf,
    project_root: PathBuf,
}

impl DaemonDir {
    /// Returns the path to this daemon directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the socket path.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        socket_path(&self.path)
    }

    /// Writes the project root marker. Must be called *after* the
    /// socket is bound.
    pub fn write_marker(&self) -> Result<(), std::io::Error> {
        std::fs::write(
            self.path.join(DIR_FILENAME),
            self.project_root.to_string_lossy().as_bytes(),
        )
    }

    /// Writes the PID file.
    pub fn write_pid(&self) -> Result<(), std::io::Error> {
        std::fs::write(self.path.join(PID_FILENAME), std::process::id().to_string())
    }

    /// Writes the bound session id so `tau -a` can join that
    /// specific session instead of minting a fresh one.
    pub fn write_session_id(&self, session_id: &str) -> Result<(), std::io::Error> {
        std::fs::write(self.path.join(SESSION_ID_FILENAME), session_id.as_bytes())
    }

    /// Removes the daemon directory.
    pub fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Reads the session id a running daemon at `daemon_dir` is bound to.
#[must_use]
pub fn read_session_id(daemon_dir: &Path) -> Option<String> {
    std::fs::read_to_string(daemon_dir.join(SESSION_ID_FILENAME))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Creates a new daemon directory for the current process.
pub fn prepare_daemon_dir(project_root: &Path) -> Result<DaemonDir, std::io::Error> {
    let pid = std::process::id();
    let path = root_runtime_dir().join(pid.to_string());
    std::fs::create_dir_all(&path)?;
    Ok(DaemonDir {
        path,
        project_root: project_root.to_path_buf(),
    })
}

/// Finds a running harness daemon for the given project root.
#[must_use]
pub fn find_harness_for_dir(project_root: &Path) -> Option<PathBuf> {
    let runtime_dir = root_runtime_dir();
    if !runtime_dir.exists() {
        return None;
    }

    let entries = std::fs::read_dir(&runtime_dir).ok()?;
    for entry in entries.flatten() {
        let pid_dir = entry.path();

        let dir_file = pid_dir.join(DIR_FILENAME);
        let stored_root = match std::fs::read_to_string(&dir_file) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if paths_equal(Path::new(stored_root.trim()), project_root) {
            if verify_harness_running(&pid_dir) {
                return Some(pid_dir);
            } else {
                let _ = std::fs::remove_dir_all(&pid_dir);
            }
        }
    }

    None
}

/// Verifies that a daemon is actually running by connecting to its
/// socket.
fn verify_harness_running(daemon_dir: &Path) -> bool {
    let sock = daemon_dir.join(SOCK_FILENAME);
    UnixStream::connect(sock).is_ok()
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a_canon), Ok(b_canon)) => a_canon == b_canon,
        _ => a == b,
    }
}
