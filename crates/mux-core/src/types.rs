use serde::{Deserialize, Serialize};

use crate::error::MuxError;

/// A validated host alias (alphanumeric, hyphens, underscores; non-empty, ≤64 chars,
/// must start with an alphanumeric character). Dots are NOT allowed: aliases are
/// local nicknames, not hostnames — permitting dots would risk aliases colliding with
/// FQDNs in lookup logic. Leading hyphens are also rejected because aliases are
/// passed as argv tokens to `ssh` and `tmux`, where a leading `-` parses as a flag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct HostAlias(String);

impl HostAlias {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for HostAlias {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let first_ok = s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        let len_ok = (1..=64).contains(&s.len());
        let chars_ok = s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        (first_ok && len_ok && chars_ok)
            .then(|| HostAlias(s.to_owned()))
            .ok_or_else(|| MuxError::InvalidHostAlias(s.to_owned()))
    }
}

impl<'de> Deserialize<'de> for HostAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
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

impl std::str::FromStr for SessionStatus {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "dead" => Ok(Self::Dead),
            "unreachable" => Ok(Self::Unreachable),
            "orphaned" => Ok(Self::Orphaned),
            _ => Err(MuxError::InvalidSessionStatus(s.to_owned())),
        }
    }
}

/// Selector for a session: either a full UUID or a short name.
///
/// Not serialized — this is a transient CLI input type, never persisted.
/// Parsing precedence: try UUID first, fall back to shortname.
///
/// **Constraint**: a shortname that happens to be a valid UUID string is always
/// parsed as `Uuid`, never `Shortname`. Session-create paths should reject or warn
/// when the user provides a UUID-shaped name so it cannot become unreachable by name.
///
/// `Infallible` error means every string is a valid selector; "does this resolve to
/// an existing session?" is checked at resolution time against the session store,
/// not here.
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
        assert!("a".parse::<HostAlias>().is_ok());
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
    fn host_alias_rejects_leading_hyphen() {
        assert!("-host".parse::<HostAlias>().is_err());
        assert!("-".parse::<HostAlias>().is_err());
    }

    #[test]
    fn host_alias_rejects_over_length() {
        let long = "a".repeat(65);
        assert!(long.parse::<HostAlias>().is_err());
        assert!("a".repeat(64).parse::<HostAlias>().is_ok());
    }

    #[test]
    fn host_alias_trailing_hyphen_ok() {
        // trailing hyphen is allowed — only leading is rejected for argv safety
        assert!("host-".parse::<HostAlias>().is_ok());
    }

    #[test]
    fn host_alias_deserialize_rejects_invalid() {
        let json = r#""has.dots""#;
        let result: Result<HostAlias, _> = serde_json::from_str(json);
        assert!(result.is_err(), "deserialization should reject invalid alias");
    }

    #[test]
    fn host_alias_serde_roundtrip() {
        let alias: HostAlias = "my-host".parse().unwrap();
        let json = serde_json::to_string(&alias).unwrap();
        assert_eq!(json, r#""my-host""#);
        let back: HostAlias = serde_json::from_str(&json).unwrap();
        assert_eq!(alias, back);
    }

    #[test]
    fn session_status_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&SessionStatus::Active).unwrap(),
            r#""active""#
        );
        assert_eq!(
            serde_json::to_string(&SessionStatus::Unreachable).unwrap(),
            r#""unreachable""#
        );
    }

    #[test]
    fn session_status_from_str_roundtrip() {
        for (s, expected) in &[
            ("active", SessionStatus::Active),
            ("dead", SessionStatus::Dead),
            ("unreachable", SessionStatus::Unreachable),
            ("orphaned", SessionStatus::Orphaned),
        ] {
            assert_eq!(&s.parse::<SessionStatus>().unwrap(), expected);
        }
    }

    #[test]
    fn session_status_from_str_rejects_unknown() {
        assert!("unknown".parse::<SessionStatus>().is_err());
        assert!("Active".parse::<SessionStatus>().is_err());
    }

    #[test]
    fn transport_mode_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&TransportMode::Streamlocal).unwrap(),
            r#""streamlocal""#
        );
        assert_eq!(
            serde_json::to_string(&TransportMode::Tcp).unwrap(),
            r#""tcp""#
        );
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
