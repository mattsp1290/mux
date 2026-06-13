// Integration tests for session lifecycle commands:
//   mux create, mux list, mux status, mux attach, mux kill
//
// These tests require:
//   - Docker (via TestHost::start)
//   - A deployed mux-agent binary (MUX_AGENT_BINARY env var)
//   - An SSH agent loaded with the test identity (docker/test-host/test_ed25519)
//
// Once mux-qz4 wires the integration crate, run with:
//   cargo test -p mux-integration-tests --test integration --features integration-tests -- --test-threads=1

use crate::harness::TestHost;

// ── mux create ───────────────────────────────────────────────────────────────

/// Happy path: mux create clones a repository, starts a tmux session,
/// and records the session in local state.
///
/// Acceptance: `mux list` shows the session with status=active after create.
#[test]
#[ignore = "requires Docker + mux-agent binary (mux-qz4 implementation)"]
fn create_happy_path_session_appears_in_list() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: mux init → mux host add → mux host test → mux agent deploy → mux create <public-repo> → assert mux list shows session")
}

/// `mux create` with SSH_AUTH_SOCK unset → exit 1, error contains "ssh_agent_not_forwarded",
/// hint contains "ssh-add".
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn create_without_ssh_agent_exits_with_ssh_agent_not_forwarded() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: unset SSH_AUTH_SOCK, run mux create, assert exit 1 + error code in stderr")
}

/// Working directory already exists on the remote host → exit 1, error contains
/// "workdir_pre_existing".
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn create_workdir_pre_existing_exits_with_error() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: SSH into container and pre-create the workdir, then mux create → assert exit 1 + workdir_pre_existing")
}

/// Git clone failure (non-existent repo URL) → exit 1, error surfaced with mux: prefix.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn create_git_clone_failure_exits_nonzero() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: mux create <invalid-url> → assert exit 1")
}

// ── mux list ─────────────────────────────────────────────────────────────────

/// `mux list` with no sessions → exit 0, empty output (or headers only).
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn list_no_sessions_exits_zero() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: mux init → mux host add → mux list → assert exit 0")
}

/// `mux list --plain` → tab-separated rows, no ANSI codes.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn list_plain_outputs_tab_separated_rows() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session → mux list --plain → assert no ANSI escapes in stdout")
}

/// Unreachable host → session row shows status=unreachable, not an error exit.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn list_unreachable_host_shows_unreachable_status() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session, stop container, mux list → assert unreachable in output, exit 0")
}

// ── mux status ───────────────────────────────────────────────────────────────

/// UUID lookup returns session details.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn status_uuid_lookup_returns_details() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session, get UUID from list, mux status <uuid> → assert exit 0 + output")
}

/// Shortname lookup returns session details.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn status_shortname_lookup_returns_details() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session, mux status <shortname> → assert exit 0 + output")
}

/// UUID lookup takes priority over shortname when both could match.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn status_uuid_takes_priority_over_shortname() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create two sessions, confirm UUID lookup doesn't fall back to shortname")
}

// ── mux attach ───────────────────────────────────────────────────────────────

/// Dead session rejected before SSH attempt.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn attach_dead_session_exits_with_error() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session, kill it, mux attach → assert exit 1 + 'session already dead' or similar")
}

// ── mux kill ─────────────────────────────────────────────────────────────────

/// Killing an active session marks it dead in local state.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn kill_active_session_marks_dead() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session → mux kill → mux status → assert status=dead")
}

/// Killing an already-dead session is idempotent (exit 0).
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn kill_already_dead_session_is_noop() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: kill session, kill again → assert second kill exits 0")
}

/// Killing a session owned by a different client UUID exits 1.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn kill_non_owned_session_exits_nonzero() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: create session with one MUX_HOME, try to kill from a different MUX_HOME → assert exit 1 + 'session not owned'")
}

// ── SSH agent forwarding failure ─────────────────────────────────────────────

/// `mux create` with no keys in ssh-agent (agent running but empty) → exit 1
/// with ssh_agent_not_forwarded hint.
///
/// Note: the error triggers when the git clone requires authentication and the
/// agent has no key. Use a private repo URL or a URL that requires auth.
#[test]
#[ignore = "requires Docker (mux-qz4 implementation)"]
fn create_ssh_agent_empty_no_keys_exits_with_agent_error() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: start ssh-agent with no keys, run mux create private-url → assert exit 1")
}

// ── streamlocal vs TCP transport ─────────────────────────────────────────────

/// MUX_FORCE_TRANSPORT=tcp forces TCP even when streamlocal is available.
#[test]
#[ignore = "requires Docker + running mux-agent (mux-qz4 implementation)"]
fn create_force_transport_tcp() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: MUX_FORCE_TRANSPORT=tcp mux create → assert session created, transport=tcp in DB")
}

/// MUX_FORCE_TRANSPORT=streamlocal forces streamlocal; if unavailable → exit 1.
#[test]
#[ignore = "requires Docker + running mux-agent (mux-qz4 implementation)"]
fn create_force_transport_streamlocal_when_unavailable_exits() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement: remove agent socket, MUX_FORCE_TRANSPORT=streamlocal mux create → assert exit 1")
}
