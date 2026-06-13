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

impl TryFrom<u16> for Port {
    type Error = MuxError;

    fn try_from(n: u16) -> Result<Self, Self::Error> {
        if n == 0 {
            Err(MuxError::InvalidPort(n.to_string()))
        } else {
            Ok(Port(n))
        }
    }
}

/// A remote SSH endpoint in `user@addr` form.
///
/// `user` is a Unix username (no `@`). `addr` is a hostname or IP address (no `@`).
/// The spec (docs/01 §mux host add) does not specify further constraints on username
/// or address format; validation is conservative (non-empty, no `@` in addr component).
/// Serializes/deserializes as a flat `"user@addr"` string, consistent with `Display`.
/// Argv-safety constraints (e.g., leading hyphens) are enforced at the SSH invocation
/// layer, not here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    user: String,
    addr: String,
}

impl Endpoint {
    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }
}

impl Serialize for Endpoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Endpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl std::str::FromStr for Endpoint {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '@');
        let user = parts.next().unwrap_or("").to_owned();
        let addr = parts.next().unwrap_or("").to_owned();
        if user.is_empty() || addr.is_empty() || addr.contains('@') {
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

/// A normalised repository reference, parsed from `owner/repo` or `git@host:path.git` forms.
///
/// Spec: docs/02 §Repo normalisation
///
/// Rules:
/// - `owner/repo` → owner and repo extracted, host=None.
/// - `git@host:path.git` → owner/repo extracted from path, host=Some(host).
/// - `owner/repo.git` shorthand is explicitly **rejected** (ambiguous `.git` suffix).
/// - `.git` suffix is matched case-insensitively (after lowercasing the path).
/// - Owner and repo are lowercased and stored canonically.
/// - An empty owner or repo component is rejected.
/// - Owner and repo must each contain at least one ASCII alphanumeric character to
///   guarantee a non-empty `storage_slug` (a filesystem path component).
/// - `Display` always produces the canonical form: `git@host:owner/repo.git` for
///   git@ inputs (with `.git` re-added if it was absent), `owner/repo` for slug inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    owner: String,
    repo: String,
    host: Option<String>,
}

impl RepoRef {
    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Canonical owner/repo identifier used in storage and RPC comparisons.
    pub fn repo_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Filesystem-safe identifier: lowercase, non-alnum replaced with hyphens.
    pub fn storage_slug(&self) -> String {
        let raw = format!("{}-{}", self.owner, self.repo);
        let mut slug = String::new();
        let mut prev_hyphen = false;
        for c in raw.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c);
                prev_hyphen = false;
            } else if !prev_hyphen {
                slug.push('-');
                prev_hyphen = true;
            }
        }
        slug.trim_matches('-').to_owned()
    }

    /// Git clone URL. Requires a host — returns `None` if no host was present in the input.
    /// Use `clone_url_for` to supply a fallback host.
    pub fn clone_url(&self) -> Option<String> {
        self.host
            .as_ref()
            .map(|h| format!("git@{}:{}/{}.git", h, self.owner, self.repo))
    }

    /// Git clone URL, using the stored host or `default_host` if none was parsed.
    pub fn clone_url_for(&self, default_host: &str) -> String {
        let h = self.host.as_deref().unwrap_or(default_host);
        format!("git@{}:{}/{}.git", h, self.owner, self.repo)
    }

    /// The `repo` component — used as the working-directory leaf name.
    pub fn repo_leaf(&self) -> &str {
        &self.repo
    }
}

impl std::str::FromStr for RepoRef {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || MuxError::InvalidRepo(s.to_owned());

        if let Some(rest) = s.strip_prefix("git@") {
            // Parse git@host:owner/repo.git
            let colon = rest.find(':').ok_or_else(err)?;
            let host = rest[..colon].to_ascii_lowercase();
            // Lowercase before stripping ".git" so "r.GIT" is treated the same as "r.git".
            let path_lower = rest[colon + 1..].to_ascii_lowercase();
            let path = path_lower.strip_suffix(".git").unwrap_or(&path_lower);
            let slash = path.find('/').ok_or_else(err)?;
            let owner = path[..slash].to_owned();
            let repo = path[slash + 1..].to_owned();
            if owner.is_empty()
                || repo.is_empty()
                || repo.contains('/')
                || host.is_empty()
                || !owner.chars().any(|c| c.is_ascii_alphanumeric())
                || !repo.chars().any(|c| c.is_ascii_alphanumeric())
            {
                return Err(err());
            }
            Ok(RepoRef {
                owner,
                repo,
                host: Some(host),
            })
        } else {
            // Parse owner/repo — reject owner/repo.git shorthand (case-insensitive)
            let s_lower = s.to_ascii_lowercase();
            if s_lower.ends_with(".git") {
                return Err(MuxError::InvalidRepo(format!(
                    "{s}: use 'owner/repo' not 'owner/repo.git'"
                )));
            }
            let slash = s_lower.find('/').ok_or_else(err)?;
            let owner = s_lower[..slash].to_owned();
            let repo = s_lower[slash + 1..].to_owned();
            if owner.is_empty()
                || repo.is_empty()
                || repo.contains('/')
                || !owner.chars().any(|c| c.is_ascii_alphanumeric())
                || !repo.chars().any(|c| c.is_ascii_alphanumeric())
            {
                return Err(err());
            }
            Ok(RepoRef {
                owner,
                repo,
                host: None,
            })
        }
    }
}

impl std::fmt::Display for RepoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.host {
            Some(h) => write!(f, "git@{}:{}/{}.git", h, self.owner, self.repo),
            None => write!(f, "{}/{}", self.owner, self.repo),
        }
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

impl std::str::FromStr for TransportMode {
    type Err = MuxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "streamlocal" => Ok(Self::Streamlocal),
            "tcp" => Ok(Self::Tcp),
            _ => Err(MuxError::InvalidForceTransport(s.to_owned())),
        }
    }
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
    fn transport_mode_from_str() {
        assert_eq!("streamlocal".parse::<TransportMode>().unwrap(), TransportMode::Streamlocal);
        assert_eq!("tcp".parse::<TransportMode>().unwrap(), TransportMode::Tcp);
        assert!(matches!(
            "invalid".parse::<TransportMode>(),
            Err(MuxError::InvalidForceTransport(s)) if s == "invalid"
        ));
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
        assert_eq!(ep.user(), "alice");
        assert_eq!(ep.addr(), "192.168.1.1");
    }

    #[test]
    fn endpoint_valid_hostname() {
        let ep: Endpoint = "bob@host.example.com".parse().unwrap();
        assert_eq!(ep.user(), "bob");
        assert_eq!(ep.addr(), "host.example.com");
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
        assert!("alice@host@extra".parse::<Endpoint>().is_err());
    }

    #[test]
    fn endpoint_serde_roundtrip() {
        let ep: Endpoint = "alice@192.168.1.1".parse().unwrap();
        let json = serde_json::to_string(&ep).unwrap();
        assert_eq!(json, r#""alice@192.168.1.1""#);
        let back: Endpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(ep, back);
    }

    #[test]
    fn endpoint_deserialize_rejects_invalid() {
        assert!(serde_json::from_str::<Endpoint>(r#""noatsign""#).is_err());
        assert!(serde_json::from_str::<Endpoint>(r#""@addr""#).is_err());
        assert!(serde_json::from_str::<Endpoint>(r#""user@""#).is_err());
    }

    #[test]
    fn port_serde_rejects_above_max() {
        assert!(serde_json::from_str::<Port>("65536").is_err());
        assert!(serde_json::from_str::<Port>(r#""65536""#).is_err());
    }

    #[test]
    fn port_try_from_u16() {
        assert_eq!(Port::try_from(22u16).unwrap().value(), 22);
        assert_eq!(Port::try_from(65535u16).unwrap().value(), 65535);
        assert!(Port::try_from(0u16).is_err());
    }

    // RepoRef tests

    #[test]
    fn repo_ref_owner_repo_form() {
        let r: RepoRef = "mattsp1290/mux".parse().unwrap();
        assert_eq!(r.owner(), "mattsp1290");
        assert_eq!(r.repo(), "mux");
        assert_eq!(r.host(), None);
        assert_eq!(r.repo_slug(), "mattsp1290/mux");
        assert_eq!(r.repo_leaf(), "mux");
    }

    #[test]
    fn repo_ref_git_url_form() {
        let r: RepoRef = "git@github.com:mattsp1290/mux.git".parse().unwrap();
        assert_eq!(r.owner(), "mattsp1290");
        assert_eq!(r.repo(), "mux");
        assert_eq!(r.host(), Some("github.com"));
        assert_eq!(r.repo_slug(), "mattsp1290/mux");
        assert_eq!(r.clone_url().unwrap(), "git@github.com:mattsp1290/mux.git");
    }

    #[test]
    fn repo_ref_rejects_dot_git_shorthand() {
        assert!("mattsp1290/mux.git".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_rejects_empty_owner() {
        assert!("/mux".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_rejects_empty_repo() {
        assert!("mattsp1290/".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_rejects_no_slash() {
        assert!("mattsp1290".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_rejects_too_many_slashes() {
        // org/sub/repo is not a valid two-component owner/repo
        assert!("org/sub/repo".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_lowercases_input() {
        let r: RepoRef = "MyOrg/MyRepo".parse().unwrap();
        assert_eq!(r.owner(), "myorg");
        assert_eq!(r.repo(), "myrepo");
        assert_eq!(r.repo_slug(), "myorg/myrepo");
    }

    #[test]
    fn repo_ref_storage_slug_replaces_non_alnum() {
        let r: RepoRef = "my-org/my_repo".parse().unwrap();
        // owner has hyphen (ok), repo has underscore → replaced with hyphen
        assert_eq!(r.storage_slug(), "my-org-my-repo");
    }

    #[test]
    fn repo_ref_storage_slug_collapses_hyphens() {
        let r: RepoRef = "my.org/my.repo".parse().unwrap();
        // dots → hyphens, no consecutive hyphens
        assert_eq!(r.storage_slug(), "my-org-my-repo");
    }

    #[test]
    fn repo_ref_display_owner_repo_roundtrip() {
        let s = "mattsp1290/mux";
        let r: RepoRef = s.parse().unwrap();
        assert_eq!(r.to_string(), s);
    }

    #[test]
    fn repo_ref_display_git_url_roundtrip() {
        let s = "git@github.com:mattsp1290/mux.git";
        let r: RepoRef = s.parse().unwrap();
        assert_eq!(r.to_string(), s);
    }

    #[test]
    fn repo_ref_clone_url_for_default_host() {
        let r: RepoRef = "mattsp1290/mux".parse().unwrap();
        assert_eq!(
            r.clone_url_for("github.com"),
            "git@github.com:mattsp1290/mux.git"
        );
    }

    #[test]
    fn repo_ref_git_url_no_dot_git_is_ok() {
        // git@ form without trailing .git is accepted (strip_suffix returns original)
        let r: RepoRef = "git@github.com:mattsp1290/mux".parse().unwrap();
        assert_eq!(r.owner(), "mattsp1290");
        assert_eq!(r.repo(), "mux");
    }

    #[test]
    fn repo_ref_git_url_rejects_empty_host() {
        assert!("git@:mattsp1290/mux.git".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_git_url_rejects_no_colon() {
        assert!("git@github.com/mattsp1290/mux.git"
            .parse::<RepoRef>()
            .is_err());
    }

    #[test]
    fn repo_ref_git_url_case_insensitive_dot_git() {
        // .GIT and .Git should be stripped the same as .git
        let r: RepoRef = "git@github.com:mattsp1290/mux.GIT".parse().unwrap();
        assert_eq!(r.repo(), "mux", ".GIT should be stripped");
        let r: RepoRef = "git@github.com:mattsp1290/mux.Git".parse().unwrap();
        assert_eq!(r.repo(), "mux", ".Git should be stripped");
    }

    #[test]
    fn repo_ref_rejects_non_alnum_only_owner() {
        // All-non-alnum owner would produce empty storage_slug
        assert!("_/repo".parse::<RepoRef>().is_err());
        assert!("./repo".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_rejects_non_alnum_only_repo() {
        // All-non-alnum repo would produce empty storage_slug
        assert!("owner/_".parse::<RepoRef>().is_err());
        assert!("owner/.".parse::<RepoRef>().is_err());
    }

    #[test]
    fn repo_ref_git_url_no_dot_git_display_adds_git() {
        // Display always produces canonical form (with .git), even when input lacked it
        let r: RepoRef = "git@github.com:mattsp1290/mux".parse().unwrap();
        assert_eq!(r.to_string(), "git@github.com:mattsp1290/mux.git");
    }
}
