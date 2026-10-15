//! Narrow clipboard authority exposed to the UI's verified artifact consumer.

use std::sync::mpsc;
use std::time::Instant;

use crate::clipboard_export::ClipboardExport;
use crate::{InputMessage, RawEvent, TermHandle};

impl TermHandle {
    /// Reserves one export before download, bound to this live terminal owner.
    ///
    /// Capability is not permission: only a correlated terminal ACK confirms
    /// publication. Ctrl-C, focus/handoff and attachment shutdown revoke this
    /// ID.
    pub fn reserve_clipboard_export(&self) -> Result<u64, &'static str> {
        let mut st = self.lock();
        if st.terminal.external_paused
            || st.terminal.input_shutdown
            || st.terminal.shutdown
            || st.terminal.output_failure.is_some()
            || !st.editor.clipboard.native_available()
        {
            return Err(
                "Native clipboard export unavailable; terminal support and ownership required",
            );
        }
        if st.terminal.clipboard_export.is_some()
            || st.terminal.clipboard_export_inflight
            || st.editor.clipboard.busy()
            || st.editor.pending_paste.is_some()
        {
            return Err("Clipboard busy; wait or press Ctrl-C before exporting");
        }
        let export = ClipboardExport::new(Instant::now());
        let token = export.token;
        st.terminal.clipboard_export = Some(export);
        drop(st);
        self.request_completion_refresh(); // Wake the input owner's deadline wait.
        Ok(token)
    }

    /// Checks whether a download still belongs to a live, unexpired
    /// reservation.
    pub fn clipboard_export_live(&self, token: u64) -> bool {
        self.lock()
            .terminal
            .clipboard_export
            .as_ref()
            .is_some_and(|export| export.token == token && Instant::now() < export.deadline)
    }

    /// Registers the download owner's bounded retirement wake under the same
    /// lock that revokes the reservation. A retired token cannot register late.
    ///
    /// The receiver must recheck liveness after each wake; successful
    /// completion also retires the reservation. Only one observer may
    /// register.
    pub fn watch_clipboard_export(
        &self,
        token: u64,
        wake: mpsc::SyncSender<()>,
    ) -> Result<(), &'static str> {
        let mut st = self.lock();
        let export = st
            .terminal
            .clipboard_export
            .as_mut()
            .filter(|export| export.token == token && Instant::now() < export.deadline)
            .ok_or("Clipboard export reservation expired or was cancelled")?;
        if export.retired.is_some() {
            return Err("Clipboard export already has a retirement observer");
        }
        export.retired = Some(wake);
        Ok(())
    }

    /// Starts bounded asynchronous output only after complete verification and
    /// Close.
    ///
    /// Bytes and media declaration remain private and are never rendered or
    /// logged.
    pub fn supply_clipboard_export(
        &self,
        token: u64,
        bytes: Vec<u8>,
        mime: &str,
        plain_alias: bool,
    ) -> Result<(), &'static str> {
        let mut st = self.lock();
        let export = st
            .terminal
            .clipboard_export
            .as_mut()
            .filter(|export| export.token == token && Instant::now() < export.deadline)
            .ok_or("Clipboard export reservation expired or was cancelled")?;
        export.supply(bytes, mime, plain_alias)?;
        drop(st);
        self.notify_redraw();
        Ok(())
    }

    /// Streams a verified original as an explicit copied-file offer, never a
    /// raw MIME write. The caller must have completed verification and Close.
    pub fn supply_clipboard_file_export(
        &self,
        token: u64,
        bytes: Vec<u8>,
        name: &crate::ClipboardFileName,
    ) -> Result<(), &'static str> {
        let mut st = self.lock();
        let export = st
            .terminal
            .clipboard_export
            .as_mut()
            .filter(|export| export.token == token && Instant::now() < export.deadline)
            .ok_or("Clipboard export reservation expired or was cancelled")?;
        export.supply_file(bytes, name)?;
        drop(st);
        self.notify_redraw();
        Ok(())
    }

    /// Retires a failed download only if it still owns the reservation.
    pub fn fail_clipboard_export(&self, token: u64, notice: &'static str) {
        let mut st = self.lock();
        if !st
            .terminal
            .clipboard_export
            .as_ref()
            .is_some_and(|export| export.token == token)
        {
            return;
        }
        st.terminal.clipboard_export = None;
        drop(st);
        self.export_notice(notice);
    }

    /// Cancels locally without waiting for terminal output; admitted packets
    /// may finish.
    pub fn cancel_clipboard_export(&self) -> bool {
        let mut st = self.lock();
        let Some(export) = st.terminal.clipboard_export.take() else {
            return false;
        };
        let notice = export.cancelled_notice();
        st.editor.clipboard.probe(Instant::now());
        drop(st);
        self.queue_clipboard_control(b"\x1b[?5522l\x1b[?5522$p");
        self.export_notice(notice);
        true
    }

    /// Retires an expired download/write/ACK, without claiming native rollback.
    pub(super) fn expire_clipboard_export(&self) -> bool {
        let mut st = self.lock();
        if !st
            .terminal
            .clipboard_export
            .as_ref()
            .is_some_and(|export| Instant::now() >= export.deadline)
        {
            return false;
        }
        st.terminal.clipboard_export = None;
        st.editor.clipboard.probe(Instant::now());
        drop(st);
        self.queue_clipboard_control(b"\x1b[?5522l\x1b[?5522$p");
        self.export_notice("Clipboard export timed out; terminal outcome unknown.");
        true
    }

    /// Surfaces a fixed, content-free clipboard diagnostic through the input
    /// owner.
    pub fn export_notice(&self, notice: &'static str) {
        let _ = self
            .input_tx
            .send(InputMessage::Raw(RawEvent::ClipboardExportNotice(notice)));
    }

    /// Queues tiny lifecycle controls without blocking cancellation behind bulk
    /// I/O.
    pub(super) fn queue_clipboard_control(&self, bytes: &[u8]) {
        let mut st = self.lock();
        if st.terminal.external_paused || st.terminal.input_shutdown {
            return;
        }
        // Coalesced lifecycle controls cannot grow with an unresponsive
        // terminal.
        st.terminal.clipboard_control.clear();
        st.terminal.clipboard_control.extend_from_slice(bytes);
        drop(st);
        self.notify_redraw();
    }
}
