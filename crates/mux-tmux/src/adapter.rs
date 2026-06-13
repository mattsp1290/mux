//! TmuxAdapter — direct argv invocation of tmux, no shell.
//!
//! Spec: prompts/docs/tmux-contract.md

use thiserror::Error;
use tokio::process::Command;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum TmuxError {
    #[error("invalid session name: {0}")]
    InvalidSessionName(String),

    #[error("invalid workdir: {0}")]
    InvalidWorkdir(String),

    #[error("invalid status string: {0}")]
    InvalidStatusString(String),

    #[error("tmux command '{command}' failed with exit code {exit_code:?}: {stderr}")]
    TmuxFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("failed to spawn tmux process: {0}")]
    SpawnFailed(String),
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Information about a single mux-managed tmux session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub name: String,
    pub created: u64,  // Unix timestamp from #{session_created}
    pub activity: u64, // Unix timestamp from #{session_activity}
}

// ── Adapter ───────────────────────────────────────────────────────────────────

/// Adapter that invokes tmux via direct argv (no shell, no sh -c).
#[derive(Debug, Clone)]
pub struct TmuxAdapter {
    tmux_bin: String,
}

impl Default for TmuxAdapter {
    fn default() -> Self {
        Self {
            tmux_bin: "tmux".to_owned(),
        }
    }
}

impl TmuxAdapter {
    /// Create an adapter using the default `tmux` binary on PATH.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an adapter with a specific binary path (useful for testing).
    pub fn with_bin(bin: impl Into<String>) -> Self {
        Self {
            tmux_bin: bin.into(),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Create a new tmux session.
    ///
    /// Runs: `tmux new-session -d -s <name> -c <workdir>`
    /// Then optionally: `tmux set-option -t <name> status on`
    ///               and `tmux set-option -t <name> status-right <status_right>`
    pub async fn new_session(
        &self,
        name: &str,
        workdir: &str,
        status_right: Option<&str>,
    ) -> Result<(), TmuxError> {
        validate_session_name(name)?;
        validate_workdir(workdir)?;
        if let Some(s) = status_right {
            validate_status_string(s)?;
        }

        self.run_tmux(&["new-session", "-d", "-s", name, "-c", workdir])
            .await?;

        if let Some(status) = status_right {
            self.run_tmux(&["set-option", "-t", name, "status", "on"])
                .await?;
            self.run_tmux(&["set-option", "-t", name, "status-right", status])
                .await?;
        }

        Ok(())
    }

    /// Kill a mux-managed tmux session.
    ///
    /// Runs: `tmux kill-session -t <name>`
    pub async fn kill_session(&self, name: &str) -> Result<(), TmuxError> {
        validate_session_name(name)?;
        self.run_tmux(&["kill-session", "-t", name]).await?;
        Ok(())
    }

    /// List all mux-managed tmux sessions (those with the `mux-` prefix).
    ///
    /// Runs: `tmux list-sessions -F '#{session_name}\t#{session_created}\t#{session_activity}'`
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, TmuxError> {
        let output = self
            .run_tmux(&[
                "list-sessions",
                "-F",
                "#{session_name}\t#{session_created}\t#{session_activity}",
            ])
            .await?;
        Ok(parse_list_output(&output))
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Run a tmux subcommand with the given arguments (direct argv, no shell).
    /// Returns stdout as a String on success, or a TmuxError on failure.
    async fn run_tmux(&self, args: &[&str]) -> Result<String, TmuxError> {
        let output = Command::new(&self.tmux_bin)
            .args(args)
            .output()
            .await
            .map_err(|e| TmuxError::SpawnFailed(e.to_string()))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let command = format!("{} {}", self.tmux_bin, args.join(" "));
            let exit_code = output.status.code();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(TmuxError::TmuxFailed {
                command,
                exit_code,
                stderr,
            })
        }
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

/// Parse the output of `tmux list-sessions -F '#{session_name}\t#{session_created}\t#{session_activity}'`.
///
/// Exposed as a standalone function so it can be unit-tested without invoking tmux.
pub fn parse_list_output(output: &str) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    for line in output.lines() {
        // Strip carriage returns (CRLF handling)
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 3 {
            tracing::debug!(
                "tmux list-sessions: skipping malformed row ({} fields): {:?}",
                fields.len(),
                line
            );
            continue;
        }
        let name = fields[0];
        // Filter: only mux-managed sessions
        if !name.starts_with("mux-") {
            continue;
        }
        let created = match fields[1].parse::<u64>() {
            Ok(v) => v,
            Err(_) => {
                tracing::debug!(
                    "tmux list-sessions: skipping row with unparseable created timestamp: {:?}",
                    line
                );
                continue;
            }
        };
        let activity = match fields[2].parse::<u64>() {
            Ok(v) => v,
            Err(_) => {
                tracing::debug!(
                    "tmux list-sessions: skipping row with unparseable activity timestamp: {:?}",
                    line
                );
                continue;
            }
        };
        sessions.push(SessionInfo {
            name: name.to_owned(),
            created,
            activity,
        });
    }
    sessions
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_session_name(name: &str) -> Result<(), TmuxError> {
    if name.is_empty() {
        return Err(TmuxError::InvalidSessionName(
            "session name must not be empty".to_owned(),
        ));
    }
    if !name.starts_with("mux-") {
        return Err(TmuxError::InvalidSessionName(format!(
            "session name must start with 'mux-', got: {name:?}"
        )));
    }
    Ok(())
}

fn validate_workdir(workdir: &str) -> Result<(), TmuxError> {
    if workdir.is_empty() {
        return Err(TmuxError::InvalidWorkdir(
            "workdir must not be empty".to_owned(),
        ));
    }
    if !workdir.starts_with('/') {
        return Err(TmuxError::InvalidWorkdir(format!(
            "workdir must be an absolute path (starts with /), got: {workdir:?}"
        )));
    }
    Ok(())
}

fn validate_status_string(s: &str) -> Result<(), TmuxError> {
    if !is_valid_status_string(s) {
        return Err(TmuxError::InvalidStatusString(format!(
            "status string contains shell-special characters: {s:?}"
        )));
    }
    Ok(())
}

/// Returns true if the status string contains no shell-special characters.
/// Forbidden: `$`, backtick, `\`, `"`, `'`, `|`, `&`, `;`, `<`, `>`
fn is_valid_status_string(s: &str) -> bool {
    const FORBIDDEN: &[char] = &['$', '`', '\\', '"', '\'', '|', '&', ';', '<', '>'];
    !s.chars().any(|c| FORBIDDEN.contains(&c))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Validation: session name ──────────────────────────────────────────────

    #[test]
    fn valid_session_name_passes() {
        assert!(validate_session_name("mux-my-session").is_ok());
        assert!(validate_session_name("mux-a").is_ok());
        assert!(validate_session_name("mux-123").is_ok());
    }

    #[test]
    fn empty_session_name_fails() {
        let err = validate_session_name("").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidSessionName(_)));
        let msg = err.to_string();
        assert!(msg.contains("empty"), "expected 'empty' in: {msg}");
    }

    #[test]
    fn session_name_without_mux_prefix_fails() {
        let err = validate_session_name("my-session").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidSessionName(_)));
    }

    #[test]
    fn mux_prefix_alone_is_valid() {
        // "mux-" with nothing after is technically valid per the spec
        assert!(validate_session_name("mux-").is_ok());
    }

    // ── Validation: workdir ───────────────────────────────────────────────────

    #[test]
    fn absolute_workdir_passes() {
        assert!(validate_workdir("/home/user/.mux/abc").is_ok());
        assert!(validate_workdir("/").is_ok());
        assert!(validate_workdir("/tmp").is_ok());
    }

    #[test]
    fn empty_workdir_fails() {
        let err = validate_workdir("").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidWorkdir(_)));
    }

    #[test]
    fn relative_workdir_fails() {
        let err = validate_workdir("relative/path").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidWorkdir(_)));
    }

    #[test]
    fn workdir_starting_with_tilde_fails() {
        // ~ is not an absolute path (doesn't start with /)
        let err = validate_workdir("~/projects").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidWorkdir(_)));
    }

    // ── Validation: status string ─────────────────────────────────────────────

    #[test]
    fn safe_status_string_passes() {
        assert!(is_valid_status_string("session: my-project [active]"));
        assert!(is_valid_status_string("mux 1.0 running"));
        assert!(is_valid_status_string(""));
        assert!(is_valid_status_string("foo bar baz"));
    }

    #[test]
    fn status_string_with_dollar_fails() {
        let err = validate_status_string("$USER is logged in").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidStatusString(_)));
    }

    #[test]
    fn status_string_with_backtick_fails() {
        let err = validate_status_string("cmd: `echo hi`").unwrap_err();
        assert!(matches!(err, TmuxError::InvalidStatusString(_)));
    }

    #[test]
    fn status_string_all_forbidden_chars_fail() {
        for ch in ['$', '`', '\\', '"', '\'', '|', '&', ';', '<', '>'] {
            let s = format!("bad{ch}char");
            assert!(
                !is_valid_status_string(&s),
                "expected {ch:?} to be forbidden in status string"
            );
        }
    }

    // ── Parsing: parse_list_output ────────────────────────────────────────────

    #[test]
    fn parse_empty_output_returns_empty_vec() {
        let result = parse_list_output("");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_normal_two_mux_sessions() {
        let output = "mux-alpha\t1700000000\t1700000100\nmux-beta\t1700001000\t1700001200\n";
        let sessions = parse_list_output(output);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "mux-alpha");
        assert_eq!(sessions[0].created, 1700000000);
        assert_eq!(sessions[0].activity, 1700000100);
        assert_eq!(sessions[1].name, "mux-beta");
        assert_eq!(sessions[1].created, 1700001000);
        assert_eq!(sessions[1].activity, 1700001200);
    }

    #[test]
    fn parse_strips_crlf_line_endings() {
        let output = "mux-session\t1700000000\t1700000100\r\n";
        let sessions = parse_list_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "mux-session");
        assert_eq!(sessions[0].created, 1700000000);
        assert_eq!(sessions[0].activity, 1700000100);
    }

    #[test]
    fn parse_filters_out_non_mux_sessions() {
        let output = "other-session\t1700000000\t1700000100\nmux-mine\t1700001000\t1700001200\nwork\t1700002000\t1700002100\n";
        let sessions = parse_list_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "mux-mine");
    }

    #[test]
    fn parse_skips_malformed_rows_wrong_field_count() {
        // One field, three fields, five fields — all should be skipped
        let output = "mux-bad\nmux-also-bad\t1234\nmux-too-many\t1234\t5678\textra\nmux-good\t1700000000\t1700000100\n";
        let sessions = parse_list_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "mux-good");
    }

    #[test]
    fn parse_mixed_valid_and_invalid_rows() {
        let output = concat!(
            "mux-valid1\t1000\t2000\n",
            "not-mux\t3000\t4000\n",
            "malformed-row\n",
            "mux-valid2\t5000\t6000\r\n",
            "\n", // empty line
            "mux-valid3\t7000\t8000\n",
        );
        let sessions = parse_list_output(output);
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].name, "mux-valid1");
        assert_eq!(sessions[1].name, "mux-valid2");
        assert_eq!(sessions[2].name, "mux-valid3");
    }

    #[test]
    fn parse_single_mux_session_no_trailing_newline() {
        let output = "mux-solo\t9999\t8888";
        let sessions = parse_list_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "mux-solo");
        assert_eq!(sessions[0].created, 9999);
        assert_eq!(sessions[0].activity, 8888);
    }

    // ── Adapter construction ──────────────────────────────────────────────────

    #[test]
    fn default_adapter_uses_tmux_bin() {
        let adapter = TmuxAdapter::default();
        assert_eq!(adapter.tmux_bin, "tmux");
    }

    #[test]
    fn with_bin_sets_custom_binary() {
        let adapter = TmuxAdapter::with_bin("/usr/local/bin/tmux");
        assert_eq!(adapter.tmux_bin, "/usr/local/bin/tmux");
    }

    // ── Validation integration (async new_session / kill_session guards) ──────

    #[tokio::test]
    async fn new_session_rejects_bad_name() {
        let adapter = TmuxAdapter::default();
        let err = adapter
            .new_session("not-mux", "/tmp", None)
            .await
            .unwrap_err();
        assert!(matches!(err, TmuxError::InvalidSessionName(_)));
    }

    #[tokio::test]
    async fn new_session_rejects_relative_workdir() {
        let adapter = TmuxAdapter::default();
        let err = adapter
            .new_session("mux-test", "relative/path", None)
            .await
            .unwrap_err();
        assert!(matches!(err, TmuxError::InvalidWorkdir(_)));
    }

    #[tokio::test]
    async fn new_session_rejects_bad_status_string() {
        let adapter = TmuxAdapter::default();
        let err = adapter
            .new_session("mux-test", "/tmp", Some("bad $CHARS here"))
            .await
            .unwrap_err();
        assert!(matches!(err, TmuxError::InvalidStatusString(_)));
    }

    #[tokio::test]
    async fn kill_session_rejects_bad_name() {
        let adapter = TmuxAdapter::default();
        let err = adapter.kill_session("no-prefix").await.unwrap_err();
        assert!(matches!(err, TmuxError::InvalidSessionName(_)));
    }
}
