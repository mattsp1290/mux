# mux User Guide

`mux` is a command-line tool for creating and managing persistent tmux sessions on remote hosts over SSH. You start a session from your laptop, disconnect, come back hours later, and pick up exactly where you left off — without manually SSHing in, finding the right directory, or reattaching by hand.

## Table of Contents

- [Prerequisites](#prerequisites)
- [State directory](#state-directory)
- [Workflow overview](#workflow-overview)
- [Step 1: Initialize mux](#step-1-initialize-mux)
- [Step 2: Add a remote host](#step-2-add-a-remote-host)
- [Step 3: Test the connection](#step-3-test-the-connection)
- [Step 4: Deploy the agent](#step-4-deploy-the-agent)
- [Step 5: Create a session](#step-5-create-a-session)
- [Managing sessions](#managing-sessions)
- [Host management](#host-management)
- [Agent management](#agent-management)
- [Shell completions](#shell-completions)
- [Environment variables](#environment-variables)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

- **ssh-agent** running and loaded with a key that has access to your remote host. Run `ssh-add -l` to verify; if it prints nothing, run `ssh-add` to load your default key.
- **tmux ≥ 3.0** installed on each remote host. `mux host test` will tell you if the version is too old.
- The remote host must be reachable by SSH. `mux` uses your agent-forwarded key — it does not manage passwords.

---

## State directory

`mux` stores all local state (database, fingerprints, logs) in a single directory:

- Default: `~/.mux`
- Override: set the `MUX_HOME` environment variable to any path

Everything in that directory is created with mode `0700` (owner-only).

---

## Workflow overview

The first time you set up a new remote host, you go through these steps once:

```
mux init          → create local state
mux host add      → register the host
mux host test     → connect, probe, and accept the host key (TOFU)
mux agent deploy  → upload the mux-agent binary to the remote host
```

After that, creating and attaching to sessions is just:

```
mux create owner/repo --host myserver
mux attach myproject
```

---

## Step 1: Initialize mux

```sh
mux init
```

Creates `~/.mux` (or `$MUX_HOME`) and sets up the local database. Safe to run again at any time — it is idempotent.

---

## Step 2: Add a remote host

```sh
mux host add myserver alice@192.168.1.10
mux host add myserver alice@192.168.1.10 --port 2222
```

This registers the host under an alias (`myserver`) so you can refer to it by name in every subsequent command. The alias must be alphanumeric or kebab-case (e.g., `my-server`, `dev1`).

`mux host add` does **not** connect to the host. Duplicate aliases are rejected.

To see all registered hosts:

```sh
mux host list
```

Output is sorted by alias and shows the address, port, architecture, and home directory. Architecture and home directory show as placeholders until you run `mux host test`.

---

## Step 3: Test the connection

```sh
mux host test myserver
```

This opens an SSH connection using your agent-forwarded key and:

1. Probes the remote architecture (`uname -m`) and home directory (`$HOME`).
2. Checks that tmux is installed and at version 3.0 or higher.
3. Performs a **TOFU (Trust On First Use)** host key check.

### TOFU — trusting a host for the first time

The first time you run `mux host test` against a host, `mux` shows you the SSH fingerprint and asks you to confirm:

```
Host fingerprint for myserver (192.168.1.10):
  SHA256:abc123...xyz

Accept and store this fingerprint? [y/N]
```

Type `y` to accept. The fingerprint is stored locally. All future connections to this host silently verify against the stored fingerprint — you are never asked again unless the key changes.

If the stored fingerprint ever stops matching (e.g., the server was rebuilt and its host key rotated), `mux` will refuse the connection and tell you to run `mux host trust myserver` to review and accept the new key.

`mux host test` is idempotent — safe to re-run to refresh the architecture and home directory information.

---

## Step 4: Deploy the agent

```sh
mux agent deploy myserver
```

Uploads the `mux-agent` binary to the remote host at `~/.mux/bin/mux-agent` and verifies the upload. The binary is architecture-specific; `mux agent deploy` uses the arch information gathered by `mux host test`.

`mux host test` must have been run before deploying.

---

## Step 5: Create a session

```sh
mux create github.com/alice/myproject --host myserver
mux create alice/myproject --host myserver
mux create alice/myproject --host myserver --branch feature/my-branch
```

`mux create` does the following on the remote host:

1. Creates a working directory under your remote home.
2. Clones the repository (using your forwarded SSH key — `GIT_TERMINAL_PROMPT=0` prevents any interactive prompts that would hang).
3. Starts a tmux session inside the cloned directory.
4. Records the session in local state.

`--branch` defaults to the repository's default branch. Use it to check out a specific branch from the start.

`--host` is required unless you have a default host configured.

You must have `ssh-agent` running with a key that has access to the remote host **and** to the Git host (GitHub, GitLab, etc.).

---

## Managing sessions

### List sessions

```sh
mux list
mux list --plain
```

Shows all sessions with their current status. `mux list` reconciles with live agent state, so the status reflects whether each session is actually alive on the remote host. Unreachable hosts are marked without prompting for TOFU — list is read-only.

`--plain` outputs tab-separated rows without ANSI color codes, useful for scripting.

### Check session status

```sh
mux status myproject
mux status 550e8400-e29b-41d4-a716-446655440000
```

The selector is either the session's short name or its UUID. UUID lookup takes priority when both could match.

### Attach to a session

```sh
mux attach myproject
mux attach 550e8400-e29b-41d4-a716-446655440000
```

Opens an SSH connection to the session's host and attaches to the tmux session, replacing the current shell process (`exec`). When you detach from tmux (`Ctrl-b d`), you are back at your local shell — the session continues running on the remote host.

`mux attach` verifies the stored host key before connecting. Dead sessions are rejected with a clear error.

### Kill a session

```sh
mux kill myproject
mux kill 550e8400-e29b-41d4-a716-446655440000
```

Terminates the tmux session on the remote host and marks it dead in local state. `mux kill` verifies the host key before making any changes — if the fingerprint has changed, the operation is refused. Killing an already-dead session exits cleanly (exit code 0). Trying to kill a session you do not own exits with code 1.

---

## Host management

### List hosts

```sh
mux host list
```

### Remove a host

```sh
mux host remove myserver
mux host remove myserver --yes
```

Without `--yes`, you are asked to confirm. Removing a host cascade-deletes its stored fingerprint and any sessions associated with it.

### Review or rotate a host key

```sh
mux host trust myserver
```

Shows the currently stored fingerprint for the host and lets you accept a new one. Use this when a host's SSH key has changed (e.g., after a server rebuild) and `mux` is refusing connections with `mux: host_key_mismatch`.

---

## Agent management

The `mux-agent` daemon runs on each remote host and manages session lifecycle. You normally only interact with it when troubleshooting.

### View agent logs

```sh
mux agent logs myserver
mux agent logs myserver --follow
```

Streams the last 200 lines of the agent log file (`~/.mux/agent.log` on the remote host). `--follow` tails the log in real time.

### Stop the agent

```sh
mux agent stop myserver
```

Sends a shutdown signal to the agent. Falls back to killing the process directly if the RPC call fails. Safe to run when the agent is not running — no-process-found is treated as success.

---

## Shell completions

```sh
mux completions bash   >> ~/.bash_completion.d/mux
mux completions zsh    >> ~/.zsh/completions/_mux
mux completions fish   > ~/.config/fish/completions/mux.fish
mux completions powershell
mux completions elvish
```

Completions are written to stdout. Redirect to the appropriate file for your shell.

---

## Environment variables

| Variable | Description |
|---|---|
| `MUX_HOME` | Override the state directory (default: `~/.mux`) |
| `MUX_FORCE_TRANSPORT` | Force transport for `mux create`: `streamlocal` or `tcp`. Does not affect existing sessions. |

---

## Troubleshooting

### `mux: ssh_agent_not_forwarded`

Your SSH agent is not running or has no keys loaded.

```sh
eval "$(ssh-agent -s)"
ssh-add
```

Then retry. For `mux create` to clone from GitHub, you also need a key loaded that has access to the Git host.

### `mux: host_key_mismatch`

The stored fingerprint for a host no longer matches what the server presents. This happens after a server rebuild or key rotation.

```sh
mux host trust myserver
```

Review the new fingerprint carefully before accepting. If you did not expect the key to change, treat this as a warning that the host may have changed unexpectedly.

### `mux: tofu_non_interactive`

An operation tried to prompt for TOFU acceptance but was running in a non-interactive context. This means `mux host test` has not been run yet for this host.

```sh
mux host test myserver
```

Accept the fingerprint interactively, then retry the original command.

### `mux: workdir_pre_existing`

The working directory that `mux create` would create already exists on the remote host. The directory is at `~/.mux/<uuid>/<repo-name>` on the remote.

Either remove the existing directory on the remote host, or create the session on a different host.

### `mux: session already dead`

The session you tried to attach to or interact with is no longer alive.

```sh
mux list
```

Check current session status. If you want a new session, run `mux create` again.

### Git clone failure during `mux create`

`mux create` uses `GIT_TERMINAL_PROMPT=0`, which means it will fail immediately if the clone requires interactive input (e.g., password). Make sure:

1. `ssh-agent` is running and has the right key loaded (`ssh-add -l`).
2. The key has access to the Git host (GitHub, GitLab, etc.).
3. The repository path is correct (`owner/repo` format or full URL).

### tmux version too old

`mux host test` requires tmux ≥ 3.0 on the remote host. If it reports the version is too low, upgrade tmux on the remote host before proceeding.

```sh
# On the remote host — example for Debian/Ubuntu
sudo apt install tmux
tmux -V
```

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | User error, host error, or remote error (e.g., wrong alias, permission denied, session not owned) |
| `2` | Internal error (bug — please report) |

All error messages are prefixed with `mux: ` so they are easy to identify in logs and scripts.
