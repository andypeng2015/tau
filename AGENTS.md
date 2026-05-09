## Workspace layout

- `crates/tau-proto` — shared protocol types and CBOR codec helpers
- `crates/tau-config` — user and project configuration loading
- `crates/tau-core` — event bus, routing, state, and tool registry
- `crates/tau-supervisor` — supervised child-process and stdio transport glue
- `crates/tau-test-support` — reusable end-to-end test utilities
- `crates/tau-socket` — Unix socket transport glue
- `crates/tau-harness` — harness daemon: extensions, bus, sessions, socket server
- `crates/tau-cli` — CLI entrypoint: starts harness daemon, connects as socket client
- `crates/tau-agent` — first-party agent process
- `crates/tau-ext-shell` — shell- and filesystem-oriented extension

## Design docs

- `ARCHITECTURE.md` — single binary design and crate layout
- `FEATURES.md` — update after editing any new major features
- `doc/ORIGINAL_DESIGN.md` — pre-implementation design exploration

## Getting started

- `cargo check`
- `nix develop`
- `selfci check`


## Definition of done

- Code is formatted.
- Relevant tests pass.
- Run `selfci check` after every major change.
- Update `FEATURES.md` after editing any new major features.
