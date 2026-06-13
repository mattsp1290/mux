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

/// A validated SSH port number (1–65535).
///
/// Port 0 is rejected: it is the kernel-assigned ephemeral port and is never a valid
/// explicit SSH target. Port numbers above 65535 cannot exist on TCP/IP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Port(u16);

impl Port {
    pub fn value(self) -> u16 {
        self.0
    }
}

impl std::str::FromStr for Port {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n: u32 = s.parse().map_err(|_| MuxError::InvalidPort(s.to_owned()))?;
        if (1..=65535).contains(&n) {
            Ok(Port(n as u16))
        } else {
            Err(MuxError::InvalidPort(s.to_owned()))
        }
    }
}

impl<'de> Deserialize<'de> for Port {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Accept either a JSON number or a quoted string.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum NumOrStr {
            Num(u32),
            Str(String),
        }
        let v = NumOrStr::deserialize(deserializer)?;
        let n = match v {
            NumOrStr::Num(n) => n,
            NumOrStr::Str(s) => s
                .parse::<u32>()
                .map_err(|_| serde::de::Error::custom(format!("invalid port: {s}")))?,
        };
        if (1..=65535).contains(&n) {
            Ok(Port(n as u16))
        } else {
            Err(serde::de::Error::custom(format!("invalid port: {n}")))
        }
    }
}

impl std::fmt::Display for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for Port {
    fn default() -> Self {
        Port(22)
    }
}

/// A remote SSH endpoint in `user@addr` form.
///
/// `user` is a Unix username (no `@`). `addr` is a hostname or IP address (no `@`).
/// The spec (docs/01 §mux host add) does not specify further constraints on username
/// or address format; validation is conservative (non-empty, no `@` in either component).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Endpoint {
    pub user: String,
    pub addr: String,
}

impl std::str::FromStr for Endpoint {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '@');
        let user = parts.next().unwrap_or("").to_owned();
        let addr = parts.next().unwrap_or("").to_owned();
        if user.is_empty() || addr.is_empty() || user.contains('@') || addr.contains('@') {
            return Err(MuxError::InvalidEndpoint(s.to_owned()));
        }
        Ok(Endpoint { user, addr })
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.user, self.addr)
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
        assert!(
            result.is_err(),
            "deserialization should reject invalid alias"
        );
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

    #[test]
    fn port_valid() {
        assert_eq!("22".parse::<Port>().unwrap().value(), 22);
        assert_eq!("1".parse::<Port>().unwrap().value(), 1);
        assert_eq!("65535".parse::<Port>().unwrap().value(), 65535);
    }

    #[test]
    fn port_rejects_zero() {
        assert!("0".parse::<Port>().is_err());
    }

    #[test]
    fn port_rejects_above_max() {
        assert!("65536".parse::<Port>().is_err());
        assert!("99999".parse::<Port>().is_err());
    }

    #[test]
    fn port_rejects_non_numeric() {
        assert!("ssh".parse::<Port>().is_err());
        assert!("".parse::<Port>().is_err());
        assert!("-1".parse::<Port>().is_err());
    }

    #[test]
    fn port_default_is_22() {
        assert_eq!(Port::default().value(), 22);
    }

    #[test]
    fn port_display() {
        assert_eq!(Port::default().to_string(), "22");
    }

    #[test]
    fn port_serde_from_number() {
        let p: Port = serde_json::from_str("22").unwrap();
        assert_eq!(p.value(), 22);
    }

    #[test]
    fn port_serde_from_string() {
        let p: Port = serde_json::from_str(r#""8022""#).unwrap();
        assert_eq!(p.value(), 8022);
    }

    #[test]
    fn port_serde_rejects_zero() {
        assert!(serde_json::from_str::<Port>("0").is_err());
    }

    #[test]
    fn endpoint_valid() {
        let ep: Endpoint = "alice@192.168.1.1".parse().unwrap();
        assert_eq!(ep.user, "alice");
        assert_eq!(ep.addr, "192.168.1.1");
    }

    #[test]
    fn endpoint_valid_hostname() {
        let ep: Endpoint = "bob@host.example.com".parse().unwrap();
        assert_eq!(ep.user, "bob");
        assert_eq!(ep.addr, "host.example.com");
    }

    #[test]
    fn endpoint_display_roundtrip() {
        let s = "alice@192.168.1.1";
        let ep: Endpoint = s.parse().unwrap();
        assert_eq!(ep.to_string(), s);
    }

    #[test]
    fn endpoint_rejects_missing_at() {
        assert!("alice".parse::<Endpoint>().is_err());
        assert!("".parse::<Endpoint>().is_err());
    }

    #[test]
    fn endpoint_rejects_empty_user() {
        assert!("@host.example.com".parse::<Endpoint>().is_err());
    }

    #[test]
    fn endpoint_rejects_empty_addr() {
        assert!("alice@".parse::<Endpoint>().is_err());
    }

    #[test]
    fn endpoint_rejects_multiple_at() {
        // Only the first @ is the user/addr separator; a second @ in user is impossible
        // given splitn(2), but addr containing @ must be rejected.
        assert!("alice@host@extra".parse::<Endpoint>().is_err());
    }
}
