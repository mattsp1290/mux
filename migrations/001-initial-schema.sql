-- Migration 001: Initial schema
-- Implements: docs/03 §SQLite setup, §Hosts table, §Known host fingerprints table,
--             §Agent versions table, §Sessions table
--
-- This file is pure DDL: no BEGIN/COMMIT and no _migrations INSERT.
-- The migration runner owns transaction control and version recording.
--
-- Runner requirements before applying:
--   PRAGMA foreign_keys = ON;   (required for ON DELETE CASCADE to function)
--   PRAGMA journal_mode = WAL;
--   PRAGMA busy_timeout = 5000;
--   PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS hosts (
    id          INTEGER PRIMARY KEY,
    alias       TEXT    NOT NULL UNIQUE,
    user        TEXT    NOT NULL,
    addr        TEXT    NOT NULL,
    port        INTEGER NOT NULL DEFAULT 22,
    arch        TEXT,
    home        TEXT,
    transport   TEXT,             -- 'streamlocal' | 'tcp' | NULL until host test
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS known_host_fingerprints (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    algorithm   TEXT    NOT NULL,          -- e.g. 'ssh-ed25519'
    fingerprint TEXT    NOT NULL,
    trusted_at  INTEGER NOT NULL,          -- Unix seconds
    UNIQUE (host_id, algorithm)
);

CREATE TABLE IF NOT EXISTS agent_versions (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    version     TEXT    NOT NULL,
    deployed_at INTEGER NOT NULL,          -- Unix seconds
    UNIQUE (host_id)
);

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
    updated_at      INTEGER NOT NULL       -- Unix seconds; updated on every mutation
);

-- Indexes for hot-path queries (per-host session listing, shortname resolution)
CREATE INDEX IF NOT EXISTS idx_sessions_host ON sessions (host_id);
CREATE INDEX IF NOT EXISTS idx_sessions_shortname ON sessions (shortname);
