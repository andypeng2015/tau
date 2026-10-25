//! Scoped deletion exclusion for bounded off-loop work outside the journal
//! worker. Physical custody never preserves revoked semantic admission
//! authority.

use std::fs::File;
use std::sync::{Arc, Weak};
use std::time::Instant;

use super::identity::{LeaseIdentity, PersistenceLease, StreamIdentity};
use super::owner::{AdmissionState, PersistenceAdmissionError, Shared, validate_generation};
use super::worker::StreamLifecycle;

/// Non-owning registry state; removing it cannot close a descriptor under the
/// admission mutex.
#[derive(Default)]
pub(super) struct AgentIoState {
    /// Exact worker-prepared agent lock, never a pathname to reopen.
    pub(super) lock: Weak<File>,
    /// Scoped holders that must finish before a successful release.
    active: usize,
    /// Deterministic observation that release reached the nonzero-guard
    /// barrier.
    #[cfg(test)]
    pub(super) release_wait_observed: bool,
}

impl AgentIoState {
    /// Confirms no scope can retain physical custody at release publication.
    pub(super) fn is_idle(&self) -> bool {
        self.active == 0
    }
}

/// Private strong custody that cannot escape a synchronous worker-only closure.
struct AgentIoGuard {
    /// Exact owner and generation whose scoped count was admitted.
    identity: Arc<LeaseIdentity>,
    /// Bookkeeping remains available through revocation and worker exit.
    shared: Arc<Shared>,
    /// Existing deletion-exclusion handle; taken before notifying drained
    /// state.
    lock: Option<Arc<File>>,
}

impl PersistenceLease {
    /// Runs bounded synchronous storage work while retaining this exact agent's
    /// existing deletion-exclusion lock.
    ///
    /// Call only on an off-loop worker, with population bounded by its admitted
    /// job permits. The closure must not wait for the semantic worker, main
    /// loop, extension RPC, or completion consumption; release may be
    /// waiting for this scope. Acquire this scope before any artifact-store
    /// lock. Return completed metadata or bytes, not open handles or deferred
    /// I/O.
    ///
    /// Only physically Prepared Agent generations qualify. Revocation rejects
    /// new scopes immediately and invalidates the returned result if observed
    /// after the closure. Physical custody ends before return, including
    /// unwind; callers must still revalidate exact runtime and lease
    /// authority before publishing a semantic event. No journal handle or
    /// append authority is exposed, and no filesystem path is reopened.
    pub fn with_agent_io_guard<T>(
        &self,
        operation: impl FnOnce(&tau_proto::AgentId) -> T,
    ) -> Result<T, PersistenceAdmissionError> {
        let guard = AgentIoGuard::acquire(self)?;
        let StreamIdentity::Agent(agent_id) = self.stream() else {
            unreachable!("guard acquisition accepts only Agent streams");
        };
        let result = operation(agent_id);
        let authority = self.validate_agent_io_authority();
        drop(guard);
        authority?;
        Ok(result)
    }

    /// Revalidates exact Prepared Agent authority without I/O or physical
    /// custody.
    ///
    /// This is suitable for a runtime completion check, not protection for I/O;
    /// ordinary semantic admission remains the final generation check.
    pub fn validate_agent_io_authority(&self) -> Result<(), PersistenceAdmissionError> {
        let shared = self
            .identity
            .shared
            .upgrade()
            .ok_or(PersistenceAdmissionError::Unavailable)?;
        let state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        validate_agent_authority(&shared, &state, &self.identity)
    }
}

impl AgentIoGuard {
    /// Validates and acquires the existing handle at one admission-mutex cut.
    fn acquire(lease: &PersistenceLease) -> Result<Self, PersistenceAdmissionError> {
        let shared = lease
            .identity
            .shared
            .upgrade()
            .ok_or(PersistenceAdmissionError::Unavailable)?;
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        validate_agent_authority(&shared, &state, &lease.identity)?;
        let registered = state
            .streams
            .get_mut(lease.stream())
            .expect("validated generation");
        let active = registered
            .agent_io
            .active
            .checked_add(1)
            .ok_or(PersistenceAdmissionError::Full)?;
        let lock = registered
            .agent_io
            .lock
            .upgrade()
            .ok_or(PersistenceAdmissionError::Unavailable)?;
        // No fallible work follows strong acquisition while holding the mutex.
        registered.agent_io.active = active;
        drop(state);
        Ok(Self {
            identity: Arc::clone(&lease.identity),
            shared,
            lock: Some(lock),
        })
    }
}

impl Drop for AgentIoGuard {
    fn drop(&mut self) {
        // This may close the final descriptor after worker exit or poison.
        // It must happen off-loop and before taking the admission mutex.
        drop(self.lock.take());
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(registered) = state.streams.get_mut(&self.identity.stream)
            && registered.generation == self.identity.generation
        {
            registered.agent_io.active = registered
                .agent_io
                .active
                .checked_sub(1)
                .expect("one count per scoped guard");
        }
        drop(state);
        self.shared.wake.notify_all();
    }
}

/// Generic admission also allows reserved/creation states; physical I/O must
/// not.
fn validate_agent_authority(
    shared: &Shared,
    state: &AdmissionState,
    identity: &LeaseIdentity,
) -> Result<(), PersistenceAdmissionError> {
    validate_generation(
        state,
        shared.owner_epoch,
        identity.owner_epoch,
        &identity.stream,
        identity.generation,
    )?;
    if !matches!(identity.stream, StreamIdentity::Agent(_)) {
        return Err(PersistenceAdmissionError::StaleLease);
    }
    if state.streams[&identity.stream].lifecycle != StreamLifecycle::Prepared {
        return Err(PersistenceAdmissionError::NotPrepared);
    }
    Ok(())
}

/// Publishes only a weak reference to a worker-owned exact Agent lock.
pub(super) fn publish_agent_lock(shared: &Shared, identity: &LeaseIdentity, lock: &Arc<File>) {
    if !matches!(identity.stream, StreamIdentity::Agent(_)) {
        return;
    }
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(registered) = state.streams.get_mut(&identity.stream)
        && registered.generation == identity.generation
    {
        registered.agent_io.lock = Arc::downgrade(lock);
    }
}

/// Drains only the selected exact generations using the original release
/// deadline.
///
/// Scoped work must not depend on this worker or the waiting lifecycle caller.
pub(super) fn wait_for_agent_io(
    shared: &Shared,
    identities: &[Arc<LeaseIdentity>],
    deadline: Instant,
) -> Result<(), PersistenceAdmissionError> {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    loop {
        if !state.available {
            return Err(PersistenceAdmissionError::Unavailable);
        }
        if deadline <= Instant::now() {
            return Err(PersistenceAdmissionError::Lifecycle(
                "release deadline elapsed".to_owned(),
            ));
        }
        let mut active = false;
        for identity in identities {
            let registered = state
                .streams
                .get(&identity.stream)
                .filter(|registered| registered.generation == identity.generation)
                .ok_or(PersistenceAdmissionError::StaleLease)?;
            active |= registered.agent_io.active != 0 || registered.active_folds != 0;
        }
        if !active {
            return Ok(());
        }
        #[cfg(test)]
        {
            for identity in identities {
                if let Some(registered) = state.streams.get_mut(&identity.stream) {
                    registered.agent_io.release_wait_observed = true;
                }
            }
            shared.wake.notify_all();
        }
        let (next, _) = shared
            .wake
            .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|error| error.into_inner());
        state = next;
    }
}
