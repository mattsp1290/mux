# 06 — tmux Behaviour

## Invocation contract

- Direct argv only. No shell (`sh -c`), no `$SHELL`, no `system()`.
- Binary: `tmux` on `PATH` (no hardcoded path).
- No global flags: `-L` (socket name) and `-g` (global options) are forbidden.
  All options must be session-scoped.
- No user config mutation: do not read or write `~/.tmux.conf`.

## Session naming

- All mux-managed sessions are prefixed `mux-` (e.g. `mux-myrepo-happy-panda`).
- The prefix is applied by the agent at create time; it is stored in `tmux_name`.
- `mux list` ignores sessions without the `mux-` prefix.

## Required tmux version

≥ 3.0 (verified during `mux host test`).

## Session creation argv

```
tmux new-session -d -s <tmux_name> -c <workdir>
```

Options applied after creation (session-scoped, NOT global):

```
tmux set-option -t <tmux_name> status on
tmux set-option -t <tmux_name> status-right "<status_string>"
```

Numeric status values: `set-option` accepts `on`/`off`/integers. Always use the
documented string form (`on`/`off`) for boolean options.

## Session listing argv

```
tmux list-sessions -F '#{session_name}\t#{session_created}\t#{session_activity}'
```

- Output is tab-separated.
- CRLF line endings must be handled (some tmux versions emit CRLF over SSH).
- Malformed rows must be tolerated (skipped with a debug log).
- Filter: only rows where `session_name` starts with `mux-`.
- Preserve duplicate session names (unexpected but must not panic).

## Session kill argv

```
tmux kill-session -t <tmux_name>
```

## Option ordering

For `set-option`: session-scoped options must be applied after `new-session` completes.
The order within a batch of `set-option` calls is undefined; they must not depend on
each other.

## mux- prefix filtering

Listing and status operations filter to `mux-`-prefixed sessions only. Non-prefixed
sessions are invisible to mux.

## Status bar

Session-scoped status bar settings are applied at create time. The status string
content is implementation-defined but must not include shell-special characters that
require quoting for argv safety.
