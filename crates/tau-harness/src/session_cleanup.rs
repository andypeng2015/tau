//! Best-effort cleanup of old per-session state directories.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tau_core::list_session_metas;

pub(crate) fn spawn_session_cleanup(sessions_dir: PathBuf, retention: Option<Duration>) {
    let Some(retention) = retention else {
        return;
    };

    if let Err(error) = std::thread::Builder::new()
        .name("tau-session-cleanup".to_owned())
        .spawn(move || cleanup_old_sessions(sessions_dir, retention))
    {
        tracing::warn!(
            target: "tau_harness::session_cleanup",
            %error,
            "failed to spawn session cleanup thread"
        );
    }
}

pub(crate) fn cleanup_old_sessions(sessions_dir: PathBuf, retention: Duration) {
    let cutoff = unix_now().saturating_sub(retention.as_secs());
    let metas = match list_session_metas(&sessions_dir) {
        Ok(metas) => metas,
        Err(error) => {
            tracing::warn!(
                target: "tau_harness::session_cleanup",
                sessions_dir = %sessions_dir.display(),
                %error,
                "failed to list session metadata for cleanup"
            );
            return;
        }
    };

    for (session_id, meta) in metas {
        if cutoff < meta.last_touched {
            continue;
        }

        let path = sessions_dir.join(session_id.as_str());
        if let Err(error) = fs::remove_dir_all(&path) {
            tracing::warn!(
                target: "tau_harness::session_cleanup",
                session_id = %session_id,
                path = %path.display(),
                %error,
                "failed to remove old session directory"
            );
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
