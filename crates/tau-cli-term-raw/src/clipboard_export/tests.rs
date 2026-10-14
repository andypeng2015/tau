use std::io::{self, Write};
use std::sync::{Arc, Mutex, mpsc};

use super::*;
use crate::{CursorShape, Event, RawEvent, Term, TermHandle};

/// Arms only synthetic capability state; tests never query the system
/// clipboard.
fn native(handle: &TermHandle) {
    let now = Instant::now();
    let mut st = handle.lock();
    st.editor.clipboard.probe(now);
    st.editor.clipboard.mode_report(true, now);
}

/// Ignores the reservation's ordinary wake event, retaining semantic outcomes.
fn next(term: &Term) -> Event {
    loop {
        let event = term.get_next_event().expect("virtual terminal event");
        if !matches!(event, Event::CompletionRefresh) {
            return event;
        }
    }
}

/// Every byte sequence, including empty and maximum-size originals, survives
/// continuous base64 chunking without middle padding or oversized packets.
#[test]
fn packets_preserve_originals_and_bound_each_write() {
    for size in [0, 1, 2, 4095, 4096, 8191, 16 * 1024 * 1024] {
        let bytes: Vec<_> = (0..size).map(|n| (n % 251) as u8).collect();
        let mut export = ClipboardExport::new(Instant::now());
        export
            .supply(bytes.clone(), "application/octet-stream", false)
            .expect("bounded original");
        let mut encoded = String::new();
        let mut packets = 0;
        while let Some(packet) = export.admit_packet() {
            assert!(packet.len() < 6144);
            let text = std::str::from_utf8(&packet).expect("ASCII OSC packet");
            assert!(text.starts_with("\x1b]5522;"));
            assert!(text.ends_with("\x1b\\"));
            if text.contains(":mime=") {
                let (_, data) = text
                    .split_once(":mime=")
                    .expect("MIME metadata")
                    .1
                    .split_once(';')
                    .expect("data separator");
                encoded.push_str(data.strip_suffix("\x1b\\").expect("OSC terminator"));
            }
            packets += 1;
        }
        assert!(packets >= 3); // Begin, MIME-bearing data (even empty), commit.
        assert_eq!(STANDARD.decode(encoded).expect("continuous base64"), bytes);
        assert!(export.cancelled_notice().contains("unknown"));
    }
}

/// Writer replies are independent from ingress, strictly correlated and
/// accepted at commit admission (including an ACK racing the local flush
/// return).
#[test]
fn acknowledgments_require_writer_identity_and_commit_admission() {
    let mut export = ClipboardExport::new(Instant::now());
    export
        .supply(b"hello".to_vec(), "text/plain;charset=utf-8", true)
        .expect("bounded text");
    let done = format!("5522;type=write:id={}:status=DONE", export.id);
    assert!(export.receive(done.as_bytes()).is_none());
    assert!(
        export
            .receive(b"5522;type=write:id=old:status=DONE")
            .is_none()
    );
    while export.admit_packet().is_some() {}
    assert_eq!(
        export.receive(done.as_bytes()),
        Some("Terminal acknowledged clipboard export.")
    );
    assert!(
        export
            .receive(format!("{done};payload").as_bytes())
            .is_none()
    );
    assert!(
        export
            .receive(format!("{done}:id={}", export.id).as_bytes())
            .is_none()
    );
    assert!(
        export
            .receive(done.replace("type=write", "type=read").as_bytes())
            .is_none()
    );
    assert!(
        export
            .receive(done.replace("DONE", "EPERM").as_bytes())
            .expect("correlated denial")
            .contains("denied")
    );
    export.deadline = Instant::now();
    assert!(export.receive(done.as_bytes()).is_none());
}

/// Fake output that blocks exactly one data packet, not display writes.
struct BlockData {
    /// Observed private output in this synthetic test only.
    bytes: Arc<Mutex<Vec<u8>>>,
    /// One-shot notification after writer packet admission.
    started: Option<mpsc::SyncSender<()>>,
    /// Test-controlled release, bounded to avoid a wedged test teardown.
    release: mpsc::Receiver<()>,
}

impl Write for BlockData {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes
            .windows(b"type=wdata".len())
            .any(|window| window == b"type=wdata")
            && let Some(started) = self.started.take()
        {
            let _ = started.send(());
            self.release
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| io::Error::other("synthetic gate expired"))?;
        }
        self.bytes
            .lock()
            .expect("captured output mutex")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Cancellation must return while output is blocked, keep the admitted OSC
/// intact, and suppress all later chunks/commit without allowing another
/// exporter to race.
#[test]
fn cancel_during_inflight_packet_never_admits_commit() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (started, entered) = mpsc::sync_channel(1);
    let (release, gate) = mpsc::sync_channel(1);
    let (_term, handle, _input) = Term::new_virtual(
        80,
        24,
        "> ",
        Box::new(BlockData {
            bytes: bytes.clone(),
            started: Some(started),
            release: gate,
        }),
        CursorShape::Bar,
    );
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    handle
        .supply_clipboard_export(token, vec![7; 20_000], "application/octet-stream", false)
        .expect("bounded original");
    entered
        .recv_timeout(Duration::from_secs(2))
        .expect("data admitted");
    assert!(handle.lock().terminal.clipboard_export_inflight);
    assert!(handle.cancel_clipboard_export());
    assert!(!handle.clipboard_export_live(token));
    native(&handle);
    assert!(handle.reserve_clipboard_export().is_err());
    release.send(()).expect("release blocked writer");
    handle.redraw_sync();
    assert!(!handle.lock().terminal.clipboard_export_inflight);
    let output = String::from_utf8(bytes.lock().expect("captured output mutex").clone())
        .expect("ASCII wire output");
    assert_eq!(output.matches("type=write:").count(), 1);
    assert_eq!(output.matches("type=wdata:").count(), 1);
    let data = output.split("type=wdata:").nth(1).expect("data packet");
    assert!(data.contains(":mime="));
    assert!(data.contains("\x1b\\"));
}

/// Focus, foreground pause and disconnect revoke slow source publication before
/// terminal output begins; none can resurrect the reserved download.
#[test]
fn lifecycle_revokes_reserved_downloads() {
    let (term, handle, input) =
        Term::new_virtual(80, 24, "> ", Box::new(io::sink()), CursorShape::Bar);
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    input
        .send(RawEvent::FocusChanged { focused: false })
        .expect("inject focus loss");
    assert!(matches!(
        next(&term),
        Event::FocusChanged { focused: false }
    ));
    assert!(
        handle
            .supply_clipboard_export(token, vec![], "application/octet-stream", false)
            .is_err()
    );
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    term.pause_for_external_with_release(|| Ok(()))
        .expect("synthetic handoff");
    assert!(!handle.clipboard_export_live(token));
    assert!(handle.reserve_clipboard_export().is_err());
    handle.lock().terminal.external_paused = false;
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    handle.request_input_shutdown();
    assert!(!handle.clipboard_export_live(token));
}

/// A native denial retires the write immediately and does not enter paste
/// routing.
#[test]
fn native_denial_is_a_directed_notice_without_fallback() {
    let (term, handle, input) =
        Term::new_virtual(80, 24, "> ", Box::new(io::sink()), CursorShape::Bar);
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    let id = handle
        .lock()
        .terminal
        .clipboard_export
        .as_ref()
        .expect("reserved export")
        .id
        .clone();
    input
        .send(RawEvent::Osc(
            format!("5522;type=write:id={id}:status=EPERM").into_bytes(),
        ))
        .expect("inject native denial");
    let Event::Notice(notice) = next(&term) else {
        panic!("notice")
    };
    assert!(notice.contains("denied"));
    assert!(!handle.clipboard_export_live(token));
}

/// Retirement wakes a registered waiter without polling, and registration can
/// never revive an already-cancelled reservation.
#[test]
fn retirement_registration_is_atomic_with_cancellation() {
    let (_term, handle, _input) =
        Term::new_virtual(80, 24, "> ", Box::new(io::sink()), CursorShape::Bar);
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    let (wake, receiver) = mpsc::sync_channel(1);
    handle
        .watch_clipboard_export(token, wake.clone())
        .expect("register observer");
    assert!(handle.watch_clipboard_export(token, wake.clone()).is_err());
    assert!(handle.cancel_clipboard_export());
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("retirement wakes observer");
    assert!(handle.watch_clipboard_export(token, wake.clone()).is_err());
    native(&handle);
    let next = handle.reserve_clipboard_export().expect("next reservation");
    assert!(handle.cancel_clipboard_export());
    assert!(handle.watch_clipboard_export(next, wake).is_err());
}

/// Fake writer that admits the final commit but blocks its flush return.
struct BlockCommit {
    /// One-shot notification after the commit's bytes enter the writer.
    entered: Option<mpsc::SyncSender<()>>,
    /// Test-controlled completion of the admitted flush.
    release: mpsc::Receiver<()>,
    /// Whether the current flush includes the final commit.
    commit: bool,
}

impl Write for BlockCommit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(bytes);
        self.commit |= text.contains("type=wdata:") && !text.contains(":mime=");
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.commit
            && let Some(entered) = self.entered.take()
        {
            let _ = entered.send(());
            self.release
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| io::Error::other("synthetic commit flush expired"))?;
        }
        Ok(())
    }
}

/// DONE may retire the reservation before commit flush returns, but a second
/// export must remain excluded until that admitted packet finishes.
#[test]
fn done_before_commit_flush_keeps_inflight_exclusion() {
    let (entered, started) = mpsc::sync_channel(1);
    let (release, gate) = mpsc::sync_channel(1);
    let (term, handle, input) = Term::new_virtual(
        80,
        24,
        "> ",
        Box::new(BlockCommit {
            entered: Some(entered),
            release: gate,
            commit: false,
        }),
        CursorShape::Bar,
    );
    native(&handle);
    let token = handle
        .reserve_clipboard_export()
        .expect("native reservation");
    let id = handle
        .lock()
        .terminal
        .clipboard_export
        .as_ref()
        .expect("reservation")
        .id
        .clone();
    handle
        .supply_clipboard_export(token, b"exact".to_vec(), "application/octet-stream", false)
        .expect("verified original");
    started
        .recv_timeout(Duration::from_secs(2))
        .expect("commit flush entered");
    input
        .send(RawEvent::Osc(
            format!("5522;type=write:id={id}:status=DONE").into_bytes(),
        ))
        .expect("synthetic DONE");
    assert!(matches!(next(&term), Event::Notice(notice) if notice.contains("acknowledged")));
    assert!(!handle.clipboard_export_live(token));
    assert!(handle.reserve_clipboard_export().is_err());
    release.send(()).expect("release commit flush");
    handle.redraw_sync();
    assert!(handle.reserve_clipboard_export().is_ok());
}
