//! Provider-local verified bytes and main-loop Artifact response correlation.
//!
//! Workers never read the transport or filesystem. The existing runtime loop
//! demultiplexes their bounded requests alongside image-generation uploads.

mod cache;
mod client;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

pub(crate) use client::Client;
use tau_client::{ArtifactClient, ClientHandle, ManualRuntimeWaker};
use tau_proto::{
    ArtifactError, ArtifactOp, ArtifactRequestId, ArtifactResult, ArtifactValue, SessionId,
};

/// Shared transfer admission bound, independent of cache residency.
const MAX_TRANSFERS: usize = 8;

/// Main-loop owner for worker RPC correlation and a process-local byte cache.
#[derive(Default)]
pub(crate) struct State {
    /// Verified encoded bytes and bounded single-flight ownership.
    cache: Arc<cache::Shared>,
    /// Sent operations, including canceled allocations awaiting late cleanup.
    requests: HashMap<ArtifactRequestId, Rpc>,
}

/// One worker request; dropped receivers never grant another worker its result.
pub(crate) struct Rpc {
    /// Exact session captured from the consuming prompt.
    session: SessionId,
    /// One Open, sequential Read, or Close operation.
    op: ArtifactOp,
    /// Private response mailbox, never part of semantic publication.
    reply: mpsc::SyncSender<Signal>,
    /// Sticky worker cancellation/deadline state.
    live: Arc<AtomicBool>,
}

/// Reply and cancellation share one bounded mailbox, preventing lost wakes.
pub(super) enum Signal {
    /// A correlated response retains allocation cleanup until consumed.
    Reply(Reply),
    /// Cancellation or shutdown became authoritative.
    Aborted,
}

/// Undelivered allocations close even when cancellation races a successful
/// channel send. Taking the value transfers cleanup to ArtifactDownload.
pub(super) struct Reply {
    /// Result remains armed for cleanup until the worker consumes it.
    value: Option<Result<ArtifactValue, ArtifactError>>,
    /// Original allocation session.
    session: SessionId,
    /// Existing outbound writer; never transport-read authority.
    handle: ClientHandle,
}

impl Reply {
    /// Transfers the result into the worker's download state machine.
    fn take(mut self) -> Result<ArtifactValue, ArtifactError> {
        self.value.take().expect("owned reply")
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(result) = self.value.take() {
            close_late_allocation(&self.session, result, &self.handle);
        }
    }
}

impl State {
    /// Creates a worker endpoint without granting transport-read authority.
    pub(crate) fn client(
        &self,
        tx: mpsc::Sender<crate::WorkerMessage>,
        waker: ManualRuntimeWaker,
    ) -> Client {
        Client::new(Arc::clone(&self.cache), tx, waker)
    }

    /// Admits one operation without blocking the input or cancellation loop.
    pub(crate) fn request(&mut self, rpc: Rpc, handle: &ClientHandle) {
        if !rpc.live.load(Ordering::Acquire)
            || (self.cache.closed.load(Ordering::Acquire)
                && !matches!(rpc.op, ArtifactOp::Close { .. }))
        {
            send_reply(rpc, Err(ArtifactError::Unavailable), handle);
            return;
        }
        if self.requests.len() >= MAX_TRANSFERS {
            // Local overload rejects immediately. Only an external Artifact
            // Busy response uses retry backoff: that service has no capacity
            // notification protocol.
            send_reply(rpc, Err(ArtifactError::Unavailable), handle);
            return;
        }
        match ArtifactClient::new(handle.clone()).start_request(rpc.session.clone(), rpc.op.clone())
        {
            Ok(id) => {
                self.requests.insert(id, rpc);
            }
            Err(_) => {
                send_reply(rpc, Err(ArtifactError::Unavailable), handle);
            }
        }
    }

    /// Returns unowned results to the image-tool demultiplexer unchanged.
    pub(crate) fn result(
        &mut self,
        result: ArtifactResult,
        handle: &ClientHandle,
    ) -> Option<ArtifactResult> {
        let Some(rpc) = self.requests.remove(&result.request_id) else {
            return Some(result);
        };
        if !tau_proto::artifact_frame_fits(&tau_proto::HarnessOutputMessage::ArtifactResult(
            Box::new(result.clone()),
        )) {
            close_late_allocation(&rpc.session, result.result, handle);
            send_reply(rpc, Err(ArtifactError::Invalid), handle);
            return None;
        }
        if rpc.live.load(Ordering::Acquire) && !self.cache.closed.load(Ordering::Acquire) {
            send_reply(rpc, result.result, handle);
        } else {
            close_late_allocation(&rpc.session, result.result, handle);
        }
        None
    }

    /// Revokes workers and evicts cached bytes; pending Opens retain late
    /// cleanup authority until transport closure.
    pub(crate) fn shutdown(&mut self) {
        self.cache.closed.store(true, Ordering::Release);
        if let Ok(mut cache) = self.cache.inner.lock() {
            cache.clear();
        }
        self.cache.notify();
        for rpc in self.requests.values() {
            let _ = rpc.reply.try_send(Signal::Aborted);
        }
    }
}

/// A failed send drops its armed reply and releases any returned allocation.
fn send_reply(rpc: Rpc, value: Result<ArtifactValue, ArtifactError>, handle: &ClientHandle) {
    let _ = rpc.reply.try_send(Signal::Reply(Reply {
        value: Some(value),
        session: rpc.session,
        handle: handle.clone(),
    }));
}

/// An Open can allocate after its waiter has canceled. The server's fixed read
/// lifetime remains the fallback if this best-effort Close cannot be admitted.
fn close_late_allocation(
    session: &SessionId,
    result: Result<ArtifactValue, ArtifactError>,
    handle: &ClientHandle,
) {
    if let Ok(ArtifactValue::Opened { read, .. }) = result {
        let _ = ArtifactClient::new(handle.clone())
            .start_request(session.clone(), ArtifactOp::Close { read });
    }
}
