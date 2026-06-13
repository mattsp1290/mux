# SQLite State

Spec: docs/03-local-state.md  
Status: Active  
Linked from: prompts/docs/README.md

## Storage location and permissions

- DB file: `$MUX_HOME/mux.db` (default `~/.mux/mux.db`)
- File mode: 0600
- Directory mode: 0700
- Created by `mux init`; also created lazily on first use

## Connection settings

Apply on every connection open:

```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;   -- milliseconds
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
```

All four pragmas must be set. `foreign_keys = ON` is required for cascade deletes to work.

## Migration strategy

- Numbered sequential SQL files; applied in ascending order.
- Use `CREATE TABLE IF NOT EXISTS` and `ALTER TABLE … ADD COLUMN IF NOT EXISTS` — never destructive in the forward direction.
- Version tracking table:

```sql
CREATE TABLE IF NOT EXISTS _migrations (
    id         INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL   -- Unix seconds
);
```

- No rollback support; forward-only schema evolution.
- Concurrency-safe: `IF NOT EXISTS` patterns tolerate concurrent `mux init` calls.

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
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE UNIQUE,
    version     TEXT    NOT NULL,
    deployed_at INTEGER NOT NULL           -- Unix seconds
);
```

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
    workdir         TEXT,                  -- remote absolute path; NULL for imported rows
    transport_mode  TEXT,                  -- 'streamlocal' | 'tcp'; NULL until create
    status          TEXT    NOT NULL DEFAULT 'active',
    imported        INTEGER NOT NULL DEFAULT 0,  -- 1 = not mux-created workdir
    created_at      INTEGER NOT NULL,      -- Unix seconds
    updated_at      INTEGER NOT NULL       -- Unix seconds
);
```

## SessionStatus values

| Value | Meaning |
|-------|---------|
| `active` | Session is live and accessible |
| `dead` | Session confirmed gone |
| `unreachable` | Host could not be contacted during last list/status |
| `orphaned` | tmux session has `mux-` prefix but agent's UUID→tmux_name map does not include it |

## Reservation semantics

A reservation row is inserted BEFORE any remote operation:

| Column | Initial value | Filled at step |
|--------|---------------|----------------|
| `status` | `'active'` | — |
| `tmux_name` | `NULL` | Step 12 (CreateSession RPC) |
| `workdir` | `NULL` | Step 8 (after mkdir) |
| `transport_mode` | `NULL` | Step 6 (after transport probe) |

Rows where `tmux_name IS NULL` are **in-flight reservations** — excluded from `mux list` reconciliation. They are not real sessions.

On any failure after insertion: **DELETE the reservation row** (and conditionally remove workdir — see session-flows.md §Create flow rollback).

## Timestamp convention

All `*_at` columns store Unix seconds as `INTEGER`. Never use SQLite's `datetime()` function.

## Migrations directory

Migration SQL files live at `migrations/NNN-description.sql` (zero-padded 3-digit prefix).

Migration 001 creates all tables above plus `_migrations`.
