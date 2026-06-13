# tmux Invocation Contract

Spec: docs/06-tmux-behavior.md  
Status: Active  
Linked from: prompts/docs/README.md

## Binary invocation rules

- Invoke `tmux` via direct argv only. Never via `sh -c`, `$SHELL`, or `system()`.
- Binary name: `tmux` on `PATH` — no hardcoded absolute path.
- Forbidden flags: `-L` (socket name) and `-g` (global options). All options must be session-scoped.
- Do not read or write `~/.tmux.conf` or any user config file.

## Required tmux version

≥ 3.0. Verified during `mux host test` using `tmux -V`. Sessions must not be created on hosts where the version check has not run or returned < 3.0.

## Session naming convention

All mux-managed sessions carry the `mux-` prefix. The agent applies this at create time.
The full name is stored in `sessions.tmux_name`. Examples: `mux-myrepo-happy-panda`.

`mux list` and all status/kill operations filter to `mux-`-prefixed sessions. Non-prefixed sessions are invisible to mux.

## Create session argv

```
tmux new-session -d -s <tmux_name> -c <workdir>
```

Post-creation options (session-scoped, NOT global):

```
tmux set-option -t <tmux_name> status on
tmux set-option -t <tmux_name> status-right "<status_string>"
```

Constraints:
- `set-option` calls must be applied AFTER `new-session` completes.
- Boolean options always use `on`/`off` (not `1`/`0`).
- Status string must not contain shell-special characters that require quoting.
- The order of multiple `set-option` calls within a batch is undefined; they must not depend on each other.

## List sessions argv

```
tmux list-sessions -F '#{session_name}\t#{session_created}\t#{session_activity}'
```

Parsing rules:
- Output is tab-separated (`\t`).
- Handle CRLF line endings (some tmux versions emit CRLF over SSH) — strip `\r`.
- Skip malformed rows (wrong field count); emit a debug log entry.
- Filter: only process rows where `session_name` starts with `mux-`.
- Preserve duplicate session names — unexpected but must not panic.

## Kill session argv

```
tmux kill-session -t <tmux_name>
```

## Option ordering invariant

`set-option` calls are applied after `new-session` completes. Batched `set-option` calls within a single "apply options" step have no defined ordering among themselves; they must be independent.

## Status bar content

The status bar string is implementation-defined. It must not include characters that require shell quoting (e.g., `$`, `` ` ``, `\`, `"`, `'`, `|`, `&`, `;`, `<`, `>`) when passed as an argv element.
