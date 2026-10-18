//! Independent byte/entry residency bounds and concurrent-fetch ownership.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use tau_proto::ArtifactKey;

#[cfg(test)]
mod tests;
/// A cache owns at most this many encoded bytes, not decoded pixels or tokens.
pub(super) const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Small objects cannot grow the cache's metadata without bound.
const MAX_CACHE_ENTRIES: usize = 64;

/// Shared process-local resolver state; no lock is held across RPC or decoding.
#[derive(Default)]
pub(super) struct Shared {
    /// Cache residency and distinct fetch ownership under one short lock.
    pub(super) inner: Mutex<Cache>,
    /// Transport/session shutdown revokes every worker endpoint.
    pub(super) closed: AtomicBool,
}

impl Shared {
    /// Subscribes before checking the cache predicate, so completion cannot be
    /// lost between that check and the worker's wait.
    pub(super) fn subscribe(
        self: &Arc<Self>,
    ) -> Result<(Waiter, mpsc::SyncSender<()>, mpsc::Receiver<()>), tau_proto::ArtifactError> {
        let (send, receive) = mpsc::sync_channel(1);
        let mut cache = self
            .inner
            .lock()
            .map_err(|_| tau_proto::ArtifactError::Unavailable)?;
        let id = cache.next_waiter;
        cache.next_waiter = id
            .checked_add(1)
            .ok_or(tau_proto::ArtifactError::Unavailable)?;
        cache.waiters.insert(id, send.clone());
        Ok((
            Waiter {
                shared: Arc::clone(self),
                id,
            },
            send,
            receive,
        ))
    }

    /// Coalesces completion/shutdown wakes without holding a cache lock while
    /// notifying worker-owned channels.
    pub(super) fn notify(&self) {
        let senders: Vec<_> = self
            .inner
            .lock()
            .map(|cache| cache.waiters.values().cloned().collect())
            .unwrap_or_default();
        for send in senders {
            let _ = send.try_send(());
        }
    }
}

/// Verified byte residency is separate from outstanding transfer allocations.
#[derive(Default)]
pub(super) struct Cache {
    /// Immutable digest-verified encoded payloads.
    entries: HashMap<ArtifactKey, Entry>,
    /// Total bytes charged to the cache, excluding outstanding consumer Arcs.
    bytes: usize,
    /// LRU order; saturating age cannot wrap a new entry into ancient state.
    age: u64,
    /// Only these keys own bounded physical downloads.
    pub(super) fetching: HashSet<ArtifactKey>,
    /// One coalesced wake slot per currently waiting bounded execution worker.
    waiters: HashMap<u64, mpsc::SyncSender<()>>,
    /// Monotonic subscription identity, never reused after wraparound.
    next_waiter: u64,
}

/// One retained immutable object with its cache-local recency.
struct Entry {
    /// Verified encoded bytes; consumers share rather than copy.
    bytes: Arc<[u8]>,
    /// Last cache access order.
    age: u64,
}

impl Cache {
    /// Returns verified bytes and refreshes only local eviction order.
    pub(super) fn get(&mut self, key: &ArtifactKey) -> Option<Arc<[u8]>> {
        self.age = self.age.saturating_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.age = self.age;
        Some(Arc::clone(&entry.bytes))
    }

    /// Evicts before insertion; live requests may still own their bounded input
    /// Arcs without turning those into cache residency.
    pub(super) fn insert(&mut self, key: ArtifactKey, bytes: Arc<[u8]>) {
        if bytes.len() > MAX_CACHE_BYTES || self.entries.contains_key(&key) {
            return;
        }
        while self.entries.len() >= MAX_CACHE_ENTRIES
            || self.bytes.saturating_add(bytes.len()) > MAX_CACHE_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.age)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes -= entry.bytes.len();
            }
        }
        self.age = self.age.saturating_add(1);
        self.bytes += bytes.len();
        self.entries.insert(
            key,
            Entry {
                bytes,
                age: self.age,
            },
        );
    }

    /// Eviction does not revoke active fetch owners or transcript roots.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

/// Releases single-flight ownership on success, failure, cancellation or
/// unwind.
pub(super) struct Flight {
    /// Shared bounded registry.
    pub(super) shared: Arc<Shared>,
    /// Exact digest claimed by this worker.
    pub(super) key: ArtifactKey,
}

impl Drop for Flight {
    fn drop(&mut self) {
        if let Ok(mut cache) = self.shared.inner.lock() {
            cache.fetching.remove(&self.key);
        }
        self.shared.notify();
    }
}

/// Removes a worker's cache subscription on every success/error/unwind path.
pub(super) struct Waiter {
    /// Shared short-lock cache owner.
    shared: Arc<Shared>,
    /// Exact subscription identity.
    id: u64,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if let Ok(mut cache) = self.shared.inner.lock() {
            cache.waiters.remove(&self.id);
        }
    }
}
