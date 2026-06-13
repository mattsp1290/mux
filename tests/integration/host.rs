// Integration tests for mux host test (and supporting host lifecycle operations).
//
// These are stubs — implementations require the Docker test infrastructure from
// mux-3bv and will be written in mux-qz4. The function signatures and scenario
// comments document the acceptance criteria from mux-av5.
//
// Once mux-qz4 wires the integration crate, run with:
//   cargo test -p mux-integration-tests --test integration --features integration-tests -- --test-threads=1
// (test-threads=1 required: tests share fixed-port Docker containers)
//
// Skip semantics: the `integration-tests` feature gate is the intended CI skip mechanism.
// `require_docker!()` is a local-dev convenience; under `-- --ignored` on Docker-less runners
// it early-returns and reports PASSED, not skipped.

use crate::harness::TestHost;

// ── mux host test ─────────────────────────────────────────────────────────────

/// Happy path: required tools present (tmux ≥ 3.0, uname), arch normalized
/// (x86_64 → amd64), home captured, transport persisted as streamlocal or tcp.
///
/// Spec (docs/01): runs sentinels uname -m, $HOME, tmux -V; persists arch,
/// home, transport_mode, fingerprint, tmux_version.
///
/// After the test succeeds: verify DB row has arch=amd64, home=/home/testuser,
/// transport!=NULL. Note: tmux_version persistence has a TODO in
/// crates/mux-cli/src/host.rs:332-335 (deferred schema migration); omit the
/// DB tmux_version assertion until that column is added.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_happy_path_persists_probe_results() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// `uname -m` returns `x86_64` → stored arch must be `amd64` (not `x86_64`).
///
/// Mirrors normalize_arch unit test at crates/mux-cli/src/host.rs (normalize_arch fn).
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_normalizes_x86_64_to_amd64() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// tmux ≥ 3.0 required — version string is printed in the success message and
/// is parsed for the ≥ 3.0 check.
///
/// Note: tmux_version is printed by `cmd_test_core` but NOT yet persisted to the
/// DB (see crates/mux-cli/src/host.rs:332-335 TODO). Assert the success message
/// contains the version string, not a DB column value.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_tmux_version_at_least_3_0_required() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// tmux missing from PATH → host test exits 1, error mentions tmux,
/// arch and home remain NULL in the DB (partial-failure atomicity).
///
/// Implementation: `(tmux -V 2>&1 || echo 'tmux-not-found')` causes
/// parse_tmux_version to fail → preflight returns Err → no update_probe call.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_fails_when_tmux_missing() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// tmux present but version < 3.0 → host test exits 1, error mentions
/// minimum version requirement.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_fails_when_tmux_version_too_old() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Preflight output missing the MUX_SENTINEL_V1 marker (e.g., MOTD noise
/// consumed the output) → host test exits 1, error cites missing sentinel.
///
/// Simulate by running `mux host test` against a container whose MOTD
/// replaces or truncates the sentinel lines. Exercises parse_preflight_output
/// error path at crates/mux-cli/src/host.rs (missing start sentinel).
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_fails_when_preflight_sentinel_missing() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Agent socket absent → transport probe defaults to tcp (not an error).
///
/// Implementation: `test -S $HOME/.mux/agent.sock` returns exit 1 (no socket)
/// → transport="tcp" persisted. Agent has not been deployed on this container.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_defaults_to_tcp_when_no_agent_socket() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Transport is persisted after a successful host test: streamlocal if
/// the agent sock path is reachable, tcp otherwise.
///
/// After host test: assert `hosts.transport` is `streamlocal` or `tcp`.
/// This exercises the transport probe path that follows the preflight.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_persists_transport_after_probe() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// MUX_FORCE_TRANSPORT=tcp overrides the probed transport and persists tcp
/// even when streamlocal would be preferred.
///
/// Note: MUX_FORCE_TRANSPORT is not yet wired into cmd_test_core
/// (crates/mux-cli/src/host.rs). This stub documents the spec requirement;
/// implement once the env var is read in the host test flow.
#[test]
#[ignore = "MUX_FORCE_TRANSPORT not yet wired into cmd_test_core (mux-qz4)"]
fn host_test_force_transport_tcp_overrides_probe() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement when MUX_FORCE_TRANSPORT is wired into host test")
}

/// Host not in the DB (alias not added yet) → host test exits 1 with
/// a human-readable alias-not-found error before SSH.
///
/// This test does NOT need a running Docker container — the failure happens
/// in the local DB lookup before any SSH connection is attempted.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_errors_when_alias_not_found() {
    crate::require_docker!();
    // No TestHost::start() here — failure is local (DB lookup before SSH)
    todo!("implement in mux-qz4")
}

// ── mux host trust (TOFU) ─────────────────────────────────────────────────────

/// First contact with a new host: TOFU prompt shown; --yes auto-accepts,
/// fingerprint stored in known-hosts table.
///
/// Note: stdin plumbing requires TestHost::mux() to support --yes flag injection;
/// verify the harness can pass CLI flags to the subprocess.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_first_contact_yes_flag_accepts() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Subsequent connection to the same host: fingerprint matches → no prompt,
/// host test exits 0 without user interaction.
///
/// Setup: run host test once (stores fingerprint), then run again.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_known_fingerprint_skips_prompt() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Fingerprint mismatch (host key changed) → host test exits 1,
/// error references the mismatch and instructs the user to re-trust.
///
/// Use `mux-test-host-b` for key-change simulation: seed a known-hosts DB
/// entry with `mux-test-host-a`'s key, then connect to `mux-test-host-b`
/// which presents a different key.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_fingerprint_mismatch_rejects() {
    crate::require_docker!();
    let _host_a = TestHost::start("mux-test-host-a");
    let _host_b = TestHost::start("mux-test-host-b");
    todo!("implement in mux-qz4")
}

/// Non-interactive mode (stdin not a tty) with unknown fingerprint → exit 1,
/// instructs user to run `mux host test <alias>` interactively.
///
/// `TestHost::mux()` runs as a subprocess (stdin is not a tty by default),
/// so this scenario is automatically exercised when the host's fingerprint is
/// not yet stored in the known-hosts table.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_non_interactive_unknown_fingerprint_rejects() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}
