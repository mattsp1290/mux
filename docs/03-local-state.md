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
    tmux_name       TEXT    NOT NULL,      -- includes mux- prefix
    repo_slug       TEXT    NOT NULL,      -- owner/repo normalised
    branch          TEXT    NOT NULL,
    workdir         TEXT    NOT NULL,      -- remote absolute path
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
| `orphaned` | tmux session exists but no longer in the mux session map |

## Reservation semantics

Before any remote operation, a session row is inserted with `status = 'active'` and
a new UUID. This reserves the UUID and shortname. If the remote operation fails, the
row is deleted. This prevents partial-create rows from persisting.

## Timestamps

All timestamps are Unix seconds (INTEGER). Never use SQLite's `datetime()` function.
