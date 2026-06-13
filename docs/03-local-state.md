# 03 — Local State

## Storage location

`$MUX_HOME/mux.db` (default `~/.mux/mux.db`). Created with mode 0600. Directory
created with mode 0700.

## SQLite connection settings

Applied on every connection open:

```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;   -- milliseconds
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
```

## Migrations

- Numbered sequential SQL files applied in order.
- Concurrency-safe: use `CREATE TABLE IF NOT EXISTS` and `ALTER TABLE … ADD COLUMN IF
  NOT EXISTS` patterns; never destructive in the forward direction.
- Version tracked in a `_migrations` table: `(id INTEGER PRIMARY KEY, applied_at INTEGER)`.
- Rollbacks are not supported; forward-only schema evolution.

## Hosts table

```sql
CREATE TABLE hosts (
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

## Known host fingerprints table

```sql
CREATE TABLE known_host_fingerprints (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    algorithm   TEXT    NOT NULL,          -- e.g. 'ssh-ed25519'
    fingerprint TEXT    NOT NULL,
    trusted_at  INTEGER NOT NULL,          -- Unix seconds
    UNIQUE (host_id, algorithm)
);
```

## Agent versions table

```sql
CREATE TABLE agent_versions (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE UNIQUE,
    version     TEXT    NOT NULL,
    deployed_at INTEGER NOT NULL           -- Unix seconds
);
```

## Sessions table

```sql
CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY,
    uuid            TEXT    NOT NULL UNIQUE,
    host_id         INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    shortname       TEXT    NOT NULL,
    tmux_name       TEXT,                  -- includes mux- prefix; NULL during reservation
    repo_slug       TEXT    NOT NULL,      -- owner/repo (slash form; see docs/02 §repo_slug)
    branch          TEXT    NOT NULL,
    workdir         TEXT,                  -- remote absolute path; NULL for imported rows
                                           -- where workdir is unknown until first list
    transport_mode  TEXT,                  -- 'streamlocal' | 'tcp'; NULL until create
    status          TEXT    NOT NULL DEFAULT 'active',  -- see SessionStatus
    imported        INTEGER NOT NULL DEFAULT 0,  -- 1 = not mux-created workdir
    created_at      INTEGER NOT NULL,      -- Unix seconds
    updated_at      INTEGER NOT NULL       -- Unix seconds
);
```

## SessionStatus values

| Value | Meaning |
|---|---|
| `active` | Session is live and accessible |
| `dead` | Session confirmed gone (tmux session destroyed) |
| `unreachable` | Host could not be contacted during last list/status |
| `orphaned` | tmux session exists with `mux-` prefix but not in the agent's UUID→tmux_name map (set during `mux list` reconciliation) |

## Reservation semantics

Before any remote operation, a session row is inserted with:
- `status = 'active'`
- `tmux_name = NULL` (filled after `CreateSession` RPC completes)
- `workdir = NULL` (filled after workdir is created)
- `transport_mode = NULL` (filled after transport probe)

The `tmux_name`, `workdir`, and `transport_mode` fields are updated atomically in step 10
of the create flow (docs/07). If the remote operation fails, the row is deleted. In-flight
reservation rows (where `tmux_name IS NULL`) are ignored by `mux list` reconciliation
(they are not yet real sessions).

## Timestamps

All timestamps are Unix seconds (INTEGER). Never use SQLite's `datetime()` function.
