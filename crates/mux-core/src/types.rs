use serde::{Deserialize, Serialize};

use crate::error::MuxError;

/// A validated host alias (alphanumeric, hyphens; non-empty). Dots are NOT
/// allowed: aliases are local nicknames, not hostnames — permitting dots would
/// risk aliases colliding with FQDNs in lookup logic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HostAlias(String);

impl std::str::FromStr for HostAlias {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        valid
            .then(|| HostAlias(s.to_owned()))
            .ok_or_else(|| MuxError::InvalidHostAlias(s.to_owned()))
    }
}

impl std::fmt::Display for HostAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Transport mode selected for a host.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    Streamlocal,
    Tcp,
}

/// Session lifecycle status stored in local SQLite.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Dead,
    Unreachable,
    Orphaned,
}

/// Selector for a session: either a full UUID or a short name.
///
/// Not serialized — this is a transient CLI input type, never persisted.
/// Parsing precedence: try UUID first, fall back to shortname.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSelector {
    Uuid(uuid::Uuid),
    Shortname(String),
}

impl std::str::FromStr for SessionSelector {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match uuid::Uuid::parse_str(s) {
            Ok(uuid) => Ok(SessionSelector::Uuid(uuid)),
            Err(_) => Ok(SessionSelector::Shortname(s.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_alias_valid() {
        assert!("my-host".parse::<HostAlias>().is_ok());
        assert!("host_1".parse::<HostAlias>().is_ok());
    }

    #[test]
    fn host_alias_rejects_empty() {
        assert!("".parse::<HostAlias>().is_err());
    }

    #[test]
    fn host_alias_rejects_dots() {
        assert!("host.example.com".parse::<HostAlias>().is_err());
    }

    #[test]
    fn host_alias_rejects_spaces() {
        assert!("my host".parse::<HostAlias>().is_err());
    }

    #[test]
    fn session_selector_parses_uuid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let sel: SessionSelector = uuid_str.parse().unwrap();
        assert!(matches!(sel, SessionSelector::Uuid(_)));
    }

    #[test]
    fn session_selector_falls_back_to_shortname() {
        let sel: SessionSelector = "my-session".parse().unwrap();
        assert_eq!(sel, SessionSelector::Shortname("my-session".to_owned()));
    }
}
