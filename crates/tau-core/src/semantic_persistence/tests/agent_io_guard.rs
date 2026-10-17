//! Exact-generation lock custody, revocation, release and unwind oracles.

use fs2::FileExt as _;
use tau_proto::AgentId;

use super::*;
use crate::{PersistenceAdmissionError, PersistenceLease, StreamIdentity};

/// One fully written Agent generation; temporary storage outlives its owners.
struct AgentFixture {
    /// Unique semantic owner retained independently of the store.
    owner: Arc<SemanticPersistenceOwner>,
    /// Managed projections and the store's owner reference.
    store: AgentStore,
    /// Exact lease used by the off-loop scope.
    lease: PersistenceLease,
    /// Stable transcript identity.
    agent_id: AgentId,
    /// Private filesystem removed only after owner/store teardown.
    root: tempfile::TempDir,
}

impl AgentFixture {
    /// Creates a real prepared lock using the production persistence path.
    fn new(backend: Option<Arc<WriteFaultBackend>>) -> Self {
        let root = tempfile::tempdir().expect("state");
        let owner = Arc::new(
            match backend {
                Some(backend) => {
                    SemanticPersistenceOwner::with_test_backend(Default::default(), backend)
                }
                None => SemanticPersistenceOwner::new(Default::default()),
            }
            .expect("owner"),
        );
        let agent_id = AgentId::parse("guarded-agent").expect("agent");
        let mut store =
            AgentStore::open_managed(root.path().join("agents"), owner.clone()).expect("store");
        let lease = store.reserve_new_agent(agent_id.as_str()).expect("reserve");
        store
            .append_agent_event(agent_id.as_str(), None, started_event(&agent_id))
            .expect("create");
        assert_eq!(
            owner.wait_for_latest_durability_for_test(Duration::from_secs(2)),
            DurabilityBarrierOutcome::Durable
        );
        Self {
            owner,
            store,
            lease,
            agent_id,
            root,
        }
    }

    /// Returns only a diagnostic path for independent lock probes in tests.
    fn lock_path(&self) -> std::path::PathBuf {
        self.root
            .path()
            .join("agents")
            .join(self.agent_id.as_str())
            .join("lock")
    }

    /// Copies non-owning registry state without acquiring descriptor custody.
    fn weak_lock(&self) -> std::sync::Weak<File> {
        let shared = self.lease.identity.shared.upgrade().expect("live owner");
        let state = shared.state.lock().expect("state");
        state.streams[self.lease.stream()].agent_io.lock.clone()
    }
}

/// A worker paused inside its synchronous storage operation, not at completion
/// delivery.
struct HeldGuard {
    /// Releases the synthetic filesystem pause.
    finish: mpsc::SyncSender<()>,
    /// The scope's final authority result after custody is dropped.
    worker: thread::JoinHandle<Result<(), PersistenceAdmissionError>>,
}

impl HeldGuard {
    /// Waits for exact lock acquisition before returning control to the test.
    fn start(lease: &PersistenceLease) -> Self {
        let expected = lease.stream().clone();
        let lease = lease.clone();
        let (entered, received) = mpsc::sync_channel(1);
        let (finish, wait) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            lease.with_agent_io_guard(|agent_id| {
                entered.send(agent_id.clone()).expect("entered");
                wait.recv().expect("finish scoped work");
            })
        });
        let agent_id = received
            .recv_timeout(Duration::from_secs(2))
            .expect("acquired");
        assert_eq!(StreamIdentity::Agent(agent_id), expected);
        Self { finish, worker }
    }

    /// Ends physical work before inspecting the returned authority result.
    fn finish(self) -> Result<(), PersistenceAdmissionError> {
        self.finish.send(()).expect("finish");
        self.worker.join().expect("scope did not panic")
    }
}

/// Probes the real OS lock through an independent open file description.
fn assert_lock_held(path: &Path, held: bool) {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("lock probe");
    match file.try_lock_exclusive() {
        Ok(()) => assert!(!held, "physical exclusion was lost"),
        Err(error) => {
            assert!(held, "physical exclusion was not released: {error}");
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        }
    }
}

/// Waits for a positive worker barrier observation rather than relying on a
/// sleep.
fn wait_for_release_drain(lease: &PersistenceLease) {
    let shared = lease.identity.shared.upgrade().expect("live owner");
    let state = shared.state.lock().expect("state");
    let (state, _) = shared
        .wake
        .wait_timeout_while(state, Duration::from_secs(2), |state| {
            !state
                .streams
                .get(lease.stream())
                .is_some_and(|registered| registered.agent_io.release_wait_observed)
        })
        .expect("drain observation");
    assert!(
        state
            .streams
            .get(lease.stream())
            .is_some_and(|registered| registered.agent_io.release_wait_observed)
    );
}

/// Observes primary-worker relinquishment while a scoped holder still owns the
/// lock.
fn wait_for_holder_count(lock: &std::sync::Weak<File>, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while lock.strong_count() != count {
        assert!(
            Instant::now() < deadline,
            "lock ownership did not reach expected cut"
        );
        thread::yield_now();
    }
}

/// Rejection must happen before the caller's storage operation can have
/// effects.
fn assert_rejected_without_running(lease: &PersistenceLease) {
    let ran = AtomicBool::new(false);
    assert!(
        lease
            .with_agent_io_guard(|_| ran.store(true, Ordering::SeqCst))
            .is_err()
    );
    assert!(!ran.load(Ordering::SeqCst));
}

/// Release closes authority first and cannot report success while any physical
/// guard remains; registry removal owns no final descriptor.
#[test]
fn agent_io_guard_release_waits_for_physical_custody() {
    let fixture = AgentFixture::new(None);
    let weak = fixture.weak_lock();
    assert_eq!(weak.strong_count(), 1);
    let first = HeldGuard::start(&fixture.lease);
    let second = HeldGuard::start(&fixture.lease);
    assert_eq!(weak.strong_count(), 3);
    let owner = fixture.owner.clone();
    let lease = fixture.lease.clone();
    let (completed, received) = mpsc::sync_channel(1);
    let release = thread::spawn(move || {
        completed
            .send(owner.release(&[lease], Duration::from_secs(2)))
            .expect("release result");
    });
    wait_for_release_drain(&fixture.lease);
    assert_rejected_without_running(&fixture.lease);
    assert!(matches!(
        received.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_lock_held(&fixture.lock_path(), true);
    assert!(matches!(
        first.finish(),
        Err(PersistenceAdmissionError::StaleLease)
    ));
    assert!(matches!(
        received.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        second.finish(),
        Err(PersistenceAdmissionError::StaleLease)
    ));
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("release completed")
        .expect("release success");
    release.join().expect("release thread");
    assert_eq!(weak.strong_count(), 0);
    assert_lock_held(&fixture.lock_path(), false);
}

/// Reserved and queued Agent generations are not physically Prepared. Finishing
/// queued creation after Closing must not reopen either semantic or I/O
/// admission.
#[test]
fn agent_io_guard_creation_cannot_revive_closing_generation() {
    let root = tempfile::tempdir().expect("state");
    let backend = Arc::new(WriteFaultBackend::new());
    let owner = Arc::new(
        SemanticPersistenceOwner::with_test_backend(Default::default(), backend.clone())
            .expect("owner"),
    );
    let mut store =
        AgentStore::open_managed(root.path().join("agents"), owner.clone()).expect("store");
    let id = AgentId::parse("creation-cut").expect("agent");
    let lease = store.reserve_new_agent(id.as_str()).expect("reserve");
    assert_rejected_without_running(&lease);
    backend
        .lock_publication
        .hold_before_agent_lock_create
        .store(true, Ordering::SeqCst);
    store
        .append_agent_event(id.as_str(), None, started_event(&id))
        .expect("queued creation");
    backend.wait_until_pending_lock_create_held();
    assert_rejected_without_running(&lease);
    let release_owner = owner.clone();
    let release_lease = lease.clone();
    let release =
        thread::spawn(move || release_owner.release(&[release_lease], Duration::from_secs(2)));
    assert!(owner.wait_for_release_command_for_test(Duration::from_secs(2)));
    backend.hold_writes.store(true, Ordering::SeqCst);
    backend
        .lock_publication
        .hold_before_agent_lock_create
        .store(false, Ordering::SeqCst);
    backend
        .lock_publication
        .pending_lock_create_wake
        .notify_all();
    backend.wait_until_write_held();
    {
        let shared = lease.identity.shared.upgrade().expect("live owner");
        let state = shared.state.lock().expect("state");
        assert_eq!(
            state.streams[lease.stream()].lifecycle,
            StreamLifecycle::Closing
        );
    }
    assert_rejected_without_running(&lease);
    assert!(
        store
            .append_agent_event(id.as_str(), None, display_name_event(&id, "must reject"))
            .is_err()
    );
    backend.release_writes();
    release
        .join()
        .expect("release thread")
        .expect("accepted creation drains");
    assert_lock_held(&root.path().join("agents/creation-cut/lock"), false);
    let cold = AgentStore::open_lazy(root.path().join("agents")).expect("cold reader");
    assert_eq!(
        cold.agent_events(id.as_str())
            .expect("durable first frame")
            .len(),
        1
    );
}

/// A weak registry token cannot revive an old generation or be used for a
/// Session stream, while an independently prepared successor remains usable.
#[test]
fn agent_io_guard_requires_exact_prepared_agent_generation() {
    let mut fixture = AgentFixture::new(None);
    assert!(fixture.lease.is_same_generation_as(&fixture.lease.clone()));
    fixture
        .store
        .release_managed_agents(Duration::from_secs(2))
        .expect("release old");
    assert_rejected_without_running(&fixture.lease);
    fixture
        .store
        .prepare_existing_agent(fixture.agent_id.as_str())
        .expect("new generation");
    let current = fixture
        .store
        .managed_persistence_leases()
        .pop()
        .expect("current lease");
    assert!(!fixture.lease.is_same_generation_as(&current));
    assert_rejected_without_running(&fixture.lease);
    assert_eq!(
        current
            .with_agent_io_guard(|id| id.clone())
            .expect("current scope"),
        fixture.agent_id
    );
    let mut sessions =
        SessionStore::open_managed(fixture.root.path().join("sessions"), fixture.owner.clone())
            .expect("sessions");
    sessions
        .prepare_session("session", SessionPreparationMode::New)
        .expect("session");
    for lease in sessions.managed_persistence_leases("session") {
        assert_rejected_without_running(&lease);
    }
}

/// Poison revokes new scopes immediately but retains deletion exclusion until
/// already admitted storage I/O unwinds outside the semantic worker.
#[test]
fn agent_io_guard_preserves_exclusion_after_agent_poison() {
    let backend = Arc::new(WriteFaultBackend::new());
    let mut fixture = AgentFixture::new(Some(backend.clone()));
    let weak = fixture.weak_lock();
    let guard = HeldGuard::start(&fixture.lease);
    backend.fail_next_write.store(true, Ordering::SeqCst);
    backend.fail_next_truncate.store(true, Ordering::SeqCst);
    fixture
        .store
        .append_agent_event(
            fixture.agent_id.as_str(),
            None,
            display_name_event(&fixture.agent_id, "poison"),
        )
        .expect("accepted before poison");
    wait_for_holder_count(&weak, 1);
    assert!(matches!(
        fixture.lease.validate_agent_io_authority(),
        Err(PersistenceAdmissionError::Poisoned)
    ));
    assert_rejected_without_running(&fixture.lease);
    assert_lock_held(&fixture.lock_path(), true);
    assert!(matches!(
        guard.finish(),
        Err(PersistenceAdmissionError::Poisoned)
    ));
    assert_eq!(weak.strong_count(), 0);
    assert_lock_held(&fixture.lock_path(), false);
    fixture
        .owner
        .release(std::slice::from_ref(&fixture.lease), Duration::from_secs(2))
        .expect("release poisoned registration");
}

/// Fail-stop is an immediate authority cut, not permission to unlock physical
/// work already running under an exact generation.
#[test]
fn agent_io_guard_fail_stop_revokes_authority_without_early_unlock() {
    let fixture = AgentFixture::new(None);
    let weak = fixture.weak_lock();
    let guard = HeldGuard::start(&fixture.lease);
    fixture.owner.fail_stop();
    assert_rejected_without_running(&fixture.lease);
    wait_for_holder_count(&weak, 1);
    assert_lock_held(&fixture.lock_path(), true);
    assert!(matches!(
        guard.finish(),
        Err(PersistenceAdmissionError::Unavailable)
    ));
    assert_eq!(weak.strong_count(), 0);
    assert_lock_held(&fixture.lock_path(), false);
}

/// A timed-out release cannot grant maintenance or claim unlock, and the
/// owner's existing bounded Drop must not join a still-running image-I/O scope.
#[test]
fn agent_io_guard_release_timeout_and_owner_drop_remain_bounded() {
    let fixture = AgentFixture::new(None);
    let weak = fixture.weak_lock();
    let path = fixture.lock_path();
    let guard = HeldGuard::start(&fixture.lease);
    let started = Instant::now();
    assert!(
        fixture
            .owner
            .release_for_maintenance(
                std::slice::from_ref(&fixture.lease),
                Duration::from_millis(25)
            )
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        fixture
            .owner
            .claim_maintenance(std::slice::from_ref(&fixture.lease))
            .is_err()
    );
    assert_rejected_without_running(&fixture.lease);
    assert_lock_held(&path, true);
    let AgentFixture {
        store,
        owner,
        lease,
        root,
        ..
    } = fixture;
    drop(store);
    let started = Instant::now();
    drop(owner);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "Drop waited for image I/O"
    );
    wait_for_holder_count(&weak, 1);
    assert_rejected_without_running(&lease);
    assert_lock_held(&path, true);
    assert!(matches!(
        guard.finish(),
        Err(PersistenceAdmissionError::Unavailable)
    ));
    assert_lock_held(&path, false);
    drop(root);
}

/// Completion-channel backpressure happens after physical custody ends, so it
/// cannot deadlock synchronous lifecycle release.
#[test]
fn agent_io_guard_drops_before_completion_delivery() {
    let fixture = AgentFixture::new(None);
    let lease = fixture.lease.clone();
    let (scope_done, scope_finished) = mpsc::sync_channel(1);
    let (completion, consume) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        let result = lease.with_agent_io_guard(|_| "verified");
        scope_done.send(()).expect("scope ended");
        completion.send(result).expect("deliver");
    });
    scope_finished
        .recv_timeout(Duration::from_secs(2))
        .expect("scope finished");
    assert_eq!(fixture.weak_lock().strong_count(), 1);
    fixture
        .owner
        .release(std::slice::from_ref(&fixture.lease), Duration::from_secs(2))
        .expect("release while completion is blocked");
    assert_lock_held(&fixture.lock_path(), false);
    assert_eq!(
        consume.recv().expect("completion").expect("physical work"),
        "verified"
    );
    worker.join().expect("worker");
}

/// Failed or panicking storage work releases custody and its drain count before
/// returning control; ordinary storage errors do not poison semantic authority.
#[test]
fn agent_io_guard_releases_on_error_and_unwind() {
    let fixture = AgentFixture::new(None);
    let lease = fixture.lease.clone();
    assert_eq!(
        thread::spawn(move || lease.with_agent_io_guard(|_| Err::<(), _>("CAS failure")))
            .join()
            .expect("worker")
            .expect("valid lease"),
        Err("CAS failure")
    );
    let lease = fixture.lease.clone();
    assert!(
        thread::spawn(
            move || lease.with_agent_io_guard::<()>(|_| panic!("injected image worker panic"))
        )
        .join()
        .is_err()
    );
    assert!(fixture.lease.validate_agent_io_authority().is_ok());
    assert_eq!(fixture.weak_lock().strong_count(), 1);
    fixture
        .owner
        .release(std::slice::from_ref(&fixture.lease), Duration::from_secs(2))
        .expect("unwind drained custody");
    assert_lock_held(&fixture.lock_path(), false);
}

/// Queueing and prior semantic work consume the original release deadline; an
/// expired command must not start a fresh guard wait or grant maintenance
/// later.
#[test]
fn agent_io_guard_queued_release_does_not_restart_deadline() {
    let backend = Arc::new(WriteFaultBackend::new());
    let mut fixture = AgentFixture::new(Some(backend.clone()));
    let weak = fixture.weak_lock();
    let guard = HeldGuard::start(&fixture.lease);
    backend.hold_writes.store(true, Ordering::SeqCst);
    fixture
        .store
        .append_agent_event(
            fixture.agent_id.as_str(),
            None,
            display_name_event(&fixture.agent_id, "accepted before release"),
        )
        .expect("accepted write");
    backend.wait_until_write_held();
    assert!(
        fixture
            .owner
            .release_for_maintenance(std::slice::from_ref(&fixture.lease), Duration::ZERO,)
            .is_err()
    );
    assert_rejected_without_running(&fixture.lease);
    backend.release_writes();
    // This barrier sits behind the expired Release command. It must finish
    // while image I/O is still paused, rather than starting a new wait budget.
    assert_eq!(
        fixture
            .owner
            .wait_for_latest_durability_for_test(Duration::from_secs(2)),
        DurabilityBarrierOutcome::Durable,
    );
    let shared = fixture.lease.identity.shared.upgrade().expect("owner");
    assert_eq!(
        shared.state.lock().expect("state").streams[fixture.lease.stream()].lifecycle,
        StreamLifecycle::Closing,
    );
    assert!(
        fixture
            .owner
            .claim_maintenance(std::slice::from_ref(&fixture.lease))
            .is_err()
    );
    assert!(matches!(
        guard.finish(),
        Err(PersistenceAdmissionError::StaleLease)
    ));
    assert_eq!(
        weak.strong_count(),
        1,
        "failed release did not claim unlock"
    );
    fixture.owner.fail_stop();
    wait_for_holder_count(&weak, 0);
    assert_lock_held(&fixture.lock_path(), false);
}

/// Numeric generation reuse by a different owner is not the same capability.
#[test]
fn agent_io_guard_rejects_old_owner_even_when_generation_number_matches() {
    let mut fixture = AgentFixture::new(None);
    fixture
        .store
        .release_managed_agents(Duration::from_secs(2))
        .expect("release");
    let successor = Arc::new(SemanticPersistenceOwner::new(Default::default()).expect("successor"));
    let mut store =
        AgentStore::open_managed(fixture.root.path().join("agents"), successor).expect("store");
    store
        .prepare_existing_agent(fixture.agent_id.as_str())
        .expect("resume");
    let lease = store
        .managed_persistence_leases()
        .pop()
        .expect("successor lease");
    assert_eq!(fixture.lease.generation(), lease.generation());
    assert!(!fixture.lease.is_same_generation_as(&lease));
    assert_rejected_without_running(&fixture.lease);
    assert_eq!(
        lease
            .with_agent_io_guard(|id| id.clone())
            .expect("successor scope"),
        fixture.agent_id
    );
}

/// Expiry or fail-stop after guard drain and physical close still prevents
/// Released publication and maintenance. The pause models slow close or worker
/// descheduling without leaving a descriptor under the admission mutex.
#[test]
fn agent_io_guard_final_release_cut_rechecks_deadline_and_availability() {
    for expire in [true, false] {
        let fixture = AgentFixture::new(None);
        let guard = HeldGuard::start(&fixture.lease);
        fixture
            .owner
            .arm_preparation_pause_for_test(PreparationPauseCut::ReleaseHandlesDropped);
        let owner = fixture.owner.clone();
        let lease = fixture.lease.clone();
        let (done, completion) = mpsc::sync_channel(1);
        let release = thread::spawn(move || {
            done.send(owner.release_for_maintenance(&[lease], Duration::from_secs(2)))
                .expect("release outcome");
        });
        wait_for_release_drain(&fixture.lease);
        assert!(matches!(
            guard.finish(),
            Err(PersistenceAdmissionError::StaleLease)
        ));
        assert!(
            fixture
                .owner
                .wait_for_preparation_pause_for_test(Duration::from_secs(2))
        );
        assert_eq!(fixture.weak_lock().strong_count(), 0);
        assert_lock_held(&fixture.lock_path(), false);
        if expire {
            assert!(
                completion
                    .recv_timeout(Duration::from_secs(3))
                    .expect("original deadline")
                    .is_err()
            );
        } else {
            fixture.owner.fail_stop();
        }
        fixture.owner.release_preparation_pause_for_test();
        if expire {
            assert_eq!(
                fixture
                    .owner
                    .wait_for_latest_durability_for_test(Duration::from_secs(2)),
                DurabilityBarrierOutcome::Durable,
                "expired release finished before this barrier",
            );
        } else {
            assert!(matches!(
                completion
                    .recv_timeout(Duration::from_secs(2))
                    .expect("revoked outcome"),
                Err(PersistenceAdmissionError::Unavailable)
            ));
            assert!(
                fixture
                    .owner
                    .wait_for_worker_exit_for_test(Duration::from_secs(2))
            );
        }
        release.join().expect("release caller");
        let shared = fixture.lease.identity.shared.upgrade().expect("owner");
        assert_eq!(
            shared.state.lock().expect("state").streams[fixture.lease.stream()].lifecycle,
            StreamLifecycle::Closing,
        );
        assert!(
            fixture
                .owner
                .claim_maintenance(std::slice::from_ref(&fixture.lease))
                .is_err()
        );
    }
}
