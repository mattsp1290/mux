# CLI Contract

Spec: docs/01-cli-commands.md  
Status: Active  
Linked from: prompts/docs/README.md

## Global behaviour

- Binary name: `mux`
- State directory: `$MUX_HOME` if set, else `~/.mux`
- If `MUX_HOME` unset and home directory cannot be determined: every command exits with `mux: <reason>` to stderr, exit code 1
- All user-facing errors are prefixed `mux: `
- Exit codes: `0` = success, `1` = user/host/remote error, `2` = internal/unexpected error

## Command matrix

### `mux init`

| Item | Value |
|------|-------|
| Creates | `$MUX_HOME` (mode 0700), `$MUX_HOME/mux.db` (mode 0600), all migrations |
| Idempotent | Yes — repeated runs succeed with no duplication |
| Config file | None created |

### `mux host add <alias> <user@addr> [--port <port>]`

| Item | Value |
|------|-------|
| `<alias>` | Validated per docs/02 §HostAlias rules |
| `<user@addr>` | Unix username + hostname or IP |
| `--port` | 1–65535; default 22 |
| Connects at add time | No — no TOFU prompt |
| Duplicate alias | Error, exit 1 |

### `mux host list`

Sorted by alias ascending. Columns: alias, user@addr, port, arch, home (show placeholders until `host test` runs).

### `mux host remove <alias> [--yes]`

Without `--yes`: print what will be deleted and ask for confirmation.  
With `--yes`: delete without confirmation.  
Cascade-removes: fingerprints, agent_versions, sessions for that host.

### `mux host test <alias>`

| Check | Sentinel command |
|-------|-----------------|
| arch | `uname -m` |
| home | `echo $HOME` |
| tmux version ≥ 3.0 | `tmux -V` |

Persists: arch, home, transport_mode, fingerprint, tmux_version.  
TOFU: interactive if first contact; silent if fingerprint matches.  
Idempotent: unchanged trust does not re-prompt.

### `mux host trust <alias>`

Shows current fingerprint; allows rotation with re-confirmation prompt.

### `mux agent deploy <alias>`

Preconditions: arch and home set (host test has run).  
Selects binary via `MUX_AGENT_BINARY` env var or built-in lookup.  
Upload path: `<home>/.mux/bin/mux-agent`.  
Post-upload: verify size + hash; `chmod +x`.  
Agent lifecycle: graceful stop first, kill fallback if needed.  
Persists version to `agent_versions` only after verified upload.

### `mux agent logs <alias> [--follow]`

Streams last 200 lines from `<home>/.mux/agent.log`.  
`--follow`: tail semantics.

### `mux agent stop <alias>`

Send Shutdown RPC if transport is known; fall back to process kill.  
No-process-found: success (idempotent).

### `mux create <repo> [--host <alias>] [--branch <branch>]`

| Flag | Notes |
|------|-------|
| `<repo>` | Normalised per docs/02 §Repo normalisation |
| `--host` | Required unless default host configured |
| `--branch` | Git branch/ref; defaults to repo default branch |

Full transaction: docs/07 §Create flow.  
`GIT_TERMINAL_PROMPT=0` prevents hanging on credential prompts.  
Requires `ssh-agent` forwarding (`SSH_AUTH_SOCK`).

### `mux attach <selector>`

`<selector>`: UUID or shortname.  
UUID exact match only — no shortname fallback for UUID-shaped input.  
Rejects dead sessions.  
TOFU probe then `exec ssh` (replaces current process) — see docs/07 §Attach flow.

### `mux list [--plain]`

Per-host reconciliation: docs/07 §List flow.  
Skips TOFU prompts (read-only refresh).  
`--plain`: tab-separated, no ANSI codes.

### `mux status <selector>`

`<selector>`: UUID or shortname.  
UUID takes priority; unknown UUID does not fall back to shortname.  
Rejects host-alias strings.  
No mutation on unreachable host.

### `mux kill <selector>`

TOFU verification before any mutation.  
Fingerprint mismatch: refuses operation.  
No-op dead: exit 0, `mux: session already dead`.  
Non-owned: exit 1, `mux: session not owned by this client`.  
Marks dead locally only after `tmux_killed OR workdir_removed` (docs/07 §Kill flow).

### `mux completions <shell>`

Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`.  
Writes to stdout.

## Output conventions

- `--plain` on `mux list`: tab-separated, no ANSI; suitable for scripting.
- All errors to stderr, prefixed `mux: `.
- No partial output on error — write nothing (or write the full header) then exit with the appropriate code.

## Exit code summary

| Code | Meaning |
|------|---------|
| 0 | Success (including no-op kill on dead session) |
| 1 | User / host / remote error (bad alias, unreachable host, permission denied, etc.) |
| 2 | Internal / unexpected error (panic, assertion, internal contract violation) |
