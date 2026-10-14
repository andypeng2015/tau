# SPEC-extension-activity-leases: Transient extension activity ownership

## Record justification

Configured-extension admission, ordinary event interception, runtime idle shutdown,
and extension-owned effect settlement jointly implement activity ownership, so no
single component can own its lifecycle contract.

## Ownership and acknowledgement

Configured extensions may acquire generic activity holds for opaque operation IDs.
The harness binds each hold to the authenticated connection incarnation and current
session, not to payload-claimed ownership, an attached UI, a tool, or shell syntax.
It interprets no operation content. The initial consumer is the idle-session busy
predicate; this API does not prevent explicit shutdown, crashes, or operation
timeouts.

Acquisition uses ordinary interceptable publication. An extension must receive a
live harness-attributed `Acquired` result before starting effects. A queued,
intercepted, dropped, or unanswered request is not an acknowledgement. Requests
and results are immutable under replacement. Duplicate acquire is idempotent while
held; release retires the ID, so delayed acquisition cannot resurrect completed
work. Unknown release also retires the ID. Each owner has bounded active holds
and retained operation IDs; capacity exhaustion rejects new operations, never
evicts idempotency records.

Release follows completion or cancellation and any owned context settlement.
Both successful and failed settlement allow release; silence remains unconfirmed.
UI detach does not release extension-owned holds. Disconnect removes all holds
and retired IDs belonging to that connection; replacement processes acquire anew.
Neither journals nor replay restore this process-local state.

There is deliberately no expiry or heartbeat. A connected buggy extension, a lost
acknowledgement after acquisition, or intercepted release can keep the session busy
indefinitely until the owner disconnects or the operator shuts it down. Ordinary
idle timing resumes with a fresh window after the last accepted work settles.
