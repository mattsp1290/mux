use std::path::PathBuf;

use thiserror::Error;

/// Maximum bytes of stderr captured from a git clone failure.
/// Prevents multi-KB output from polluting structured logs or user-visible messages.
pub const MAX_STDERR_BYTES: usize = 2048;

/// Truncate stderr from a spawned process to at most MAX_STDERR_BYTES bytes and
/// redact any `user:pass@` credential fragments that git may echo in clone URLs.
///
/// Callers constructing `MuxError::GitCloneFailed` must pass stderr through this
/// function so the invariant is enforced at construction rather than Display time.
pub fn truncate_stderr(s: &str) -> String {
    // Redact credentials of the form `user:pass@` in URLs.
    let redacted = {
        // Simple state-machine redaction: replace `word:word@` patterns.
        let mut out = String::with_capacity(s.len().min(MAX_STDERR_BYTES + 64));
        let mut rest = s;
        while let Some(at_pos) = rest.find('@') {
            let before = &rest[..at_pos];
            if let Some(colon_pos) = before.rfind(':') {
                // Check the segment before the colon looks like a password (no spaces).
                let user_pass = &before[colon_pos + 1..];
                if !user_pass.contains(' ') && !user_pass.is_empty() {
                    out.push_str(&before[..colon_pos + 1]);
                    out.push_str("****@");
                    rest = &rest[at_pos + 1..];
                    continue;
                }
            }
            out.push_str(&before[..at_pos + 1]);
            rest = &rest[at_pos + 1..];
        }
        out.push_str(rest);
        out
    };
    // Truncate to MAX_STDERR_BYTES bytes (on a char boundary).
    if redacted.len() <= MAX_STDERR_BYTES {
        redacted
    } else {
        let truncated = &redacted[..redacted
            .char_indices()
            .take_while(|(i, _)| *i < MAX_STDERR_BYTES)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0)];
        format!("{truncated}… [truncated]")
    }
}

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

    /// Raised when resolving the mux state directory.
    /// Exit code 1 (user input category): the user must set HOME or run in a valid environment.
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
    ///
    /// `stderr` must be pre-processed with `truncate_stderr()` before construction
    /// to cap length and redact any `user:pass@` credential fragments.
    #[error("git clone failed with exit code {exit_code}: {stderr}")]
    GitCloneFailed { exit_code: i32, stderr: String },

    /// SSH agent forwarding is unavailable (no socket or no loaded keys).
    /// Hint: "Run `ssh-add` to load your key into ssh-agent."
    #[error("SSH agent forwarding is not available")]
    SshAgentNotForwarded,

    /// SSH private key is encrypted and cannot be used without unlocking.
    /// Hint: "Run `ssh-add` to unlock the key."
    #[error("SSH key is encrypted; run `ssh-add` to unlock")]
    SshKeyEncrypted,

    /// Server host key encountered for first time with no interactive terminal.
    #[error("TOFU requires an interactive terminal for first-contact verification")]
    TofuNonInteractive,

    /// User declined the TOFU first-contact trust prompt.
    #[error("host key rejected by user")]
    HostKeyRejected,

    /// MUX_FORCE_TRANSPORT env var has an invalid value.
    /// Hint: "Valid values are 'streamlocal' and 'tcp'."
    #[error("invalid MUX_FORCE_TRANSPORT value: {0:?}; expected 'streamlocal' or 'tcp'")]
    InvalidForceTransport(String),

    /// The remote mux-agent returned an application-level error.
    #[error("agent error: {0}")]
    AgentError(String),

    /// Agent did not become ready within the startup timeout.
    /// Hint: "Check agent.log on the remote host for details."
    #[error("agent start timed out; last log:\n{log_tail}")]
    AgentStartTimeout { log_tail: String },

    /// RPC transport or protocol failure.
    #[error("RPC error: {0}")]
    RpcError(String),

    // ── Create-flow: internal errors (exit code 2) ───────────────────────────
    /// Catch-all for genuinely unexpected/internal errors (DB corruption, impossible
    /// state, etc.). Reserved for errors that have no more-specific variant — do NOT
    /// route classifiable errors (SSH, RPC, git) through `Other`. Callers that need
    /// `anyhow::Error` → `MuxError` for a known category must map to the specific
    /// variant first; only fall back to `?` (which invokes `From<anyhow::Error>`)
    /// when the error truly is unclassifiable.
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
            MuxError::SshKeyEncrypted => Some("Run `ssh-add` to unlock the key."),
            MuxError::TofuNonInteractive => {
                Some("Run `mux host test <alias>` interactively to establish trust.")
            }
            MuxError::HostKeyRejected => {
                Some("Trust the host first with `mux host test <alias>`.")
            }
            MuxError::InvalidForceTransport(_) => {
                Some("Valid values are 'streamlocal' and 'tcp'.")
            }
            MuxError::AgentStartTimeout { .. } => {
                Some("Check agent.log on the remote host.")
            }
            _ => None,
        }
    }

    /// Returns the canonical error category string for observability (docs/08).
    ///
    /// This is the single source of truth for `CreateFlowMetrics.error_category`.
    /// Callers must use `err.category()` rather than ad-hoc string literals.
    pub fn category(&self) -> &'static str {
        match self {
            MuxError::WorkdirPreExisting(_) => "workdir_pre_existing",
            MuxError::GitCloneFailed { .. } => "git_clone_failed",
            MuxError::SshAgentNotForwarded => "ssh_agent_not_forwarded",
            MuxError::SshKeyEncrypted => "ssh_key_encrypted",
            MuxError::TofuNonInteractive => "tofu_non_interactive",
            MuxError::HostKeyRejected => "host_key_rejected",
            MuxError::SessionAlreadyExists { .. } => "session_already_exists",
            MuxError::ShortnameExhausted => "shortname_exhausted",
            MuxError::RpcError(_) | MuxError::AgentError(_) => "rpc_error",
            MuxError::AgentStartTimeout { .. } => "agent_start_timeout",
            MuxError::InvalidForceTransport(_) => "invalid_force_transport",
            MuxError::Other(_) => "other",
            // Remaining variants: user-input or host errors without a dedicated spec category.
            _ => "other",
        }
    }

    /// Returns the exit code that should be used when this error terminates the process.
    ///
    /// Exit code 2 = internal error (MuxError::Other). All other categories = 1.
    /// Explicit arms prevent future variants from silently inheriting the wrong code.
    pub fn exit_code(&self) -> i32 {
        match self {
            MuxError::InvalidHostAlias(_)
            | MuxError::InvalidPort(_)
            | MuxError::InvalidEndpoint(_)
            | MuxError::InvalidRepo(_)
            | MuxError::InvalidSessionStatus(_)
            | MuxError::HomeDirNotFound
            | MuxError::WorkdirPreExisting(_)
            | MuxError::SessionAlreadyExists { .. }
            | MuxError::ShortnameExhausted
            | MuxError::HostKeyMismatch
            | MuxError::ConnectionRefused(_)
            | MuxError::ConnectionTimeout(_)
            | MuxError::GitCloneFailed { .. }
            | MuxError::SshAgentNotForwarded
            | MuxError::SshKeyEncrypted
            | MuxError::TofuNonInteractive
            | MuxError::HostKeyRejected
            | MuxError::AgentError(_)
            | MuxError::AgentStartTimeout { .. }
            | MuxError::RpcError(_)
            | MuxError::InvalidForceTransport(_) => 1,
            MuxError::Other(_) => 2,
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

    // ── SshKeyEncrypted ──────────────────────────────────────────────────────

    #[test]
    fn ssh_key_encrypted_display() {
        let e = MuxError::SshKeyEncrypted;
        assert!(e.to_string().contains("encrypted"));
    }

    #[test]
    fn ssh_key_encrypted_hint() {
        let e = MuxError::SshKeyEncrypted;
        assert_eq!(e.hint(), Some("Run `ssh-add` to unlock the key."));
    }

    #[test]
    fn ssh_key_encrypted_exit_code() {
        assert_eq!(MuxError::SshKeyEncrypted.exit_code(), 1);
    }

    #[test]
    fn ssh_key_encrypted_category() {
        assert_eq!(MuxError::SshKeyEncrypted.category(), "ssh_key_encrypted");
    }

    // ── TofuNonInteractive ───────────────────────────────────────────────────

    #[test]
    fn tofu_non_interactive_display() {
        let e = MuxError::TofuNonInteractive;
        assert!(e.to_string().contains("interactive"));
    }

    #[test]
    fn tofu_non_interactive_hint() {
        let e = MuxError::TofuNonInteractive;
        assert_eq!(
            e.hint(),
            Some("Run `mux host test <alias>` interactively to establish trust.")
        );
    }

    #[test]
    fn tofu_non_interactive_exit_code() {
        assert_eq!(MuxError::TofuNonInteractive.exit_code(), 1);
    }

    #[test]
    fn tofu_non_interactive_category() {
        assert_eq!(MuxError::TofuNonInteractive.category(), "tofu_non_interactive");
    }

    // ── HostKeyRejected ──────────────────────────────────────────────────────

    #[test]
    fn host_key_rejected_display() {
        let e = MuxError::HostKeyRejected;
        assert!(e.to_string().contains("rejected"));
    }

    #[test]
    fn host_key_rejected_hint() {
        let e = MuxError::HostKeyRejected;
        assert_eq!(
            e.hint(),
            Some("Trust the host first with `mux host test <alias>`.")
        );
    }

    #[test]
    fn host_key_rejected_exit_code() {
        assert_eq!(MuxError::HostKeyRejected.exit_code(), 1);
    }

    #[test]
    fn host_key_rejected_category() {
        assert_eq!(MuxError::HostKeyRejected.category(), "host_key_rejected");
    }

    // ── category() ───────────────────────────────────────────────────────────

    #[test]
    fn category_all_spec_values_present() {
        let allowed: &[&str] = &[
            "workdir_pre_existing",
            "git_clone_failed",
            "ssh_agent_not_forwarded",
            "ssh_key_encrypted",
            "tofu_non_interactive",
            "host_key_rejected",
            "session_already_exists",
            "shortname_exhausted",
            "rpc_error",
            "other",
        ];
        let cases: &[MuxError] = &[
            MuxError::WorkdirPreExisting(PathBuf::from("/tmp")),
            MuxError::GitCloneFailed {
                exit_code: 1,
                stderr: String::new(),
            },
            MuxError::SshAgentNotForwarded,
            MuxError::SessionAlreadyExists {
                host: "h".into(),
                shortname: "s".into(),
            },
            MuxError::ShortnameExhausted,
            MuxError::RpcError("x".into()),
            MuxError::Other(anyhow!("boom")),
        ];
        for e in cases {
            let cat = e.category();
            assert!(
                allowed.contains(&cat),
                "category '{cat}' is not in the allowed spec list for variant {e:?}"
            );
        }
    }

    #[test]
    fn other_category_is_other() {
        let e: MuxError = anyhow!("internal").into();
        assert_eq!(e.category(), "other");
        assert_eq!(e.exit_code(), 2);
    }

    // ── truncate_stderr ───────────────────────────────────────────────────────

    #[test]
    fn truncate_stderr_passes_through_short_input() {
        let s = "fatal: repository not found";
        assert_eq!(super::truncate_stderr(s), s);
    }

    #[test]
    fn truncate_stderr_caps_long_input() {
        let long = "x".repeat(super::MAX_STDERR_BYTES + 100);
        let result = super::truncate_stderr(&long);
        assert!(
            result.len() <= super::MAX_STDERR_BYTES + 64,
            "truncated result too long: {}",
            result.len()
        );
        assert!(result.contains("[truncated]"));
    }

    #[test]
    fn truncate_stderr_redacts_credentials() {
        let s = "clone https://user:secret@github.com/org/repo failed";
        let result = super::truncate_stderr(s);
        assert!(
            !result.contains("secret"),
            "credential not redacted: {result}"
        );
        assert!(
            result.contains("****@"),
            "redaction marker missing: {result}"
        );
        assert!(
            result.contains("github.com"),
            "URL host should be preserved: {result}"
        );
    }

    // ── No mux: prefix in error messages ─────────────────────────────────────

    #[test]
    fn no_variant_display_starts_with_mux_prefix() {
        // Guard against double-prefixing: the CLI adds "mux: " at the boundary.
        // Error messages must NOT include it themselves.
        let variants: &[MuxError] = &[
            MuxError::InvalidHostAlias("x".into()),
            MuxError::WorkdirPreExisting(PathBuf::from("/tmp")),
            MuxError::GitCloneFailed {
                exit_code: 1,
                stderr: "e".into(),
            },
            MuxError::SshAgentNotForwarded,
            MuxError::HostKeyMismatch,
            MuxError::RpcError("x".into()),
            MuxError::Other(anyhow!("boom")),
        ];
        for e in variants {
            let s = e.to_string();
            assert!(
                !s.starts_with("mux:"),
                "variant {e:?} display starts with 'mux:': {s}"
            );
        }
    }
}
