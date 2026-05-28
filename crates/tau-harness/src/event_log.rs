//! Thread-safe append-only in-memory event log used by client follower
//! threads for replay + live delivery.
//!
//! The log grows unbounded over a daemon's lifetime: entries are
//! never reclaimed. Followers poll via [`EventLog::get_next_from`]
//! and never block, so no condvar is needed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tau_proto::{ConnectionId, Event, EventLogSeq, UnixMicros};

/// One entry in the event log.
///
/// `recorded_at` is stamped by [`EventLog::append`] at the moment
/// the entry is created. It matches the value carried on the wire
/// `LogEvent` envelope and any value persisted to durable semantic
/// logs — sampling the clock here once and threading the same value
/// through every downstream observer keeps offline timing analyses
/// consistent with what live subscribers saw.
#[derive(Clone, Debug)]
pub(crate) struct LogEntry {
    pub seq: EventLogSeq,
    // Read by tests; live readers consult the wire envelope or the
    // durable record instead. Kept on the in-memory entry so future
    // replay paths that want to surface original timestamps don't
    // have to re-derive them.
    #[allow(dead_code)]
    pub recorded_at: UnixMicros,
    pub source: Option<ConnectionId>,
    pub event: Event,
}

struct EventLogInner {
    entries: BTreeMap<EventLogSeq, LogEntry>,
    next_seq: EventLogSeq,
}

/// Thread-safe append-only event log.
///
/// Consumers track their own position and call
/// [`EventLog::get_next_from`] in a loop. The log does not track
/// subscribers, nor does it prune itself.
pub(crate) struct EventLog {
    inner: Mutex<EventLogInner>,
}

impl EventLog {
    /// Creates an empty event log.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(EventLogInner {
                entries: BTreeMap::new(),
                next_seq: EventLogSeq::new(0),
            }),
        })
    }

    /// Reserves the next harness runtime event-log sequence.
    ///
    /// Most live events use [`EventLog::append`], which reserves a sequence and
    /// stores a replayable in-memory entry. Durable-history replay uses this
    /// lighter path: replayed transcript facts are already stored in agent
    /// logs, but their `LogEvent` envelopes still need fresh globally
    /// monotonic [`EventLogSeq`] values rather than reusing persisted
    /// per-agent/per-session sequences.
    pub(crate) fn reserve_seq(&self) -> EventLogSeq {
        let mut inner = self.inner.lock().expect("event log mutex poisoned");
        let seq = inner.next_seq;
        inner.next_seq = inner.next_seq.next();
        seq
    }

    /// Appends an event and returns its sequence number alongside the
    /// wall-clock timestamp stamped on the entry.
    ///
    /// Stamping happens here (the single chokepoint every event passes
    /// through on its way to the bus) so the value the wire `LogEvent`
    /// envelope carries, the value followers see on replay, and any
    /// value persisted to disk are all the same micros — offline
    /// timing analyses agree with what live consumers saw.
    pub(crate) fn append(
        &self,
        source: Option<ConnectionId>,
        event: Event,
    ) -> (EventLogSeq, UnixMicros) {
        let recorded_at = UnixMicros::now();
        let mut inner = self.inner.lock().expect("event log mutex poisoned");
        let seq = inner.next_seq;
        inner.next_seq = inner.next_seq.next();
        inner.entries.insert(
            seq,
            LogEntry {
                seq,
                recorded_at,
                source,
                event,
            },
        );
        (seq, recorded_at)
    }

    /// Returns the first entry with seq >= `from`, or `None` if no such
    /// entry exists yet.
    pub(crate) fn get_next_from(&self, from: EventLogSeq) -> Option<LogEntry> {
        let inner = self.inner.lock().expect("event log mutex poisoned");
        inner
            .entries
            .range(from..)
            .next()
            .map(|(_, entry)| entry.clone())
    }

    /// Returns the next runtime event-log sequence, which may be assigned to an
    /// appended entry or reserved for a durable-history replay envelope. Used
    /// by tests to assert that no event-log sequence was consumed across a
    /// section of code.
    #[cfg(test)]
    pub(crate) fn next_seq(&self) -> EventLogSeq {
        self.inner
            .lock()
            .expect("event log mutex poisoned")
            .next_seq
    }
}

#[cfg(test)]
mod tests;
