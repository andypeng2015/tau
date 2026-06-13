//! Direct trusted host shell execution for Rhai scripts.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run one shell command to completion with bounded capture and timeout.
pub(crate) fn run_shell_command(
    command: String,
    cwd: Option<String>,
    timeout: Duration,
) -> serde_json::Value {
    let started = Instant::now();
    let mut command_builder = Command::new("sh");
    command_builder
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(cwd.unwrap_or_else(|| ".".to_owned()));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        #[allow(unsafe_code)]
        // SAFETY: `pre_exec` runs in the child after fork and before exec. The
        // closure only calls the async-signal-safe `setsid` libc function and
        // constructs an OS error from errno on failure; it does not touch shared
        // Rust state from the parent process.
        unsafe {
            command_builder.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = match command_builder.spawn() {
        Ok(child) => child,
        Err(error) => {
            return serde_json::json!({
                "success": false,
                "status": null,
                "signal": null,
                "timed_out": false,
                "termination_reason": "start_error",
                "output": format!("failed to start shell command: {error}"),
                "truncated": false,
                "valid_utf8": true,
            });
        }
    };

    let stdout = child.stdout.take().map(read_pipe_capped);
    let stderr = child.stderr.take().map(read_pipe_capped);
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                timed_out = true;
                kill_child_process_group(&mut child);
                break child.wait().ok();
            }
            Err(_) => break None,
        }
    };

    let out = stdout
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    let err = stderr
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    let mut bytes = out.bytes;
    let mut truncated = out.truncated;
    let mut valid_utf8 = out.valid_utf8;
    let mut output = out.text;
    if !err.text.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("[stderr]\n");
        output.push_str(&err.text);
    }
    bytes += err.bytes;
    truncated |= err.truncated;
    valid_utf8 &= err.valid_utf8;
    let total_lines = output.lines().count() as u64;
    let duration_seconds = started.elapsed().as_secs_f64();
    let status_code = status.as_ref().and_then(std::process::ExitStatus::code);
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.as_ref().and_then(|status| status.signal())
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let success = status
        .as_ref()
        .is_some_and(std::process::ExitStatus::success)
        && !timed_out;
    let termination_reason = if timed_out {
        "timeout"
    } else if signal.is_some() {
        "signal"
    } else if status.is_some() {
        "exit"
    } else {
        "wait_error"
    };
    serde_json::json!({
        "success": success,
        "status": status_code,
        "signal": signal,
        "timed_out": timed_out,
        "duration_seconds": duration_seconds,
        "termination_reason": termination_reason,
        "output": output,
        "truncated": truncated,
        "total_lines": if truncated { Some(total_lines) } else { None },
        "total_bytes": if truncated { Some(bytes as u64) } else { None },
        "valid_utf8": valid_utf8,
    })
}

fn kill_child_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        #[allow(unsafe_code)]
        // SAFETY: the child was started in its own session/process group with
        // pgid equal to its pid. Sending SIGKILL to `-pid` targets that group;
        // if this fails we fall back to killing the immediate child handle.
        unsafe {
            let pid = child.id() as i32;
            if libc::kill(-pid, libc::SIGKILL) == -1 {
                let _ = child.kill();
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[derive(Default)]
struct CapturedPipe {
    text: String,
    bytes: usize,
    truncated: bool,
    valid_utf8: bool,
}

fn read_pipe_capped<R: Read + Send + 'static>(
    mut pipe: R,
) -> std::thread::JoinHandle<CapturedPipe> {
    std::thread::spawn(move || {
        const MAX_CAPTURE_BYTES: usize = 512 * 1024;
        let mut captured = CapturedPipe {
            valid_utf8: true,
            ..Default::default()
        };
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    captured.bytes += n;
                    let room = MAX_CAPTURE_BYTES.saturating_sub(captured.text.len());
                    if room < n {
                        captured.truncated = true;
                    }
                    if room > 0 {
                        let take = room.min(n);
                        match std::str::from_utf8(&buf[..take]) {
                            Ok(s) => captured.text.push_str(s),
                            Err(_) => {
                                captured.valid_utf8 = false;
                                captured
                                    .text
                                    .push_str(&String::from_utf8_lossy(&buf[..take]));
                            }
                        }
                    }
                }
            }
        }
        captured
    })
}
