use std::path::PathBuf;

use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum MuxError {
    // ── Existing validation variants ────────────────────────────────────────
    #[error("invalid host alias: {0}")]
    InvalidHostAlias(String),

    /// Carries the raw input string so diagnostics can show what the user typed.
    #[error("invalid port: {0}")]
    InvalidPort(String),

    /// Raised when parsing `user@addr` endpoint strings.
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),

    /// Raised when parsing repository input strings (owner/repo or git@host:path.git).
    #[error("invalid repo: {0}")]
    InvalidRepo(String),

    /// Raised when loading session status from SQLite — implemented in mux-7sa.
    #[error("invalid session status: {0}")]
    InvalidSessionStatus(String),

    /// Raised when resolving the mux state directory — implemented in mux-init.
    #[error("home directory not found")]
    HomeDirNotFound,

    // ── Create-flow: user input errors (exit code 1) ─────────────────────────
    /// The working directory already exists on the remote host.
    /// Hint: "Remove the existing directory or use a different host."
    #[error("working directory already exists: {0}")]
    WorkdirPreExisting(PathBuf),

    /// The session shortname is already in use on that host.
    #[error("session already exists on host '{host}': shortname '{shortname}' is taken")]
    SessionAlreadyExists { host: String, shortname: String },

    /// All candidate shortnames derived from the repo were already taken on the host.
    #[error("all candidate shortnames are taken; no free shortname could be allocated")]
    ShortnameExhausted,

    // ── Create-flow: host errors (exit code 1) ───────────────────────────────
    /// SSH host key does not match the stored TOFU key.
    /// Hint: "Use `mux host trust <alias>` to review and rotate the key."
    #[error("host key mismatch — TOFU verification failed")]
    HostKeyMismatch,

    /// TCP connection refused to the given host:port.
    #[error("connection refused to {0}")]
    ConnectionRefused(String),

    /// TCP connection to the given host:port timed out.
    #[error("connection timed out for {0}")]
    ConnectionTimeout(String),

    // ── Create-flow: remote errors (exit code 1) ─────────────────────────────
    /// `git clone` exited with a non-zero status.
    #[error("git clone failed with exit code {exit_code}: {stderr}")]
    GitCloneFailed { exit_code: i32, stderr: String },

    /// SSH agent forwarding is unavailable (no socket or no loaded keys).
    /// Hint: "Run `ssh-add` to load your key into ssh-agent."
    #[error("SSH agent forwarding is not available")]
    SshAgentNotForwarded,

    /// The remote mux-agent returned an application-level error.
    #[error("agent error: {0}")]
    AgentError(String),

    /// RPC transport or protocol failure.
    #[error("RPC error: {0}")]
    RpcError(String),

    // ── Create-flow: internal errors (exit code 2) ───────────────────────────
    /// Catch-all for unexpected errors that do not fit a more specific category.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl MuxError {
    /// Returns a user-actionable hint for errors that require user intervention.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            MuxError::WorkdirPreExisting(_) => {
                Some("Remove the existing directory or use a different host.")
            }
            MuxError::SshAgentNotForwarded => {
                Some("Run `ssh-add` to load your key into ssh-agent.")
            }
            MuxError::HostKeyMismatch => {
                Some("Use `mux host trust <alias>` to review and rotate the key.")
            }
            _ => None,
        }
    }

    /// Returns the exit code that should be used when this error terminates the process.
    /// Category "internal" errors use exit code 2; all others use 1.
    pub fn exit_code(&self) -> i32 {
        match self {
            MuxError::Other(_) => 2,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::anyhow;

    use super::MuxError;

    // ── Existing variants ────────────────────────────────────────────────────

    #[test]
    fn invalid_host_alias_display() {
        let e = MuxError::InvalidHostAlias("bad alias!".into());
        assert!(e.to_string().contains("bad alias!"));
        assert!(e.hint().is_none());
    }

    #[test]
    fn invalid_port_display() {
        let e = MuxError::InvalidPort("99999".into());
        assert!(e.to_string().contains("99999"));
        assert!(e.hint().is_none());
    }

    #[test]
    fn invalid_endpoint_display() {
        let e = MuxError::InvalidEndpoint("nope".into());
        assert!(e.to_string().contains("nope"));
        assert!(e.hint().is_none());
    }

    #[test]
    fn invalid_repo_display() {
        let e = MuxError::InvalidRepo("not-a-repo".into());
        assert!(e.to_string().contains("not-a-repo"));
        assert!(e.hint().is_none());
    }

    #[test]
    fn invalid_session_status_display() {
        let e = MuxError::InvalidSessionStatus("unknown".into());
        assert!(e.to_string().contains("unknown"));
        assert!(e.hint().is_none());
    }

    #[test]
    fn home_dir_not_found_display() {
        let e = MuxError::HomeDirNotFound;
        assert!(e.to_string().contains("home directory not found"));
        assert!(e.hint().is_none());
    }

    // ── WorkdirPreExisting ───────────────────────────────────────────────────

    #[test]
    fn workdir_pre_existing_display() {
        let path = PathBuf::from("/home/user/projects/myrepo");
        let e = MuxError::WorkdirPreExisting(path.clone());
        let s = e.to_string();
        assert!(s.contains("myrepo"), "expected path in message, got: {s}");
    }

    #[test]
    fn workdir_pre_existing_hint() {
        let e = MuxError::WorkdirPreExisting(PathBuf::from("/tmp/x"));
        assert_eq!(
            e.hint(),
            Some("Remove the existing directory or use a different host.")
        );
    }

    // ── GitCloneFailed ───────────────────────────────────────────────────────

    #[test]
    fn git_clone_failed_display() {
        let e = MuxError::GitCloneFailed {
            exit_code: 128,
            stderr: "Repository not found".into(),
        };
        let s = e.to_string();
        assert!(s.contains("128"), "expected exit code, got: {s}");
        assert!(
            s.contains("Repository not found"),
            "expected stderr, got: {s}"
        );
    }

    #[test]
    fn git_clone_failed_no_hint() {
        let e = MuxError::GitCloneFailed {
            exit_code: 1,
            stderr: String::new(),
        };
        assert!(e.hint().is_none());
    }

    // ── SshAgentNotForwarded ─────────────────────────────────────────────────

    #[test]
    fn ssh_agent_not_forwarded_display() {
        let e = MuxError::SshAgentNotForwarded;
        assert!(e.to_string().contains("SSH agent forwarding"));
    }

    #[test]
    fn ssh_agent_not_forwarded_hint() {
        let e = MuxError::SshAgentNotForwarded;
        assert_eq!(
            e.hint(),
            Some("Run `ssh-add` to load your key into ssh-agent.")
        );
    }

    // ── SessionAlreadyExists ─────────────────────────────────────────────────

    #[test]
    fn session_already_exists_display() {
        let e = MuxError::SessionAlreadyExists {
            host: "prod-01".into(),
            shortname: "myrepo".into(),
        };
        let s = e.to_string();
        assert!(s.contains("prod-01"), "expected host, got: {s}");
        assert!(s.contains("myrepo"), "expected shortname, got: {s}");
    }

    #[test]
    fn session_already_exists_no_hint() {
        let e = MuxError::SessionAlreadyExists {
            host: "h".into(),
            shortname: "s".into(),
        };
        assert!(e.hint().is_none());
    }

    // ── ShortnameExhausted ───────────────────────────────────────────────────

    #[test]
    fn shortname_exhausted_display() {
        let e = MuxError::ShortnameExhausted;
        assert!(e.to_string().contains("candidate shortnames"));
    }

    #[test]
    fn shortname_exhausted_no_hint() {
        assert!(MuxError::ShortnameExhausted.hint().is_none());
    }

    // ── HostKeyMismatch ──────────────────────────────────────────────────────

    #[test]
    fn host_key_mismatch_display() {
        let e = MuxError::HostKeyMismatch;
        assert!(e.to_string().contains("TOFU"));
    }

    #[test]
    fn host_key_mismatch_hint() {
        let e = MuxError::HostKeyMismatch;
        assert_eq!(
            e.hint(),
            Some("Use `mux host trust <alias>` to review and rotate the key.")
        );
    }

    // ── ConnectionRefused ────────────────────────────────────────────────────

    #[test]
    fn connection_refused_display() {
        let e = MuxError::ConnectionRefused("prod-01:22".into());
        let s = e.to_string();
        assert!(s.contains("prod-01:22"), "expected host:port, got: {s}");
    }

    #[test]
    fn connection_refused_no_hint() {
        assert!(MuxError::ConnectionRefused("h:22".into()).hint().is_none());
    }

    // ── ConnectionTimeout ────────────────────────────────────────────────────

    #[test]
    fn connection_timeout_display() {
        let e = MuxError::ConnectionTimeout("remote.example.com:2222".into());
        let s = e.to_string();
        assert!(
            s.contains("remote.example.com:2222"),
            "expected host:port, got: {s}"
        );
    }

    #[test]
    fn connection_timeout_no_hint() {
        assert!(MuxError::ConnectionTimeout("h:22".into()).hint().is_none());
    }

    // ── AgentError ───────────────────────────────────────────────────────────

    #[test]
    fn agent_error_display() {
        let e = MuxError::AgentError("quota exceeded".into());
        assert!(e.to_string().contains("quota exceeded"));
    }

    #[test]
    fn agent_error_no_hint() {
        assert!(MuxError::AgentError("x".into()).hint().is_none());
    }

    // ── RpcError ─────────────────────────────────────────────────────────────

    #[test]
    fn rpc_error_display() {
        let e = MuxError::RpcError("broken pipe".into());
        assert!(e.to_string().contains("broken pipe"));
    }

    #[test]
    fn rpc_error_no_hint() {
        assert!(MuxError::RpcError("x".into()).hint().is_none());
    }

    // ── Other (catch-all) ────────────────────────────────────────────────────

    #[test]
    fn other_display() {
        let e = MuxError::Other(anyhow!("something went very wrong"));
        assert!(e.to_string().contains("something went very wrong"));
    }

    #[test]
    fn other_no_hint() {
        let e: MuxError = anyhow!("oops").into();
        assert!(e.hint().is_none());
    }

    #[test]
    fn other_exit_code_is_2() {
        let e: MuxError = anyhow!("internal").into();
        assert_eq!(e.exit_code(), 2);
    }

    #[test]
    fn non_internal_exit_code_is_1() {
        assert_eq!(MuxError::ShortnameExhausted.exit_code(), 1);
        assert_eq!(MuxError::HostKeyMismatch.exit_code(), 1);
        assert_eq!(MuxError::SshAgentNotForwarded.exit_code(), 1);
    }
}
