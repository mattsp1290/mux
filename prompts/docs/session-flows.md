# Session Flow Sequencing

Spec: docs/07-create-list-status-kill-flows.md  
Status: Active  
Linked from: prompts/docs/README.md

## Create flow — step sequence

Implements: docs/07 §Create flow

Preconditions (abort with error on failure, in this order):
1. `<repo>` normalises successfully (docs/02 §Repo normalisation).
2. Host alias exists in the host inventory.
3. Host has `arch` and `home` set (i.e. `mux host test` has run).
4. `ssh-agent` is available (`SSH_AUTH_SOCK` set and socket reachable).

Transaction steps (all-or-nothing — see rollback below). This is an 11-step flow as defined
in docs/07. (docs/03:105 summarises these as "updated atomically in step 10" — see
sqlite-state.md §Reservation semantics for the interpretation note.)

| Step | Action | DB state after |
|------|--------|----------------|
| 1 | Generate UUID (v4) and shortname | — |
| 2 | INSERT session row: `status='active'`, `tmux_name=NULL` (reservation) | In-flight row |
| 3 | TOFU probe (docs/04 §TOFU) | Fingerprint stored or verified |
| 4 | Open SSH connection with forwarded agent | — |
| 5 | Probe transport mode (docs/04 §Transport selection); UPDATE session: `transport_mode`, `updated_at` | `transport_mode` set |
| 6 | `mkdir -p $MUX_HOME/<uuid>/<repo-leaf>` — abort with `workdir_pre_existing` if already exists | — |
| 7 | UPDATE session: `workdir = $MUX_HOME/<uuid>/<repo-leaf>`, `updated_at` | `workdir` set |
| 8 | `git clone --branch <branch> <clone_url> <workdir>` with `GIT_TERMINAL_PROMPT=0` in SSH env | — |
| 9 | Ensure agent is running (docs/05 §Agent startup) | — |
| 10 | Send `CreateSession` RPC; receive `{ uuid, shortname, tmux_name }` | — |
| 11 | UPDATE session: `tmux_name = <value>`, `status = 'active'`, `updated_at` | Complete row |

**In-flight rows** (steps 2–10 inclusive): `tmux_name IS NULL`. Excluded from `mux list`
reconciliation — not yet real sessions.

**`updated_at`**: Every UPDATE step (5, 7, 11) must set `updated_at = CAST(strftime('%s','now') AS INTEGER)`.

### Rollback on any failure after step 2

Delete the session row unconditionally.

Additionally:
- If workdir was created by us (step 6 succeeded) AND the path matches `$MUX_HOME/<uuid>/<repo-leaf>` AND contains no symlink components: remove it.
- If `workdir_pre_existing` fired at step 6: delete the session row only — we did NOT create the workdir, so do NOT remove it.
- If a tmux session was created (step 10 RPC succeeded): kill it.
- Never leave a partial session row.

### Create flow error categories

| Error key | Trigger condition |
|-----------|-------------------|
| `workdir_pre_existing` | Target workdir path already exists before clone |
| `git_clone_failed` | `git clone` exits non-zero |
| `ssh_agent_not_forwarded` | `SSH_AUTH_SOCK` missing or socket unreachable |
| `session_already_exists` | UUID collision (retry is acceptable) |
| `shortname_exhausted` | Shortname collision retries all failed (non-dead sessions checked) |
| `rpc_error` | `CreateSession` RPC returned an error |
| `other` | Any other unexpected failure |

## List flow

Implements: docs/07 §List flow

1. Load sessions from SQLite where `tmux_name IS NOT NULL` and `status != 'dead'`, grouped by
   host. (In-flight reservation rows — `tmux_name IS NULL` — are excluded.)
2. For each host (parallel or sequential):
   - SSH health probe with no TOFU prompt (read-only refresh, docs/04 §Read-only refresh).
   - If unreachable: mark all `active` sessions for that host as `unreachable`, set `updated_at`;
     skip to next host.
   - If reachable: call `ListSessions` RPC → `[{ uuid, shortname, tmux_name, workdir, status }]`.
3. Reconciliation rules (set `updated_at` on any status change):
   - Agent returns UUID not in DB with `mux-` prefix → import as `active` (`imported=1`; `workdir` from agent response).
   - Agent returns UUID in DB as `active` → update local status to agent's reported status.
     **Validation**: only accept values in `{active, dead, unreachable, orphaned}`; log and
     leave local status unchanged for any unrecognised value.
   - Agent returns `mux-`-prefixed session but agent's UUID map doesn't include it → mark DB row `orphaned`.
   - UUID is in DB as `active`, absent from agent response → mark `unreachable`.
   - UUID is in DB as `unreachable`, agent now returns it → resurrect to `active`.
   - DB rows with `status = 'dead'` or `'orphaned'`: skip, do not resurface.
4. Output grouped by host, sorted by `created_at` ascending within each group.
5. `--plain`: tab-separated columns, no ANSI codes.

## Status flow

Implements: docs/07 §Status flow

1. Resolve `<selector>`:
   - UUID-shaped input: exact UUID match only. No shortname fallback.
   - Shortname-shaped input: exact shortname match.
   - Host-alias string: reject.
2. Load session + host from SQLite.
3. Attempt `GetSession { uuid }` RPC:
   - Success: display live data.
   - Host unreachable: display local data, note "unreachable". No status mutation.
   - RPC error: surface error.
4. `mux status` never mutates session state.

## Kill flow

Implements: docs/07 §Kill flow

1. Resolve `<selector>` (same as status).
2. Check local status:
   - `dead`: exit 0, message `mux: session already dead`. No remote operation.
   - `unreachable`: exit 1, message `mux: host unreachable; verify connectivity and retry`.
   - `active` or `orphaned`: continue.
3. TOFU host-key verification (interactive; docs/04 §TOFU).
   - Mismatch: refuse with `host_key_mismatch`, no mutation.
4. Connect to agent; send `KillSession { uuid, repo_slug }`.
   - `repo_slug` is the slash form (`owner/repo`; docs/02 §repo_slug).
5. Agent validates `repo_slug` ownership; returns `{ tmux_killed, workdir_removed }`.
6. Client marks session dead locally (UPDATE `status='dead'`, `updated_at`) ONLY AFTER
   `tmux_killed OR workdir_removed` is true.
7. Non-owned session (agent returns ownership failure): exit 1,
   `mux: session not owned by this client`. No mutation.
8. Imported sessions: `workdir_removed` is always false; agent never removes non-mux-created dirs.

## Attach flow

Implements: docs/07 §Attach flow

1. Resolve `<selector>`. Reject dead sessions.
2. Load session + host from SQLite.
3. TOFU probe (docs/04 §Attach pinning):
   - No stored fingerprint: interactive TOFU prompt. Decline → abort, exit 1.
   - Mismatch: abort with `host_key_mismatch`, exit 1. No `exec ssh`. No mutation.
   - Match: proceed.
4. Write a single-entry temporary `known_hosts` file for the stored fingerprint.
5. `exec ssh` (replace current process — no cleanup code runs after this):
   ```
   ssh -o UserKnownHostsFile=<tmpfile> \
       -o HostKeyAlgorithms=<stored_alg> \
       -t <user>@<addr> -p <port> \
       tmux attach-session -t <tmux_name>
   ```

## Mutation invariants (all flows)

- Never mutate DB state before TOFU verification in create, kill, or attach.
- Never update a stored fingerprint silently on mismatch.
- Never skip TOFU for state-changing operations (create, kill, attach).
- Every session row UPDATE must set `updated_at`.
- `mux list` and `mux status` only update cached status fields — they do not kill or modify sessions.
