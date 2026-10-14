# SPEC-extension-owned-shell-interaction: Explicit shell endpoints

## Record justification

Shell execution and environment-aware completion span terminal rule selection,
CLI endpoint state, extension execution, generic event transport, and passive
transcript publication, so no one component owns their end-to-end contract.

## Ownership

The UI requires `!instance command` or `!!instance command`, even when only one
executor is available. It completes configured instance names but never infers
an executor. The selected extension owns execution, cwd, cancellation, request
deduplication, and bounded running-state snapshots. Filesystem completion uses
that extension's host namespace and workdir, not the terminal host and not a
namespace implied by an arbitrary shell wrapper.

UI and extension endpoints exchange transient opaque custom events under
[SPEC-custom-extension-events](SPEC-custom-extension-events.md).
The harness neither decodes the endpoint payload nor correlates shell requests.
The shared `tau-shell-interaction` crate defines endpoint codecs and UI state;
the harness does not depend on it.

## Attribution and lifecycle

The UI subscribes before querying availability. Replies identify supported
features and a fresh process nonce, authenticated by generic delivery publisher
metadata. Requests target both the configured instance and its advertised nonce.
The extension accepts execution only from an authenticated attached UI, for its
current session and a valid loaded target when one is supplied.

Each endpoint validates session, original agent, request id, instance, and nonce.
The UI binds the advertiser to its authenticated connection generation, ignores
replayed endpoint traffic, and removes inventory on extension exit. Late or
identity-altered replies cannot settle another operation. Duplicate execution
ids never run twice within one extension process. Restart loses endpoint state
and invalidates old nonces; no journal replay resurrects an execution.

An acceptance timeout means **unconfirmed outcome**, not proof that execution
did not occur. Disconnect likewise leaves an unknown outcome. Neither condition
automatically resends execution. A later valid terminal may resolve an
unconfirmed operation. Running-state queries recover only still-active work,
not a durable execution ledger.
Before publishing acceptance or scheduling effects, the extension acquires a
generic [activity lease](SPEC-extension-activity-leases.md). It releases after
the `!!` terminal or after `!` passive-context settlement, including failure.
Quiet work therefore prevents idle shutdown without restoring shell awareness
to the harness. Acquisition failure never runs the command.
Running recovery emits separately scoped command rows followed by a content-free
completion. Each row carries the command's original agent privacy metadata,
even when the querying UI has no agent selected; a snapshot never mixes private
and ordinary command text in one unscoped payload.

## Context and history

After finishing `!`, the extension publishes its terminal observation, then
requests a bounded, attributed, nonactivating append to the captured agent under
[SPEC-passive-extension-context](SPEC-passive-extension-context.md).
The extension owns this request even if the initiating UI disconnects.
Multiple observing UIs do not append anything. `!!` submits no append.

OS effects, transient terminal delivery, and transcript commit are not atomic.
Interception can drop the terminal independently of the append. A failed append
means the command completed but context insertion failed; it never authorizes
execution retry. New historical output renders as a generic attributed entry.
A UI that already showed the matching live terminal suppresses its duplicate
live context entry, without suppressing history on a later attach.

Custom traffic uses ordinary debug treatment and generic agent privacy metadata;
`!!` means no model context, not confidential transport.
Historical first-party shell DTOs retain only the read contract in
[SPEC-shell-command-reports-and-canonical-facts](SPEC-shell-command-reports-and-canonical-facts.md).

## Completion

`Path` and `PathFuzzy` rules request asynchronous extension-owned candidates.
For bang commands only the named instance participates; ordinary prompt paths
merge advertised completers with visible instance labels. There is no local
filesystem fallback. Action, agent, session, custom terminal-owning completers,
and editor behavior stay independent.

Only the decoded token is transmitted, never the full draft. Endpoints bound
requests and results; the UI debounces edits, limits fanout, and enforces a reply
deadline. Draft, cursor, session, agent, instance incarnation, or mode changes
invalidate pending results. Accepted replacements preserve surrounding text and
UTF-8 boundaries, escape one path token, and never submit the prompt.
