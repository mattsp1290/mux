use serde::{Deserialize, Serialize};

/// A validated host alias (alphanumeric, hyphens, dots; non-empty).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HostAlias(String);

/// Transport mode selected for a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    Streamlocal,
    Tcp,
}

/// Session lifecycle status stored in local SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Dead,
    Unreachable,
    Orphaned,
}

/// Selector for a session: either a full UUID or a short name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSelector {
    Uuid(uuid::Uuid),
    Shortname(String),
}
