# 07 — Create / List / Status / Kill Flows

## Create flow

Preconditions (checked in order; abort with appropriate error on failure):
1. `<repo>` normalises successfully (docs/02).
2. Host alias exists in the host inventory (docs/03).
3. Host has `arch` and `home` set (i.e. `host test` has run).
4. `ssh-agent` is available (`SSH_AUTH_SOCK` is set and socket is reachable).

Transaction steps (must complete atomically or roll back):
1. Generate UUID (v4) and shortname (docs/02 §Shortname sanitisation).
2. Insert session row with `status = 'active'` (reservation; docs/03 §Reservation).
3. Perform TOFU probe (docs/04); abort and delete reservation on mismatch/refusal.
4. Open SSH connection with forwarded agent.
5. Probe transport mode (docs/04); persist `transport_mode` to session row.
6. Create workdir: `mkdir -p <home>/.mux/<uuid>`.
7. Clone: `git clone --branch <branch> <clone_url> <workdir>` with
   `GIT_TERMINAL_PROMPT=0` in the SSH session environment.
8. Ensure agent is running (docs/05 §Agent startup).
9. Send `CreateSession` RPC; receive `{ uuid, shortname, tmux_name }`.
10. Update session row: set `tmux_name`, confirm `status = 'active'`.

Rollback on any failure after step 2:
- Delete the session row.
- If workdir was created and is a mux-created path: remove it.
- If tmux session was created: kill it.
- Never leave a partial session row in the DB.

### Error categories

| Category | Condition |
|---|---|
| `workdir_pre_existing` | Target workdir already exists before clone |
| `git_clone_failed` | Git clone exits non-zero |
| `ssh_agent_not_forwarded` | No `SSH_AUTH_SOCK` or agent not reachable |
| `session_already_exists` | UUID collision (extremely unlikely; retry) |
| `shortname_exhausted` | All collision retries failed |
| `rpc_error` | `CreateSession` RPC returned an error |
| `other` | Any other unexpected failure |

## List flow

1. Load all non-dead sessions from SQLite, grouped by host.
2. For each host (in parallel or sequential):
   a. Check if host is reachable (SSH health probe, NO TOFU prompt).
   b. If unreachable: mark all sessions for that host as `unreachable`; skip to next host.
   c. If reachable: get live `ListSessions` from agent.
3. Reconciliation rules per session:
   - Live in agent, not in DB: import as `active` (mark `imported = 1`).
   - In DB as `active`, not in agent: mark `unreachable` (NOT dead — agent may be restarting).
   - In DB as `dead`/`orphaned`: skip (do not resurface).
   - In DB as `unreachable`, live in agent: resurrect to `active`.
4. Output: grouped by host, sorted by `created_at` ascending within each group.
5. `--plain`: tab-separated columns; no ANSI escape codes.

## Status flow

1. Resolve `<selector>`:
   - Try UUID exact match first.
   - If UUID not found: return error (no shortname fallback for UUID format).
   - If shortname: exact match.
2. Reject host-alias strings (not a valid session selector).
3. Load session and host from SQLite.
4. Attempt `GetSession` RPC:
   - Success: display live data.
   - Host unreachable: display local data, note "unreachable".
   - RPC error: surface error.
5. No mutation of session status during `mux status`.

## Kill flow

1. Resolve `<selector>` (same as status).
2. Perform TOFU host-key verification (interactive; docs/04).
   - Mismatch: refuse, error, no mutation.
3. Connect to agent; send `KillSession { uuid, repo_slug }`.
4. Agent validates `repo_slug` ownership; returns `{ tmux_killed, workdir_removed }`.
5. Client marks session dead locally only after receiving confirmation that `tmux_killed
   OR workdir_removed` is true.
6. No-op case: if session is already dead or not found, return success without mutation.
7. Imported sessions: `workdir_removed` is always false; agent never removes non-mux-created dirs.

## Attach flow

1. Resolve `<selector>`.
2. Reject dead sessions.
3. Load session and host.
4. Perform TOFU probe (docs/04 §Attach pinning).
5. Write temporary `known_hosts` file.
6. `exec ssh` — replace current process:
   ```
   ssh -o UserKnownHostsFile=<tmpfile> \
       -o HostKeyAlgorithms=<stored_alg> \
       -t <user>@<addr> -p <port> \
       tmux attach-session -t <tmux_name>
   ```
   The current process is replaced; no cleanup code runs after `exec`.
