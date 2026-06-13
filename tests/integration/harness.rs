// Integration test harness.
//
// Provides TestHost: starts a Docker container, exposes SSH, and runs mux commands.
// Each test must create its own TestHost and TempDir for MUX_HOME.
//
// See prompts/docs/integration-tests.md for the full design.
//
// NOTE: Tests that use TestHost share a fixed-port container service and must not
// run concurrently. Use `--test-threads=1` or `#[serial_test::serial]`.

use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Path to the test identity private key (relative to workspace root).
const TEST_KEY_WORKSPACE_REL: &str = "docker/test-host/test_ed25519";

/// Returns the workspace root by walking up from CARGO_MANIFEST_DIR until
/// a directory containing a Cargo.toml with `[workspace]` is found.
pub fn workspace_root() -> PathBuf {
    let start = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap());
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists() {
            let contents = std::fs::read_to_string(&candidate).unwrap_or_default();
            if contents.contains("[workspace]") {
                return dir.to_owned();
            }
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => panic!("workspace root not found from {start:?}"),
        }
    }
}

/// Returns true if Docker is available on the host.
pub fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A running test host container.
///
/// On construction, ensures the Docker service is running and SSH is ready.
/// On drop, stops and removes the container.
///
/// The committed test identity key (0644 in git) is copied to a TempDir and
/// chmod'd 0600 before use so ssh-add accepts it.
pub struct TestHost {
    pub alias: String,
    pub addr: String,
    pub port: u16,
    pub user: String,
    /// Key at 0600 permissions, safe for ssh-add.
    pub key_path: PathBuf,
    /// TempDir holding the 0600 key copy; kept alive for the TestHost lifetime.
    _key_tmp: tempfile::TempDir,
    compose_file: String,
    service: String,
}

impl TestHost {
    /// Start the named docker-compose service and return a handle.
    ///
    /// Panics if Docker is unavailable. Blocks until SSH is ready (up to 30s).
    pub fn start(service: &str) -> Self {
        if !docker_available() {
            panic!("Docker unavailable — integration tests require Docker");
        }

        let root = workspace_root();
        let compose_file = root
            .join("docker/test-host/docker-compose.yml")
            .to_string_lossy()
            .to_string();

        let status = Command::new("docker")
            .args(["compose", "-f", &compose_file, "up", "-d", service])
            .status()
            .expect("docker compose up failed");
        assert!(status.success(), "docker compose up returned non-zero");

        let port: u16 = match service {
            "mux-test-host-a" => 2221,
            "mux-test-host-b" => 2222,
            _ => panic!("unknown test service: {service}"),
        };

        // Wait for SSH to be ready (up to 30s).
        Self::wait_for_ssh("127.0.0.1", port, Duration::from_secs(30));

        // Copy the committed key (0644) to a tempdir and chmod 0600.
        // SSH rejects keys with permissions wider than 0600.
        let key_src = root.join(TEST_KEY_WORKSPACE_REL);
        let key_tmp = tempfile::TempDir::new().expect("TempDir for key");
        let key_dest = key_tmp.path().join("test_ed25519");
        std::fs::copy(&key_src, &key_dest)
            .unwrap_or_else(|e| panic!("failed to copy test key from {key_src:?}: {e}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_dest, std::fs::Permissions::from_mode(0o600))
                .expect("chmod 0600 on test key");
        }

        TestHost {
            alias: service.to_string(),
            addr: "127.0.0.1".to_string(),
            port,
            user: "testuser".to_string(),
            key_path: key_dest,
            _key_tmp: key_tmp,
            compose_file,
            service: service.to_string(),
        }
    }

    /// Block until SSH accepts a TCP connection on host:port, or panic after deadline.
    fn wait_for_ssh(host: &str, port: u16, deadline: Duration) {
        let addr = format!("{host}:{port}");
        let start = Instant::now();
        loop {
            if TcpStream::connect(&addr).is_ok() {
                return;
            }
            if start.elapsed() >= deadline {
                panic!("SSH not ready on {addr} after {deadline:?}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// SSH user@addr string.
    pub fn user_at_addr(&self) -> String {
        format!("{}@{}", self.user, self.addr)
    }

    /// Run a `mux` CLI command with the given args.
    ///
    /// Uses `CARGO_BIN_EXE_mux` (set by Cargo for integration test binaries) or
    /// the `MUX_BIN` env override for CI pipelines that supply a pre-built binary.
    pub fn mux(&self, args: &[&str]) -> (i32, String, String) {
        let mux_bin = std::env::var("MUX_BIN")
            .unwrap_or_else(|_| option_env!("CARGO_BIN_EXE_mux").unwrap_or("mux").to_string());
        let output = Command::new(&mux_bin)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {mux_bin}: {e}"));
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        (code, stdout, stderr)
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["compose", "-f", &self.compose_file, "stop", &self.service])
            .status();
        let _ = Command::new("docker")
            .args(["compose", "-f", &self.compose_file, "rm", "-f", &self.service])
            .status();
    }
}

/// Macro to return early from a test if Docker is unavailable.
///
/// Note: early return reports as PASSED in cargo output, not skipped.
/// The primary skip mechanism is the `integration-tests` feature gate —
/// don't compile the integration crate at all on runners without Docker.
#[macro_export]
macro_rules! require_docker {
    () => {
        if !$crate::harness::docker_available() {
            eprintln!("SKIP (Docker unavailable) — test reported as PASSED");
            return;
        }
    };
}
