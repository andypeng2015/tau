# dpc-tau-client

`dpc-tau-client` provides the shared runtime used by standalone Tau extensions and
other harness protocol peers.

Generic activity lease requests/results are available as typed event payloads.
Subscribe to live results before sending acquire; require harness publisher
attribution and matching session, owner, operation, and action before effects.
Release after effect and context settlement. These transient, connection-owned
holds prevent idle shutdown only, have no expiry, and are not restored on replay.

It handles the extension startup sequence, typed configuration and event
dispatch, outbound protocol messages, manual event loops, and standard logging
setup. The runtime advertises the protocol revision exported by
`dpc-tau-proto`.

The crate's Rust API follows its Cargo package version. Tau's wire protocol has
an independent major/minor revision.

Live typed handler contexts and raw deliveries expose the harness-authenticated
`publisher`; historical/synthetic deliveries may omit it. Domain protocols should
use live-only handlers and validate publisher identity, session, and their own
request/incarnation correlation rather than trusting payload-local source labels.
Generic custom-event `agent_id` metadata controls private-agent diagnostics; it
does not route or interpret the payload.

Configured extensions can send `ExtPassiveContextAppendRequest` to append
attributed output to a loaded agent without activating inference. Observe its
correlated `ExtPassiveContextAppendResult`: sending is not a commit acknowledgement.
The same publisher/agent/operation ID and body is idempotent; changed content
under an existing ID fails. Failure or silence never authorizes repeating the
external operation that produced the output.
