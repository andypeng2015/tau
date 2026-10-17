# SPEC-semantic-journal-writeback-durability: Semantic journal writeback durability

## Record justification

Semantic durability spans core framed storage and persistence workers, harness commit
and external-effect ordering, journal recovery, journal-derived checkpoints, and
lifecycle shutdown, so no one local artifact can own the complete contract.

Normal live paths validate records, reserve bounded nonblocking persistence
capacity, and perform required in-memory ordering and fold work without
filesystem I/O. Saturation or unavailable admission rejects before canonical
acceptance. Accepted jobs enter one ordered, bounded persistence stream;
canonical publication and runtime continuation do not wait for worker-side
open, lock, recovery, write, checkpoint replacement, or synchronization.

The persistence worker writes each complete
`[u64 little-endian length][CBOR payload]` frame in admitted journal order and
atomically replaces journal-derived checkpoints only after their covered frame.
It marks complete writes and newly required directory entries dirty for
coalesced synchronization. Bounded dirty state, generation and offset
watermarks prevent lost wakes and prevent a racing sync from covering later
writes. Storage failures are diagnosed and retried asynchronously without
retracting accepted facts.

A partial-write failure rolls back to the prior EOF. The journal remains usable
after an open, lock, or write failure that leaves no partial frame, or after a
successful rollback; failure to restore the live file poisons that journal. No
semantic durability barrier precedes provider, tool, peer, or other external
effects. Queue accounting includes queued and in-flight work and has no
unbounded fallback. Shutdown may request best-effort persistence or syncing but
must not wait without a bound.

Bounded off-loop image storage preparation may retain only an exact Prepared
Agent's existing deletion-exclusion lock through a scoped lease operation.
This is not journal or append authority: revocation remains immediate, and the
runtime revalidates the completed operation before ordinary canonical admission.
The registry owns no strong descriptor. Strong custody is acquired and dropped
off-loop, with physical release preceding drain notification and completion
delivery. Such work must not wait for the semantic worker, main-loop processing,
extension RPC, or completion consumption.

Lifecycle release closes admission before draining these scopes. Its one original
deadline covers queueing, accepted work, debt and guard drain; success requires
the scopes and worker handles to be released. Timeout does not promise unlock or
permit maintenance, and does not become an unbounded join. Fail-stop or poison
can revoke semantic authority while an already-running scope retains physical
deletion exclusion until the I/O unwinds. Normal live publication and its
asynchronous crash boundary remain unchanged.

A process, kernel, or power crash may lose recent admitted records, and may lose
or tear the recent written suffix even when an external effect survives. Under
the existing lock, recovery truncates only an incomplete
frame header or payload at EOF, rebuilds affected journal-derived state, and marks that
crash-tail repair dirty. Complete frames that fail decoding, source-shape,
sequence, or semantic validation fail closed byte-for-byte without rebuilding
from a prefix. An empty valid prefix is allowed. Recovery never automatically
resends uncertain external effects. Session manifest existence and creation time
are canonical state, not journal-derived state, and recovery does not reconstruct
them.

Debug `events.jsonl` remains a separate non-authoritative, bounded,
nonblocking, droppable diagnostic stream with no sync or shutdown-drain
guarantee.

The latency and crash-tail choice is governed by
[`GATE-asynchronous-journal-durability`](GATE-asynchronous-journal-durability.md).
The content-free tool-correlation records that use this writeback contract are
specified by
[`SPEC-durable-tool-observation-correlation`](SPEC-durable-tool-observation-correlation.md).
