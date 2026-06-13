-- Migration 001: Initial schema
-- Implements: docs/03 §SQLite setup, §Hosts table, §Known host fingerprints table,
--             §Agent versions table, §Sessions table

CREATE TABLE IF NOT EXISTS _migrations (
    id         INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL   -- Unix seconds
);

CREATE TABLE IF NOT EXISTS hosts (
    id          INTEGER PRIMARY KEY,
    alias       TEXT    NOT NULL UNIQUE,
    user        TEXT    NOT NULL,
    addr        TEXT    NOT NULL,
    port        INTEGER NOT NULL DEFAULT 22,
    arch        TEXT,
    home        TEXT,
    transport   TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS known_host_fingerprints (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    algorithm   TEXT    NOT NULL,
    fingerprint TEXT    NOT NULL,
    trusted_at  INTEGER NOT NULL,
    UNIQUE (host_id, algorithm)
);

CREATE TABLE IF NOT EXISTS agent_versions (
    id          INTEGER PRIMARY KEY,
    host_id     INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE UNIQUE,
    version     TEXT    NOT NULL,
    deployed_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id              INTEGER PRIMARY KEY,
    uuid            TEXT    NOT NULL UNIQUE,
    host_id         INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    shortname       TEXT    NOT NULL,
    tmux_name       TEXT,
    repo_slug       TEXT    NOT NULL,
    branch          TEXT    NOT NULL,
    workdir         TEXT,
    transport_mode  TEXT,
    status          TEXT    NOT NULL DEFAULT 'active',
    imported        INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
