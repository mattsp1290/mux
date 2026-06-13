//! Plain row structs mirroring the SQLite schema.
//!
//! These types are cheap to clone and carry no connection state.
//! Repository modules map query results into these structs.

/// A row from the `hosts` table.
#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub id: i64,
    pub alias: String,
    pub user: String,
    pub addr: String,
    pub port: i64,
    pub arch: Option<String>,
    pub home: Option<String>,
    pub transport: Option<String>,
    pub created_at: i64,
}

/// A row from the `known_host_fingerprints` table.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownHostFingerprint {
    pub id: i64,
    pub host_id: i64,
    pub algorithm: String,
    pub fingerprint: String,
    pub trusted_at: i64,
}

/// A row from the `agent_versions` table.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentVersion {
    pub id: i64,
    pub host_id: i64,
    pub version: String,
    pub deployed_at: i64,
}

/// A row from the `sessions` table.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: i64,
    pub uuid: String,
    pub host_id: i64,
    pub shortname: String,
    pub tmux_name: Option<String>,
    pub repo_slug: String,
    pub branch: String,
    pub workdir: Option<String>,
    pub transport_mode: Option<String>,
    pub status: String,
    pub imported: bool,
    pub created_at: i64,
    pub updated_at: i64,
}
