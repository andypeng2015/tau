# tau-shell-interaction

Shared codecs and UI-side state for extension-owned shell execution and path
completion. `tau-cli` and shell-like extensions use this crate; the harness does
not.

`shellux.request` and `shellux.reply` are transient opaque custom events. The
protocol requires explicit instance selection, authenticated delivery metadata,
and a process nonce. `Client` manages discovery, running snapshots, command
correlation, cancellation, and unconfirmed outcomes without execution retries.
Legacy shell DTOs are used only as local renderer data, never as live transport.

See [SPEC-extension-owned-shell-interaction](../../specs/SPEC-extension-owned-shell-interaction.md)
for the cross-component contract and `src/lib.rs` for payloads and limits.
