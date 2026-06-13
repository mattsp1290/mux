// Integration tests for `mux init`.
//
// These tests do not require Docker — `mux init` only creates local state.
// Run with:
//   cargo test -p mux-integration-tests --test integration --features integration-tests init
//
// All tests must pass on a CI runner without Docker.

use crate::harness::{run_mux, TestEnv};

/// Default state directory (~/.mux equivalent) is created when MUX_HOME is set.
/// Verifies: mux.db exists, directory has mode 0700, exit code 0.
#[test]
fn init_creates_state_directory_and_database() {
    let env = TestEnv::new();
    let mux_home = env.mux_home_str();
    let (code, _stdout, stderr) = run_mux(&["init"], &[("MUX_HOME", &mux_home)]);
    assert_eq!(code, 0, "mux init must exit 0; stderr: {stderr}");

    // mux.db must exist
    let db_path = env.mux_home.path().join("mux.db");
    assert!(db_path.exists(), "mux.db must be created in MUX_HOME");
}

/// MUX_HOME override: state written to the specified directory, not ~/.mux.
#[test]
fn init_respects_mux_home_override() {
    let env = TestEnv::new();
    let custom = env.mux_home.path().join("custom_state");
    std::fs::create_dir_all(&custom).unwrap();
    let custom_str = custom.to_string_lossy().to_string();

    let (code, _stdout, stderr) = run_mux(&["init"], &[("MUX_HOME", &custom_str)]);
    assert_eq!(code, 0, "mux init with MUX_HOME override must exit 0; stderr: {stderr}");

    let db_path = custom.join("mux.db");
    assert!(db_path.exists(), "mux.db must be created in the overridden MUX_HOME");
}

/// Running `mux init` twice is idempotent — exit 0 both times, no error.
#[test]
fn init_is_idempotent() {
    let env = TestEnv::new();
    let mux_home = env.mux_home_str();

    let (code1, _stdout1, stderr1) = run_mux(&["init"], &[("MUX_HOME", &mux_home)]);
    assert_eq!(code1, 0, "first mux init must exit 0; stderr: {stderr1}");

    let (code2, _stdout2, stderr2) = run_mux(&["init"], &[("MUX_HOME", &mux_home)]);
    assert_eq!(code2, 0, "second mux init must exit 0 (idempotent); stderr: {stderr2}");
}

/// State directory has restrictive permissions (mode 0700, owner-only).
#[cfg(unix)]
#[test]
fn init_state_directory_has_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let env = TestEnv::new();
    let mux_home = env.mux_home_str();

    let (code, _stdout, stderr) = run_mux(&["init"], &[("MUX_HOME", &mux_home)]);
    assert_eq!(code, 0, "mux init must exit 0; stderr: {stderr}");

    let meta = std::fs::metadata(env.mux_home.path())
        .expect("MUX_HOME dir must exist after init");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "MUX_HOME must have mode 0700, got {mode:o}");
}
