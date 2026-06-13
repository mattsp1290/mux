# SQLite State

Spec: docs/03-local-state.md  
Status: Active  
Linked from: prompts/docs/README.md

## Storage location and permissions

- DB file: `$MUX_HOME/mux.db` (default `~/.mux/mux.db`)
- File mode: 0600
- Directory mode: 0700
- Created by `mux init`; also created lazily on first use
- WAL mode creates `mux.db-wal` and `mux.db-shm` sidecar files. These inherit the 0600
  mode of the main DB file. Ensure umask does not loosen them (they will be 0600 if
  `mux.db` is created 0600 before WAL mode is enabled).

## Connection settings

Apply on every connection open:

```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;   -- milliseconds
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
```

All four pragmas must be set. `foreign_keys = ON` is required for cascade deletes to work
and must be set on the connection applying migrations as well as every runtime connection.

## Migration strategy

- Numbered sequential SQL files; applied in ascending order.
- Use `CREATE TABLE IF NOT EXISTS` for new tables — never destructive in the forward direction.
- For column additions in future migrations: use `ALTER TABLE … ADD COLUMN` guarded by the
  version gate (check `_migrations` before applying). **SQLite does not support
  `ADD COLUMN IF NOT EXISTS`** — idempotency for column adds is provided by the `_migrations`
  version gate, not by SQL-level guards.
- Version tracking table:

```sql
CREATE TABLE IF NOT EXISTS _migrations (
    id         INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL   -- Unix seconds
);
```

- Migration SQL files are responsible for inserting their own row:
  `INSERT OR IGNORE INTO _migrations (id, applied_at) VALUES (N, CAST(strftime('%s','now') AS INTEGER));`
- Each migration is wrapped in `BEGIN;` / `COMMIT;`.
- Migration runner checks `SELECT id FROM _migrations WHERE id = N` before applying; skips
  if already present.
- No rollback support; forward-only schema evolution.

## Schema

### `hosts`

```sql
CREATE TABLE IF NOT EXISTS hosts (
    id          INTEGER PRIMARY KEY,
    alias       TEXT    NOT NULL UNIQUE,   -- validated HostAlias
    user        TEXT    NOT NULL,
    addr        TEXT    NOT NULL,
    port        INTEGER NOT NULL DEFAULT 22,
    arch        TEXT,                      -- 'amd64' | 'arm64' | NULL until host test
    home        TEXT,                      -- remote $HOME | NULL until host test
    transport   TEXT,                      -- 'streamlocal' | 'tcp' | NULL until host test
    created_at  INTEGER NOT NULL           -- Unix seconds
);
```

Note: `hosts.transport` is the per-host default transport (set by `mux host test`).
`sessions.transport_mode` is the per-session transport (set during `mux create`). These are
distinct columns on different tables.

### `known_host_fingerprints`

```sql
CREATE TABLE IF NOT EXISTS known_host_fingerprints (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    algorithm   TEXT    NOT NULL,          -- e.g. 'ssh-ed25519'
    fingerprint TEXT    NOT NULL,
    trusted_at  INTEGER NOT NULL,          -- Unix seconds
    UNIQUE (host_id, algorithm)
);
```

### `agent_versions`

```sql
CREATE TABLE IF NOT EXISTS agent_versions (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    version     TEXT    NOT NULL,
    deployed_at INTEGER NOT NULL,          -- Unix seconds
    UNIQUE (host_id)
);
```

One row per host. `mux agent deploy` upserts:
`INSERT INTO agent_versions ... ON CONFLICT(host_id) DO UPDATE SET version=..., deployed_at=...`

### `sessions`

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id              INTEGER PRIMARY KEY,
    uuid            TEXT    NOT NULL UNIQUE,
    host_id         INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    shortname       TEXT    NOT NULL,
    tmux_name       TEXT,                  -- includes mux- prefix; NULL during reservation
    repo_slug       TEXT    NOT NULL,      -- owner/repo (slash form; docs/02 §repo_slug)
    branch          TEXT    NOT NULL,
    workdir         TEXT,                  -- remote absolute path; NULL until step 7 of create
    transport_mode  TEXT,                  -- 'streamlocal' | 'tcp'; NULL until step 5 of create
    status          TEXT    NOT NULL DEFAULT 'active',
    imported        INTEGER NOT NULL DEFAULT 0,   -- 1 = not mux-created workdir
    created_at      INTEGER NOT NULL,      -- Unix seconds
    updated_at      INTEGER NOT NULL       -- Unix seconds; set on every mutation
);
```

Indexes (defined in migration 001):
- `idx_sessions_host` on `sessions(host_id)` — per-host grouping in `mux list`
- `idx_sessions_shortname` on `sessions(shortname)` — shortname resolution in `mux attach/status`

**Shortname uniqueness**: Shortnames are application-enforced unique among non-dead sessions.
There is no DB `UNIQUE` constraint (dead sessions may share a shortname with active ones).
The `shortname_exhausted` error fires when the application's retry loop cannot find a
collision-free name among non-dead sessions.

**Agent-reported status validation**: When reconciliation writes the agent's reported status
into `sessions.status`, only accept values in `{active, dead, unreachable, orphaned}`. An
unrecognised value must be logged and the local status left unchanged.

## SessionStatus values

| Value | Meaning |
|-------|---------|
| `active` | Session is live and accessible |
| `dead` | Session confirmed gone |
| `unreachable` | Host could not be contacted during last list/status |
| `orphaned` | tmux session has `mux-` prefix but agent's UUID→tmux_name map does not include it |

## Reservation semantics

A reservation row is inserted BEFORE any remote operation. Fields are updated progressively
during the create flow (docs/07, 11 steps) and set to their final values by the end of step 11:

| Column | Initial value | Filled at step (docs/07) |
|--------|---------------|--------------------------|
| `status` | `'active'` | — (set at INSERT) |
| `transport_mode` | `NULL` | Step 5 (transport probe + UPDATE) |
| `workdir` | `NULL` | Step 7 (after mkdir + UPDATE) |
| `tmux_name` | `NULL` | Step 11 (after CreateSession RPC) |

Note: docs/03:105 states these fields are "updated atomically in step 10". The canonical
docs/07 11-step flow shows them updated progressively at steps 5, 7, and 11. The docs/07
sequence is the authoritative flow description; the docs/03 phrasing is an imprecise summary.
This interpretation is tracked per clean-room-guardrails.md §When the spec is ambiguous.

Rows where `tmux_name IS NULL` are **in-flight reservations** — excluded from `mux list`
reconciliation. They are not real sessions.

On any failure after insertion: **DELETE the reservation row** (and conditionally remove
workdir — see session-flows.md §Create flow rollback).

**`updated_at` maintenance**: Every UPDATE to a session row must set
`updated_at = CAST(strftime('%s','now') AS INTEGER)`.

## Timestamp convention

All `*_at` columns store Unix seconds as `INTEGER`. Never use SQLite's `datetime()` function.
`strftime('%s', 'now')` returns the current Unix time as text; cast with
`CAST(strftime('%s','now') AS INTEGER)`.

## Migrations directory

Migration SQL files live at `migrations/NNN-description.sql` (zero-padded 3-digit prefix).

Migration 001 creates all tables above plus `_migrations`.
