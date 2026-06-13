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

/// Happy path: required tools present (tmux ≥ 3.0, uname, sha256sum), arch
/// normalized (x86_64 → amd64), home captured, tmux version persisted,
/// transport persisted as streamlocal or tcp.
///
/// After the test succeeds: verify DB row has arch=amd64, home=/home/testuser,
/// tmux_version=<actual>, transport!=NULL.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_happy_path_persists_probe_results() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// `uname -m` returns `x86_64` → stored arch must be `amd64` (not `x86_64`).
///
/// Mirrors normalize_arch unit test at crates/mux-cli/src/host.rs:684.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_normalizes_x86_64_to_amd64() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// tmux version ≥ 3.0 present → host test exits 0, version string stored.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_tmux_version_stored_on_success() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// tmux missing from PATH → host test exits 1, error mentions tmux,
/// arch and home remain NULL in the DB.
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

/// sha256sum missing from PATH → host test exits 1, error identifies
/// the missing tool.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_fails_when_sha256sum_missing() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Home directory not readable (chmod 000 on home) → host test exits 1.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_fails_when_home_not_readable() {
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
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_force_transport_tcp_overrides_probe() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Host not in the DB (alias not added yet) → host test exits 1 with
/// a human-readable alias-not-found error before SSH.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_test_errors_when_alias_not_found() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

// ── mux host trust (TOFU) ─────────────────────────────────────────────────────

/// First contact with a new host: TOFU prompt shown; --yes auto-accepts,
/// fingerprint stored in known-hosts table.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_first_contact_yes_flag_accepts() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Subsequent connection to the same host: fingerprint matches → no prompt,
/// host test exits 0 without user interaction.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_known_fingerprint_skips_prompt() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Fingerprint mismatch (host key changed) → host test exits 1,
/// error references the mismatch and instructs the user to re-trust.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_fingerprint_mismatch_rejects() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Non-interactive mode (stdin not a tty) with unknown fingerprint → exit 1,
/// instructs user to run `mux host test <alias>` interactively.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn host_trust_non_interactive_unknown_fingerprint_rejects() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}
