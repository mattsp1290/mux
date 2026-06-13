// Integration tests for mux agent deploy / logs / stop.
//
// These are stubs — implementations require the Docker test infrastructure from
// mux-3bv and will be written in mux-qz4. The function signatures and scenario
// comments document the acceptance criteria from mux-zpx.
//
// Run with: cargo test -p mux-integration-tests --test integration --features integration-tests -- --test-threads=1
// (test-threads=1 required: tests share fixed-port Docker containers)
//
// All tests call require_docker!() at entry to skip gracefully when Docker is unavailable.

use crate::harness::TestHost;

// ── mux agent deploy ─────────────────────────────────────────────────────────

/// Successful deploy: binary uploaded, size+hash verified, chmod applied,
/// version persisted to agent_versions.
///
/// Uses MUX_AGENT_BINARY pointing to the pre-built mux-agent-amd64 binary.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_happy_path_uploads_and_persists_version() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Deploy before mux host test has run (arch/home NULL) → exit 1 with
/// human-readable error mentioning the missing precondition.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_errors_without_host_test() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// MUX_AGENT_BINARY set to a non-existent path → exit 1, error contains
/// "MUX_AGENT_BINARY".
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_mux_agent_binary_nonexistent_errors() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Simulate truncated upload → remote size mismatch → deploy exits 1,
/// version is NOT written to agent_versions.
///
/// Requires a way to inject a partial upload (e.g., a stub binary that writes
/// fewer bytes to the remote).
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_size_mismatch_does_not_persist_version() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Remote hash mismatch → deploy exits 1, version NOT persisted.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_hash_mismatch_does_not_persist_version() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// chmod fails on remote (remove write permission from target dir) →
/// deploy exits 1, version NOT persisted.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_chmod_failure_does_not_persist_version() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Agent already running when deploy is called → graceful SIGTERM stop
/// (or RPC Shutdown when wired), then redeploy succeeds.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn deploy_stops_running_agent_before_upload() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

// ── mux agent logs ───────────────────────────────────────────────────────────

/// Log file exists and has content → output returned, exit 0.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn logs_returns_tail_of_log_file() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Log file does not exist (agent never started) → empty output, exit 0.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn logs_no_file_returns_empty() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// `mux agent logs --follow` → exit 1 with "not yet supported" error
/// (streaming not implemented in v0.1).
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn logs_follow_returns_not_supported_error() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

// ── mux agent stop ───────────────────────────────────────────────────────────

/// No agent running (no lock file) → exit 0, "no agent running" message.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn stop_no_agent_running_is_noop() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Agent running, SIGTERM sufficient → process exits cleanly.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn stop_sigterm_is_sufficient() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// SIGTERM insufficient → SIGKILL fallback kills the process.
///
/// Requires a stub agent process that ignores SIGTERM.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn stop_sigkill_fallback_kills_process() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}

/// Dead process (stale lock file) → exit 0, idempotent.
#[test]
#[ignore = "requires Docker (mux-qz4)"]
fn stop_dead_process_is_noop() {
    crate::require_docker!();
    let _host = TestHost::start("mux-test-host-a");
    todo!("implement in mux-qz4")
}
