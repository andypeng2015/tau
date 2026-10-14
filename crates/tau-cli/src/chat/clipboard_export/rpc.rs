//! One private, bounded request/response bridge on the admitted UI connection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Instant;

use tau_proto::{
    ArtifactOp, ArtifactRequest, ArtifactRequestId, ArtifactResult, ArtifactValue,
    HarnessInputMessage, SessionId,
};

use super::super::{WriterHandle, send_frame};

/// Off-loop RPC helper with at most one retained result.
pub(super) struct Rpc {
    /// Existing UI output stream; never another Hello or weaker authority path.
    writer: WriterHandle,
    /// Exact immutable attachment session.
    session: SessionId,
    /// One correlation and at most one result, published before notification.
    expected: Mutex<Option<(ArtifactRequestId, Option<ArtifactResult>)>>,
    /// Revocation shared with attachment teardown.
    alive: AtomicBool,
    /// Coalesced result, stop, and raw reservation retirement notification.
    pub(super) wake: mpsc::SyncSender<()>,
    /// Sole worker's event wait; predicates are checked after consuming a wake.
    receiver: Mutex<mpsc::Receiver<()>>,
}

impl Rpc {
    /// Binds a request bridge to one authenticated UI transport and session.
    pub(super) fn new(writer: WriterHandle, session: SessionId) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        Self {
            writer,
            session,
            expected: Mutex::new(None),
            alive: AtomicBool::new(true),
            wake,
            receiver: Mutex::new(receiver),
        }
    }

    /// Indicates whether attachment teardown has revoked new work.
    pub(super) fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Revokes requests and releases any retained response destination.
    pub(super) fn stop(&self) {
        self.alive.store(false, Ordering::Release);
        *self.expected.lock().expect("clipboard RPC correlation") = None;
        let _ = self.wake.try_send(());
    }

    /// Takes a matching result once, leaving unrelated callers' responses
    /// intact.
    pub(super) fn deliver(&self, result: ArtifactResult) -> Option<ArtifactResult> {
        let mut expected = self.expected.lock().expect("clipboard RPC correlation");
        if expected
            .as_ref()
            .is_some_and(|(id, reply)| *id == result.request_id && reply.is_none())
        {
            expected.as_mut().expect("matched correlation").1 = Some(result);
            let _ = self.wake.try_send(());
            None
        } else {
            Some(result)
        }
    }

    /// Sends and awaits one bounded request off-loop with revocable authority.
    pub(super) fn request(
        &self,
        op: ArtifactOp,
        deadline: Instant,
        live: impl Fn() -> bool,
    ) -> Result<ArtifactValue, &'static str> {
        if !self.alive() || !live() {
            return Err("Clipboard export cancelled");
        }
        if Instant::now() >= deadline {
            return Err("Artifact request timed out");
        }
        let request = self.frame(op);
        *self.expected.lock().expect("clipboard RPC correlation") =
            Some((request.request_id.clone(), None));
        let result = (|| {
            send_frame(&self.writer, &HarnessInputMessage::ArtifactRequest(request))
                .map_err(|_| "Artifact transport disconnected")?;
            let receiver = self.receiver.lock().expect("clipboard RPC waiter");
            loop {
                if !self.alive() || !live() {
                    return Err("Clipboard export cancelled");
                }
                if Instant::now() >= deadline {
                    return Err("Artifact request timed out");
                }
                let response = {
                    let mut expected = self.expected.lock().expect("clipboard RPC correlation");
                    if expected.as_ref().is_some_and(|(_, reply)| reply.is_some()) {
                        expected.take().and_then(|(_, reply)| reply)
                    } else {
                        None
                    }
                };
                if let Some(result) = response {
                    return result
                        .result
                        .map_err(|_| "Artifact unavailable or request rejected");
                }
                // Notifications are buffered/coalesced only after predicate
                // publication. A wake arriving between the checks and wait
                // remains pending; no periodic liveness polling is needed.
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(()) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        return Err("Artifact request timed out");
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("Artifact transport disconnected");
                    }
                }
            }
        })();
        *self.expected.lock().expect("clipboard RPC correlation") = None;
        result
    }

    /// Verifies the entire original and acknowledges Close before exposing
    /// bytes.
    pub(super) fn download(
        &self,
        key: tau_proto::ArtifactKey,
        deadline: Instant,
        live: impl Fn() -> bool,
    ) -> Result<Vec<u8>, &'static str> {
        let mut download = tau_client::ArtifactDownload::new(key);
        let result = (|| {
            while let Some(op) = download.next_op() {
                let value = self.request(op, deadline, &live)?;
                download
                    .accept(value)
                    .map_err(|_| "Artifact download failed verification")?;
            }
            Ok::<_, &'static str>(())
        })();
        if let Err(error) = result {
            if let Some(close) = download.close_op() {
                self.send_unobserved(close);
            }
            return Err(error);
        }
        download
            .into_bytes()
            .map_err(|_| "Artifact download was not verified and closed")
    }

    /// Best-effort close on cancellation; the harness also expires/disconnects
    /// reads.
    pub(super) fn send_unobserved(&self, op: ArtifactOp) {
        if self.alive() {
            let _ = send_frame(
                &self.writer,
                &HarnessInputMessage::ArtifactRequest(self.frame(op)),
            );
        }
    }

    fn frame(&self, op: ArtifactOp) -> ArtifactRequest {
        ArtifactRequest {
            request_id: crate::ui_client::next_request_id("ui-clipboard")
                .parse()
                .expect("generated artifact correlation"),
            expected_session_id: self.session.clone(),
            op,
        }
    }
}
