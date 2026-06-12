# tau-cli-term architecture notes

`tau-cli-term` owns high-level prompt behavior: completion rule selection,
prompt shell actions, prompt-history search, and editor integration. The lower
`tau-cli-term-raw` crate owns raw terminal input state, rendering, redraw
suppression, and pause/resume of terminal mode. `tau-cli-picker` is the small
standalone picker UI used by configured commands.

## Subprocess ownership

Bounded subprocess execution lives in `src/bounded_command.rs`.

- Git/fuzzy completion helpers use `ProcessOwnership::ProcessGroup`: Tau bounds
  stdout and elapsed time, kills the process group on overflow, timeout, or
  inherited-pipe failures, but does not hand foreground terminal ownership to the
  child.
- User-configured `complete_with_command` and prompt shell actions use
  `ProcessOwnership::ForegroundProcessGroup`: Tau first releases raw terminal
  mode, starts the command in an owned process group, hands that group the
  controlling terminal foreground pgrp, then restores Tau's pgrp before
  raw-mode/redraw resume. `tcsetpgrp` is guarded against `SIGTTOU`.

## Git completion cache

Git file enumeration caches results per current directory. Positive results are
kept for the process/cwd snapshot. Negative results are short-lived only, so a
transient git failure, missing repository, or output-limit error does not disable
fuzzy git completion until Tau exits.
