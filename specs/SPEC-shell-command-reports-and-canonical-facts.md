# SPEC-shell-command-reports-and-canonical-facts: Historical shell facts

## Record justification

Historical shell DTO decoding, journal replay, and terminal rendering span
`tau-proto`, `tau-core`, `tau-harness`, and `tau-cli`, so no local artifact owns
their retained read-only interpretation.

## Historical interpretation

Old journals may contain `shell.command_finished` with the original command,
target agent, output, and exit or cancellation outcome. Replay preserves that
self-contained fact for the UI. Any accompanying `agent.user_message_injected`
already contains the historical model-visible text; replay must not inject it
again or execute the command.

The old `ui.shell_command`, `shell.command_progress`, and
`shell.command_finished` DTOs and their `_reported` forms remain decodable.
Live client and configured-extension intake rejects them. They cannot create new
semantic records. The harness has no shell provider selector, private route,
pending command map, report canonicalizer, output injector, or running snapshot.

Current behavior is governed by
[SPEC-extension-owned-shell-interaction](SPEC-extension-owned-shell-interaction.md).
New `!` history uses
[SPEC-passive-extension-context](SPEC-passive-extension-context.md), not the
historical shell terminal format.
