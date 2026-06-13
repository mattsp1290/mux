# 01 — CLI Commands

## Global behaviour

- Binary name: `mux`
- State directory: `MUX_HOME` env var if set, otherwise `~/.mux`.
- Home-directory failure: if `MUX_HOME` is unset and the home directory cannot be
  determined, every command exits with a human-readable error prefixed `mux: `.
- Error prefix: all user-facing errors are prefixed `mux: `.
- Exit codes: 0 = success, 1 = user/host/remote error, 2 = internal/unexpected error.

## `mux init`

Initialise the mux state directory.

- Creates `$MUX_HOME` with mode 0700 if it does not exist.
- Creates `$MUX_HOME/mux.db` and runs all pending migrations.
- Idempotent: repeated runs succeed without error or duplication.
- No v0.1 config file is created.

## `mux host`

Manage the remote-host inventory.

### `mux host add <alias> <user@addr> [--port <port>]`

- `<alias>`: validated per docs/02 §HostAlias rules.
- `<user@addr>`: `user` is a Unix username; `addr` is a hostname or IP.
- `--port`: port number 1–65535; default 22.
- Does NOT connect at add time; no TOFU prompt.
- Rejects duplicate aliases.
- Persists to `hosts` table (docs/03).

### `mux host list`

- Sorted by alias (ascending).
- Columns: alias, user@addr, port, arch, home (placeholders until `host test` runs).

### `mux host remove <alias> [--yes]`

- Without `--yes`: prints what will be deleted and asks for confirmation.
- With `--yes`: deletes without confirmation.
- Cascade-removes fingerprints, agent_versions, and sessions for that host.

### `mux host test <alias>`

- Connects over SSH; requires `ssh-agent` forwarding.
- Runs sentinels: `uname -m` (arch), `echo $HOME` (home), `tmux -V` (version ≥3.0).
- Parses MOTD noise to extract sentinel output.
- Confirms trust via TOFU (docs/04).
- Persists: arch, home, transport_mode, fingerprint, tmux_version, tool availability.
- Idempotent: re-running when trust is unchanged does not re-prompt.

### `mux host trust <alias>`

- Shows current fingerprint and allows rotation.
- Rotation triggers a re-confirmation prompt.

## `mux agent`

Manage the remote `mux-agent` daemon.

### `mux agent deploy <alias>`

- Preconditions: host arch and home must be set (`host test` must have run).
- Selects arch-specific binary via `MUX_AGENT_BINARY` env var or built-in lookup.
- Uploads to `<home>/.mux/bin/mux-agent` via SSH.
- Verifies size and hash after upload.
- Sets executable bit (`chmod +x`).
- Graceful stop before kill fallback if agent is running.
- Persists version to `agent_versions` only after verified upload.

### `mux agent logs <alias> [--follow]`

- Streams last 200 lines of agent log from `<home>/.mux/agent.log`.
- `--follow`: tail -f semantics.

### `mux agent stop <alias>`

- Sends Shutdown RPC if transport is known.
- Falls back to process kill if RPC fails.
- No-process-found is success (idempotent).

## `mux create <repo> [--host <alias>] [--branch <branch>]`

Create a remote tmux session for a repository.

- `<repo>`: normalised per docs/02 §Repo normalisation.
- `--host`: host alias. Required unless a default host is configured.
- `--branch`: git branch/ref to check out; defaults to repo default branch.
- Full transaction: see docs/07 §Create flow.
- `GIT_TERMINAL_PROMPT=0` prevents hanging on credential prompts.
- Requires `ssh-agent` forwarding.

## `mux attach <selector>`

Attach to an existing session.

- `<selector>`: UUID or shortname.
- Unknown UUID: no fallback to shortname.
- Rejects dead sessions.
- Performs TOFU probe (docs/04).
- Writes a one-key temporary `known_hosts` file pinning the verified key.
- Replaces the current process via `exec ssh` with pinned `HostKeyAlgorithms`.
- Target: stored `tmux_name` for the session.

## `mux list [--plain]`

List sessions with per-host reconciliation.

- Reconciliation: see docs/07 §List flow.
- Skips TOFU prompts during read-only refresh.
- `--plain`: tab-separated output without ANSI.

## `mux status <selector>`

Show session status.

- `<selector>`: UUID or shortname.
- UUID lookup takes priority; unknown UUID does not fall back to shortname.
- Rejects host-alias strings (not a valid selector).
- On unreachable host: returns local data without mutation.
- On success: shows live data from `GetSession` RPC.

## `mux kill <selector>`

Kill a session.

- `<selector>`: UUID or shortname.
- Performs TOFU host-key verification before any mutation.
- Fingerprint mismatch: refuses operation.
- Gets ownership via `repo_slug`.
- Sends `KillSession` RPC; agent removes workdir and kills tmux session.
- No-op on already-dead sessions: exits 0 with message `mux: session already dead`.
- No-op on non-owned sessions: exits 1 with message `mux: session not owned by this client`.
- Marks session dead locally only after `tmux_killed` or `workdir_removed` effect.

## `mux completions <shell>`

Generate shell completions.

- Supported: `bash`, `zsh`, `fish`, `powershell`, `elvish`.
- Writes to stdout.
