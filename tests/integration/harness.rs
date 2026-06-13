// Integration test harness.
//
// Provides TestHost: starts a Docker container, exposes SSH, and runs mux commands.
// Each test must create its own TestHost and TempDir for MUX_HOME.
//
// See prompts/docs/integration-tests.md for the full design.

use std::path::PathBuf;
use std::process::Command;

/// Path to the test identity private key (relative to workspace root).
pub const TEST_KEY: &str = "docker/test-host/test_ed25519";

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
/// On construction, ensures the Docker service is running.
/// On drop, stops and removes the container.
pub struct TestHost {
    pub alias: String,
    pub addr: String,
    pub port: u16,
    pub user: String,
    pub key_path: PathBuf,
    service: String,
}

impl TestHost {
    /// Start the named docker-compose service and return a handle.
    ///
    /// Panics if Docker is unavailable.
    pub fn start(service: &str) -> Self {
        if !docker_available() {
            panic!("Docker unavailable — integration tests require Docker");
        }

        let compose_file = "docker/test-host/docker-compose.yml";
        let status = Command::new("docker")
            .args(["compose", "-f", compose_file, "up", "-d", "--build", service])
            .status()
            .expect("docker compose up failed");
        assert!(status.success(), "docker compose up returned non-zero");

        let port = match service {
            "mux-test-host-a" => 2221,
            "mux-test-host-b" => 2222,
            _ => panic!("unknown test service: {service}"),
        };

        let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let key_path = workspace_root.join(TEST_KEY);

        TestHost {
            alias: service.to_string(),
            addr: "127.0.0.1".to_string(),
            port,
            user: "testuser".to_string(),
            key_path,
            service: service.to_string(),
        }
    }

    /// SSH user@addr string.
    pub fn user_at_addr(&self) -> String {
        format!("{}@{}", self.user, self.addr)
    }

    /// Run a `mux` CLI command with the given args.
    ///
    /// The MUX_HOME and SSH_AUTH_SOCK must be set by the caller via env.
    pub fn mux(&self, args: &[&str]) -> (i32, String, String) {
        let mux_bin = std::env::var("MUX_BIN").unwrap_or_else(|_| "mux".to_string());
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
        let compose_file = "docker/test-host/docker-compose.yml";
        let _ = Command::new("docker")
            .args(["compose", "-f", compose_file, "stop", &self.service])
            .status();
        let _ = Command::new("docker")
            .args(["compose", "-f", compose_file, "rm", "-f", &self.service])
            .status();
    }
}

/// Macro to skip a test if Docker is unavailable.
///
/// Usage: `require_docker!();` at the top of each integration test.
#[macro_export]
macro_rules! require_docker {
    () => {
        if !$crate::harness::docker_available() {
            eprintln!("SKIP: Docker unavailable");
            return;
        }
    };
}
