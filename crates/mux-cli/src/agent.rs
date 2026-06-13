//! `mux agent deploy` implementation.
//!
//! Spec: docs/01 §mux agent deploy

use anyhow::{bail, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use mux_core::error::MuxError;
use mux_state::{agent_version_repo, model::Host};

use crate::agent_start::RemoteExec;

// ── DeployHost trait ──────────────────────────────────────────────────────────

/// SSH capabilities required by agent deploy: command execution + binary upload.
pub trait DeployHost: RemoteExec {
    /// Upload `content` bytes to `remote_path` on the remote host.
    fn upload(&self, content: &[u8], remote_path: &str) -> Result<(), MuxError>;
}

// ── Context ───────────────────────────────────────────────────────────────────

pub struct DeployContext<'a, S: DeployHost> {
    pub conn: &'a Connection,
    pub host: &'a Host,
    pub ssh: S,
}

// ── Public helpers ────────────────────────────────────────────────────────────

/// Select the local agent binary for `arch`.
///
/// Priority:
/// 1. `MUX_AGENT_BINARY` env var (any path)
/// 2. Adjacent to the current executable as `mux-agent-{arch}`
///
/// Returns `(binary_path, version_string)`.
pub fn select_agent_binary(arch: &str) -> Result<(std::path::PathBuf, String)> {
    if let Ok(path_str) = std::env::var("MUX_AGENT_BINARY") {
        if !path_str.is_empty() {
            let path = std::path::PathBuf::from(&path_str);
            if !path.exists() {
                bail!("MUX_AGENT_BINARY path does not exist: {:?}", path);
            }
            let version = version_for_arch(&path, arch);
            return Ok((path, version));
        }
    }

    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot locate mux executable: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("mux executable has no parent directory"))?;
    let binary_name = format!("mux-agent-{arch}");
    let path = dir.join(&binary_name);

    if !path.exists() {
        bail!(
            "no agent binary found adjacent to mux (expected '{binary_name}'); \
             set MUX_AGENT_BINARY to the path of the mux-agent binary"
        );
    }

    let version = version_for_arch(&path, arch);
    Ok((path, version))
}

/// Detect version of the agent binary, but only attempt local execution when
/// the target `arch` matches the local build target. Cross-arch binaries
/// cannot be executed locally (e.g. deploying aarch64 from x86_64 gives
/// `Exec format error`); the fallback is the mux-cli package version.
fn version_for_arch(binary_path: &std::path::Path, arch: &str) -> String {
    if is_local_arch(arch) {
        detect_version(binary_path)
    } else {
        env!("CARGO_PKG_VERSION").to_owned()
    }
}

/// Run `binary --version` and parse "mux-agent X.Y.Z".
/// Falls back to `env!("CARGO_PKG_VERSION")` if the binary cannot be executed
/// or produces unexpected output (e.g. in test fixtures).
pub fn detect_version(binary_path: &std::path::Path) -> String {
    if let Ok(output) = std::process::Command::new(binary_path)
        .arg("--version")
        .output()
    {
        let out = String::from_utf8_lossy(&output.stdout);
        if let Some(v) = parse_version_output(&out) {
            return v;
        }
        // Try stderr too (some CLIs write version there)
        let err = String::from_utf8_lossy(&output.stderr);
        if let Some(v) = parse_version_output(&err) {
            return v;
        }
    }
    // Fallback: use the mux-cli package version as a proxy
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Parse the version string from `mux-agent X.Y.Z` output.
pub fn parse_version_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if let Some(ver) = line.strip_prefix("mux-agent ") {
            let ver = ver.trim();
            if !ver.is_empty() {
                return Some(ver.to_owned());
            }
        }
    }
    None
}

/// Parse the `pid` field from the agent lock JSON `{"pid":N,...}`.
pub fn parse_lock_pid(json: &str) -> Option<u64> {
    let key = "\"pid\":";
    let start = json.find(key)? + key.len();
    let rest = json[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Parse the `tcp_url` field from the agent lock JSON `{"pid":N,"tcp_url":"tcp://..."}`.
///
/// Returns `None` for a missing key, a closing-quote parse failure, or an empty value.
/// The returned string is the raw substring between quotes with no JSON-escape decoding;
/// the canonical single-line lock format never contains backslash sequences.
pub fn parse_lock_tcp_url(json: &str) -> Option<String> {
    let key = "\"tcp_url\":\"";
    let start = json.find(key)? + key.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    let url = &rest[..end];
    if url.is_empty() {
        return None;
    }
    Some(url.to_owned())
}

/// Parse sha256 hex digest from either `sha256sum` or `openssl dgst -sha256` output.
///
/// - `sha256sum`:          `<64-hex>  <filename>\n`
/// - `openssl dgst -sha256`: `SHA256(<filename>)= <64-hex>\n`
///
/// Anchored to the known output formats: does NOT accept any arbitrary 64-hex token.
pub fn parse_sha256_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        // openssl: line starts with "SHA256(" — parse hash after last "= "
        if line.starts_with("SHA256(") {
            if let Some(h) = line.rsplit("= ").next() {
                if is_sha256_hex(h) {
                    return Some(h.to_ascii_lowercase());
                }
            }
            continue;
        }
        // sha256sum: first field is the hash, second field is the filename
        let mut fields = line.split_whitespace();
        if let (Some(h), Some(_file)) = (fields.next(), fields.next()) {
            if is_sha256_hex(h) {
                return Some(h.to_ascii_lowercase());
            }
        }
    }
    None
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Compute the sha256 hex digest of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ── Core deploy logic ─────────────────────────────────────────────────────────

/// Run `mux agent deploy` for the given host.
///
/// Steps (all must succeed; version is persisted only after upload is verified):
/// 1. Preconditions — host must have arch and home (from `mux host test`).
/// 2. Select binary (`MUX_AGENT_BINARY` env or adjacent `mux-agent-{arch}`).
/// 3. Read binary content; compute local size and sha256.
/// 4. Graceful agent stop: SIGTERM → SIGKILL fallback.
/// 5. Create remote `<home>/.mux/bin` directory.
/// 6. Upload binary to `<home>/.mux/bin/mux-agent`.
/// 7. Verify remote size and sha256 match local.
/// 8. `chmod +x` the remote binary.
/// 9. Persist version to `agent_versions` (only here, after verification).
pub fn run_agent_deploy<S: DeployHost>(ctx: DeployContext<'_, S>) -> Result<()> {
    let DeployContext { conn, host, ref ssh } = ctx;

    // ── 1. Preconditions ─────────────────────────────────────────────────────
    let arch = host.arch.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "mux: host '{}' has no arch; run 'mux host test {}' first",
            host.alias,
            host.alias
        )
    })?;
    let home = host.home.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "mux: host '{}' has no home; run 'mux host test {}' first",
            host.alias,
            host.alias
        )
    })?;

    // ── 2. Select binary ──────────────────────────────────────────────────────
    let (binary_path, version) = select_agent_binary(arch)?;

    // ── 3. Read binary; compute size and sha256 ───────────────────────────────
    let content = std::fs::read(&binary_path)
        .map_err(|e| anyhow::anyhow!("failed to read agent binary {:?}: {e}", binary_path))?;
    let local_size = content.len();
    let local_sha256 = sha256_hex(&content);

    let remote_dir = format!("{home}/.mux/bin");
    let remote_path = format!("{remote_dir}/mux-agent");
    let lock_path = format!("{home}/.mux/agent.lock");

    // ── 4. Graceful stop ──────────────────────────────────────────────────────
    stop_agent_if_running(ssh, &lock_path)?;

    // ── 5. Create remote directory ────────────────────────────────────────────
    let (code, _, stderr) = ssh
        .run(&format!("mkdir -p {}", sh_quote(&remote_dir)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if code != 0 {
        bail!(
            "mux: failed to create remote directory {remote_dir}: {}",
            stderr.trim()
        );
    }

    // ── 6. Upload ─────────────────────────────────────────────────────────────
    ssh.upload(&content, &remote_path)
        .map_err(|e| anyhow::anyhow!("mux: upload failed: {e}"))?;

    // ── 7. Verify size + sha256 ───────────────────────────────────────────────
    verify_upload(ssh, &remote_path, local_size, &local_sha256)?;

    // ── 8. chmod +x ───────────────────────────────────────────────────────────
    let (code, _, stderr) = ssh
        .run(&format!("chmod +x {}", sh_quote(&remote_path)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if code != 0 {
        bail!(
            "mux: chmod +x failed for {remote_path}: {}",
            stderr.trim()
        );
    }

    // ── 9. Persist version (only after verified upload) ───────────────────────
    // unwrap_or_default() yields 0 (1970) if the system clock is before the epoch;
    // acceptable for a local tool — a nonsense deployed_at is visible in logs.
    let deployed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    agent_version_repo::upsert(conn, host.id, &version, deployed_at)?;

    println!(
        "Deployed mux-agent {version} to '{}' ({remote_path}).",
        host.alias
    );
    Ok(())
}

/// Run `mux agent logs` for the given host.
///
/// Reads the last 200 lines from `<home>/.mux/agent.log` over SSH.
/// A missing log file is not an error — returns an empty string.
///
/// `follow = true` is rejected until a streaming SSH executor is available; passing
/// `tail -f` through a buffered `RemoteExec::run` would block forever and never return.
pub fn run_agent_logs<S: RemoteExec>(home: &str, follow: bool, ssh: &S) -> Result<String> {
    if follow {
        bail!("mux agent logs --follow: streaming not yet supported (requires SSH streaming transport)");
    }
    let log_path = format!("{home}/.mux/agent.log");
    let qp = sh_quote(&log_path);
    // `test -f` gates `tail` on file existence: exit 1 with no stderr means the log
    // hasn't been created yet (agent never started). A real error — permission denied,
    // transport failure — causes `tail` to write to stderr, distinguishing the cases.
    let cmd = format!("test -f {qp} && tail -n 200 {qp}");

    let (code, stdout, stderr) = ssh.run(&cmd).map_err(|e| anyhow::anyhow!("{e}"))?;

    if code != 0 {
        let err_msg = stderr.trim();
        if !err_msg.is_empty() {
            bail!("mux agent logs: {err_msg}");
        }
        // test -f exited non-zero with no stderr → log file not yet created.
        return Ok(String::new());
    }
    Ok(stdout)
}

/// Run `mux agent stop` for the given host.
///
/// 1. Read lock file for PID.
/// 2. If no lock or process dead → success (idempotent).
/// 3. SIGTERM → poll up to ~3 s → SIGKILL fallback.
/// 4. Final `kill -0` confirms the process is gone; surfaces an error if not.
///
/// # Note: RPC Shutdown not yet wired
/// The lock file may contain a `tcp_url`, but the agent binds its RPC port to the
/// *remote* host's loopback interface. Without an SSH port-forward we cannot reach it
/// from the controller. RPC-based graceful shutdown will be added in a future iteration
/// when SSH tunneling is available; for now SIGTERM→SIGKILL is the sole stop path.
pub fn run_agent_stop<S: RemoteExec>(home: &str, ssh: &S) -> Result<()> {
    let lock_path = format!("{home}/.mux/agent.lock");

    // 1. Read lock file (best-effort).
    let (code, stdout, _) = ssh
        .run(&format!("cat {} 2>/dev/null", sh_quote(&lock_path)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if code != 0 || stdout.trim().is_empty() {
        println!("mux agent stop: no agent running");
        return Ok(());
    }

    // Bound PID to kernel max; reject bogus values from a hostile lock file.
    let pid = match parse_lock_pid(&stdout) {
        Some(p) if (1..=MAX_PID).contains(&p) => p,
        _ => {
            println!("mux agent stop: no agent running");
            return Ok(());
        }
    };

    // 2. Check if process is alive.
    let (alive_code, _, _) = ssh
        .run(&format!("kill -0 {pid} 2>/dev/null"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if alive_code != 0 {
        println!("mux agent stop: no agent running");
        return Ok(());
    }

    // 3. SIGTERM → poll → SIGKILL.
    let _ = ssh.run(&format!("kill -TERM {pid} 2>/dev/null"));
    let (alive_after, _, _) = ssh
        .run(&format!(
            "for _i in 1 2 3; do kill -0 {pid} 2>/dev/null || exit 0; sleep 1; done; kill -0 {pid} 2>/dev/null"
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if alive_after == 0 {
        let _ = ssh.run(&format!("kill -KILL {pid} 2>/dev/null"));
        // 4. Verify the process is actually dead — SIGKILL can fail (e.g. zombie owned by
        //    init, kernel-uninterruptible state). Surface an honest error rather than
        //    printing "agent stopped" when the process is still alive.
        let (still_alive, _, _) = ssh
            .run(&format!("kill -0 {pid} 2>/dev/null"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if still_alive == 0 {
            bail!("mux agent stop: agent (pid {pid}) may still be running after SIGKILL");
        }
    }

    println!("mux agent stop: agent stopped");
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Sanity ceiling for PID values parsed from remote lock files.
/// Matches the default Linux `kernel.pid_max`; rejects obviously bogus values
/// from hostile or corrupted lock files without hard-coding a kernel dependency.
const MAX_PID: u64 = 4_194_304;

/// POSIX single-quote escaping: wraps `s` in `'...'` and escapes embedded `'`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// True when `arch` (the remote host's arch, e.g. "amd64") matches the local
/// build target — safe to execute a binary of that arch locally.
fn is_local_arch(arch: &str) -> bool {
    let local = std::env::consts::ARCH;
    arch == local
        || (arch == "amd64" && local == "x86_64")
        || (arch == "arm64" && local == "aarch64")
}

/// Attempt graceful stop of the running agent, if any.
///
/// 1. Read lock file for PID.
/// 2. Verify process is alive (`kill -0`).
/// 3. Send SIGTERM; poll up to ~3 s.
/// 4. If still alive, send SIGKILL.
fn stop_agent_if_running<S: RemoteExec>(ssh: &S, lock_path: &str) -> Result<()> {
    // Read lock file (best-effort — missing lock means no agent).
    let (code, stdout, _) = ssh
        .run(&format!("cat {} 2>/dev/null", sh_quote(lock_path)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if code != 0 || stdout.trim().is_empty() {
        return Ok(());
    }

    // Bound the PID to the kernel max; reject obviously bogus values from a
    // hostile or corrupted remote lock file.
    let pid = match parse_lock_pid(&stdout) {
        Some(p) if (1..=MAX_PID).contains(&p) => p,
        _ => return Ok(()),
    };

    // Check if alive.
    let (alive_code, _, _) = ssh
        .run(&format!("kill -0 {pid} 2>/dev/null"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if alive_code != 0 {
        return Ok(());
    }

    // Graceful SIGTERM then poll.
    let _ = ssh.run(&format!("kill -TERM {pid} 2>/dev/null"));
    let (alive_after, _, _) = ssh
        .run(&format!(
            "for _i in 1 2 3; do kill -0 {pid} 2>/dev/null || exit 0; sleep 1; done; kill -0 {pid} 2>/dev/null"
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if alive_after == 0 {
        // Still alive after wait — SIGKILL fallback.
        let _ = ssh.run(&format!("kill -KILL {pid} 2>/dev/null"));
    }

    Ok(())
}

/// Verify that the remote file matches the expected size and sha256.
fn verify_upload<S: RemoteExec>(
    ssh: &S,
    remote_path: &str,
    expected_size: usize,
    expected_sha256: &str,
) -> Result<()> {
    // Size check.
    let (code, stdout, stderr) = ssh
        .run(&format!("wc -c < {}", sh_quote(remote_path)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if code != 0 {
        bail!(
            "mux: failed to get remote file size: {}",
            stderr.trim()
        );
    }
    let remote_size: usize = stdout.trim().parse().map_err(|_| {
        anyhow::anyhow!(
            "mux: unexpected wc -c output: {:?}",
            stdout.trim()
        )
    })?;
    if remote_size != expected_size {
        bail!(
            "mux: upload size mismatch: expected {expected_size} bytes, got {remote_size}"
        );
    }

    // Hash check — try sha256sum (Linux) then openssl (macOS/BSD).
    let qp = sh_quote(remote_path);
    let (code, stdout, _) = ssh
        .run(&format!(
            "sha256sum {qp} 2>/dev/null || openssl dgst -sha256 {qp}"
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if code != 0 {
        bail!("mux: sha256 verification command failed on remote host");
    }
    let remote_sha256 = parse_sha256_output(&stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "mux: could not parse sha256 output: {:?}",
            stdout.trim()
        )
    })?;
    if remote_sha256 != expected_sha256 {
        bail!(
            "mux: upload hash mismatch: expected {expected_sha256}, got {remote_sha256}"
        );
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mux_state::{host_repo, store::Store};
    use rusqlite::Connection;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use tempfile::TempDir;

    // ── MockDeployHost ────────────────────────────────────────────────────────

    struct MockDeployHost {
        /// Pre-programmed (exit_code, stdout, stderr) responses for `run`.
        responses: RefCell<VecDeque<(i32, String, String)>>,
        /// Commands actually executed — shared so tests can read after `run_agent_deploy`
        /// consumes `self`.
        commands: Rc<RefCell<Vec<String>>>,
        /// Content passed to `upload` — shared for the same reason.
        upload_calls: Rc<RefCell<Vec<(Vec<u8>, String)>>>,
        /// If Some, `upload` returns this error.
        upload_error: Option<String>,
    }

    impl MockDeployHost {
        fn new(responses: Vec<(i32, &str, &str)>) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(c, o, e)| (c, o.to_owned(), e.to_owned()))
                        .collect(),
                ),
                commands: Rc::new(RefCell::new(Vec::new())),
                upload_calls: Rc::new(RefCell::new(Vec::new())),
                upload_error: None,
            }
        }

        fn with_upload_error(mut self, msg: &str) -> Self {
            self.upload_error = Some(msg.to_owned());
            self
        }
    }

    impl RemoteExec for MockDeployHost {
        fn run(&self, cmd: &str) -> Result<(i32, String, String), MuxError> {
            self.commands.borrow_mut().push(cmd.to_owned());
            let mut q = self.responses.borrow_mut();
            Ok(q.pop_front().unwrap_or((
                0,
                String::new(),
                "mock: no more responses".to_owned(),
            )))
        }
    }

    impl DeployHost for MockDeployHost {
        fn upload(&self, content: &[u8], remote_path: &str) -> Result<(), MuxError> {
            if let Some(ref msg) = self.upload_error {
                return Err(MuxError::RpcError(msg.clone()));
            }
            self.upload_calls
                .borrow_mut()
                .push((content.to_vec(), remote_path.to_owned()));
            Ok(())
        }
    }

    // ── DB helpers ────────────────────────────────────────────────────────────

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("mux.db")).unwrap();
        (dir, store)
    }

    fn insert_host_with_probe(conn: &Connection, alias: &str) -> Host {
        let id =
            host_repo::insert(conn, alias, "user", "10.0.0.1", 22, 1_000_000).unwrap();
        host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user"), Some("tcp"))
            .unwrap();
        host_repo::get_by_id(conn, id).unwrap().unwrap()
    }

    fn insert_host_no_probe(conn: &Connection, alias: &str) -> Host {
        let id =
            host_repo::insert(conn, alias, "user", "10.0.0.1", 22, 1_000_000).unwrap();
        host_repo::get_by_id(conn, id).unwrap().unwrap()
    }

    // ── Env guard + mutex ─────────────────────────────────────────────────────

    /// Serializes all tests that mutate MUX_AGENT_BINARY.
    /// All callers of `setup_fake_binary()` and the `select_agent_binary_*` tests
    /// hold this lock for the duration of the test.
    static ENV_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        let m = ENV_MUTEX.get_or_init(|| std::sync::Mutex::new(()));
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// RAII guard that removes an env var on drop, ensuring tests that set
    /// `MUX_AGENT_BINARY` don't leak the value to unrelated tests.
    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    // ── Fake binary fixture ───────────────────────────────────────────────────

    /// Write a small fake binary file and set MUX_AGENT_BINARY to its path.
    /// Returns (MutexGuard, TempDir, content_bytes, sha256_hex, EnvGuard).
    /// All five values must be held for the duration of the test; the MutexGuard
    /// serializes this test against all other MUX_AGENT_BINARY-mutating tests.
    fn setup_fake_binary() -> (std::sync::MutexGuard<'static, ()>, TempDir, Vec<u8>, String, EnvGuard) {
        let lock = env_lock();
        let dir = TempDir::new().unwrap();
        let content: Vec<u8> = b"fake-mux-agent-binary-content-for-testing".to_vec();
        let path = dir.path().join("mux-agent-fake");
        std::fs::write(&path, &content).unwrap();
        std::env::set_var("MUX_AGENT_BINARY", path.to_str().unwrap());
        let hash = sha256_hex(&content);
        (lock, dir, content, hash, EnvGuard("MUX_AGENT_BINARY"))
    }

    // ── Happy path ────────────────────────────────────────────────────────────

    #[test]
    fn deploy_happy_path_persists_version() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, content, hash, _guard) = setup_fake_binary();
        let size = content.len().to_string();
        let sha_line = format!("{hash}  /home/user/.mux/bin/mux-agent");

        // SSH responses in order:
        //   1. cat lock file      → no lock file (exit 1)
        //   2. mkdir -p           → ok
        //   3. wc -c              → size
        //   4. sha256sum/openssl  → hash (sha256sum format)
        //   5. chmod +x           → ok
        let responses = vec![
            (1, "", ""),                // cat lock → no agent
            (0, "", ""),                // mkdir -p
            (0, size.as_str(), ""),     // wc -c
            (0, sha_line.as_str(), ""), // sha256sum
            (0, "", ""),                // chmod +x
        ];

        let ssh = MockDeployHost::new(responses);
        run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap();

        // Version must be persisted.
        let av = agent_version_repo::get_for_host(conn, host.id)
            .unwrap()
            .expect("version should be recorded");
        assert!(!av.version.is_empty());
    }

    #[test]
    fn deploy_upload_is_called_with_correct_path() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, content, hash, _guard) = setup_fake_binary();
        let size = content.len().to_string();
        let sha_line = format!("{hash}  /home/user/.mux/bin/mux-agent");

        let responses = vec![
            (1, "", ""),
            (0, "", ""),
            (0, size.as_str(), ""),
            (0, sha_line.as_str(), ""),
            (0, "", ""),
        ];
        let ssh = MockDeployHost::new(responses);
        let upload_calls = Rc::clone(&ssh.upload_calls);
        run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap();

        let calls = upload_calls.borrow();
        assert_eq!(calls.len(), 1, "upload should be called exactly once");
        assert_eq!(calls[0].1, "/home/user/.mux/bin/mux-agent");
        assert_eq!(calls[0].0, content);
    }

    // ── Precondition failures ────────────────────────────────────────────────

    #[test]
    fn deploy_errors_without_arch() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_no_probe(conn, "noarch");

        let ssh = MockDeployHost::new(vec![]);
        let err = run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap_err();
        assert!(
            err.to_string().contains("no arch") || err.to_string().contains("host test"),
            "expected precondition error, got: {err}"
        );
        // Nothing persisted.
        let av = agent_version_repo::get_for_host(conn, host.id).unwrap();
        assert!(av.is_none());
    }

    #[test]
    fn deploy_errors_without_home() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let id = host_repo::insert(conn, "nohome", "user", "10.0.0.1", 22, 1_000_000).unwrap();
        host_repo::update_probe(conn, id, Some("amd64"), None, None).unwrap();
        let host = host_repo::get_by_id(conn, id).unwrap().unwrap();

        let ssh = MockDeployHost::new(vec![]);
        let err = run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap_err();
        assert!(
            err.to_string().contains("no home") || err.to_string().contains("host test"),
            "expected precondition error, got: {err}"
        );
    }

    // ── Verification failures must not persist version ─────────────────────────

    #[test]
    fn deploy_size_mismatch_does_not_persist_version() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, _content, _hash, _guard) = setup_fake_binary();

        // Remote reports wrong size.
        let responses = vec![
            (1, "", ""),  // cat lock
            (0, "", ""),  // mkdir -p
            (0, "9999", ""), // wc -c — wrong size
        ];
        let ssh = MockDeployHost::new(responses);
        let err = run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap_err();
        assert!(
            err.to_string().contains("size mismatch"),
            "expected size mismatch error, got: {err}"
        );
        // Version must NOT be persisted.
        let av = agent_version_repo::get_for_host(conn, host.id).unwrap();
        assert!(av.is_none(), "version must not be persisted on size mismatch");
    }

    #[test]
    fn deploy_hash_mismatch_does_not_persist_version() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, content, _hash, _guard) = setup_fake_binary();
        let size = content.len().to_string();

        // Remote reports wrong hash.
        let wrong_hash = "a".repeat(64);
        let wrong_sha_line = format!("{wrong_hash}  /home/user/.mux/bin/mux-agent");
        let responses = vec![
            (1, "", ""),                       // cat lock
            (0, "", ""),                       // mkdir -p
            (0, size.as_str(), ""),            // wc -c
            (0, wrong_sha_line.as_str(), ""), // sha256sum
        ];
        let ssh = MockDeployHost::new(responses);
        let err = run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap_err();
        assert!(
            err.to_string().contains("hash mismatch"),
            "expected hash mismatch error, got: {err}"
        );
        let av = agent_version_repo::get_for_host(conn, host.id).unwrap();
        assert!(av.is_none(), "version must not be persisted on hash mismatch");
    }

    #[test]
    fn deploy_upload_failure_does_not_persist_version() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, _content, _hash, _guard) = setup_fake_binary();

        let responses = vec![
            (1, "", ""),  // cat lock
            (0, "", ""),  // mkdir -p
        ];
        let ssh = MockDeployHost::new(responses).with_upload_error("connection reset");
        let err = run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap_err();
        assert!(
            err.to_string().contains("upload failed"),
            "expected upload error, got: {err}"
        );
        let av = agent_version_repo::get_for_host(conn, host.id).unwrap();
        assert!(av.is_none(), "version must not be persisted on upload failure");
    }

    #[test]
    fn deploy_chmod_failure_does_not_persist_version() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, content, hash, _guard) = setup_fake_binary();
        let size = content.len().to_string();
        let sha_line = format!("{hash}  /home/user/.mux/bin/mux-agent");

        let responses = vec![
            (1, "", ""),                // cat lock → no agent
            (0, "", ""),                // mkdir -p
            (0, size.as_str(), ""),     // wc -c
            (0, sha_line.as_str(), ""), // sha256sum
            (1, "", "permission denied"), // chmod +x → fails
        ];
        let ssh = MockDeployHost::new(responses);
        let err = run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap_err();
        assert!(
            err.to_string().contains("chmod +x failed"),
            "expected chmod error, got: {err}"
        );
        // Version must NOT be persisted — chmod failure is pre-persist.
        let av = agent_version_repo::get_for_host(conn, host.id).unwrap();
        assert!(av.is_none(), "version must not be persisted on chmod +x failure");
    }

    // ── Graceful stop ─────────────────────────────────────────────────────────

    #[test]
    fn deploy_stops_running_agent_before_upload() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, content, hash, _guard) = setup_fake_binary();
        let size = content.len().to_string();

        // Agent IS running (pid = 99999, SIGTERM is enough).
        let lock_json = r#"{"pid":99999,"tcp_url":"tcp://127.0.0.1:50000"}"#;
        let sha_line = format!("{hash}  /home/user/.mux/bin/mux-agent");
        let responses = vec![
            (0, lock_json, ""),       // cat lock → agent running
            (0, "", ""),              // kill -0 → alive
            (0, "", ""),              // kill -TERM
            (1, "", ""),              // poll loop → agent dead after TERM
            (0, "", ""),              // mkdir -p
            (0, size.as_str(), ""),   // wc -c
            (0, sha_line.as_str(), ""), // sha256sum
            (0, "", ""),              // chmod +x
        ];
        let ssh = MockDeployHost::new(responses);
        let commands = Rc::clone(&ssh.commands);

        run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap();

        let cmds = commands.borrow();
        // The stop commands must precede mkdir.
        let mkdir_idx = cmds.iter().position(|c| c.contains("mkdir")).expect("mkdir not found");
        let term_idx = cmds.iter().position(|c| c.contains("TERM")).expect("TERM not found");
        assert!(term_idx < mkdir_idx, "SIGTERM must be sent before mkdir");
    }

    #[test]
    fn deploy_no_lock_file_skips_stop() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_env_lock, _bin_dir, content, hash, _guard) = setup_fake_binary();
        let size = content.len().to_string();

        // No lock file — agent not running.
        let sha_line = format!("{hash}  /home/user/.mux/bin/mux-agent");
        let responses = vec![
            (1, "", ""),              // cat lock → not found
            (0, "", ""),              // mkdir -p
            (0, size.as_str(), ""),   // wc -c
            (0, sha_line.as_str(), ""),
            (0, "", ""),              // chmod +x
        ];
        let ssh = MockDeployHost::new(responses);
        let commands = Rc::clone(&ssh.commands);

        run_agent_deploy(DeployContext { conn, host: &host, ssh }).unwrap();

        let cmds = commands.borrow();
        let has_kill = cmds.iter().any(|c| c.contains("kill -0") || c.contains("TERM") || c.contains("KILL"));
        assert!(!has_kill, "no kill commands should be issued when no lock file: {cmds:?}");
    }

    // ── Unit tests for parsing helpers ────────────────────────────────────────

    #[test]
    fn parse_lock_pid_standard() {
        assert_eq!(
            parse_lock_pid(r#"{"pid":99999,"tcp_url":"tcp://127.0.0.1:8080"}"#),
            Some(99999)
        );
    }

    #[test]
    fn parse_lock_pid_missing_returns_none() {
        assert_eq!(parse_lock_pid("{}"), None);
        assert_eq!(parse_lock_pid("not json"), None);
    }

    #[test]
    fn parse_sha256_sha256sum_format() {
        let hash = "a".repeat(64);
        let line = format!("{hash}  /home/user/.mux/bin/mux-agent\n");
        assert_eq!(parse_sha256_output(&line), Some(hash));
    }

    #[test]
    fn parse_sha256_openssl_format() {
        let hash = "b".repeat(64);
        let line = format!("SHA256(/home/user/.mux/bin/mux-agent)= {hash}\n");
        assert_eq!(parse_sha256_output(&line), Some(hash));
    }

    #[test]
    fn parse_sha256_invalid_returns_none() {
        assert_eq!(parse_sha256_output("not a hash output"), None);
        assert_eq!(parse_sha256_output(""), None);
    }

    #[test]
    fn parse_version_output_standard() {
        assert_eq!(
            parse_version_output("mux-agent 0.1.0\n"),
            Some("0.1.0".to_owned())
        );
    }

    #[test]
    fn parse_version_output_ignores_noise() {
        let output = "some startup noise\nmux-agent 0.2.0\nmore noise\n";
        assert_eq!(
            parse_version_output(output),
            Some("0.2.0".to_owned())
        );
    }

    #[test]
    fn parse_version_output_unrecognized_returns_none() {
        assert_eq!(parse_version_output("unknown binary 1.0\n"), None);
    }

    #[test]
    fn sha256_hex_known_value() {
        // SHA256 of empty bytes is well-known.
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ── parse_lock_tcp_url ────────────────────────────────────────────────────

    #[test]
    fn parse_lock_tcp_url_standard() {
        let json = r#"{"pid":99999,"tcp_url":"tcp://127.0.0.1:50000"}"#;
        assert_eq!(
            parse_lock_tcp_url(json),
            Some("tcp://127.0.0.1:50000".to_owned())
        );
    }

    #[test]
    fn parse_lock_tcp_url_missing_returns_none() {
        assert_eq!(parse_lock_tcp_url(r#"{"pid":1}"#), None);
        assert_eq!(parse_lock_tcp_url("{}"), None);
    }

    // ── run_agent_logs ────────────────────────────────────────────────────────

    struct MockRemoteExec {
        responses: RefCell<VecDeque<(i32, String, String)>>,
        commands: Rc<RefCell<Vec<String>>>,
    }

    impl MockRemoteExec {
        fn new(responses: Vec<(i32, &str, &str)>) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(c, o, e)| (c, o.to_owned(), e.to_owned()))
                        .collect(),
                ),
                commands: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl RemoteExec for MockRemoteExec {
        fn run(&self, cmd: &str) -> Result<(i32, String, String), MuxError> {
            self.commands.borrow_mut().push(cmd.to_owned());
            let mut q = self.responses.borrow_mut();
            Ok(q.pop_front().unwrap_or((0, String::new(), "mock: no more responses".to_owned())))
        }
    }

    #[test]
    fn logs_returns_output_from_remote() {
        let log_content = "line 1\nline 2\nline 3\n";
        let ssh = MockRemoteExec::new(vec![(0, log_content, "")]);
        let output = run_agent_logs("/home/user", false, &ssh).unwrap();
        assert_eq!(output, log_content);
    }

    #[test]
    fn logs_no_file_returns_empty() {
        // test -f exits 1 with no stderr when the file doesn't exist yet.
        let ssh = MockRemoteExec::new(vec![(1, "", "")]);
        let output = run_agent_logs("/home/user", false, &ssh).unwrap();
        assert!(output.is_empty(), "missing log should yield empty string, got: {output:?}");
    }

    #[test]
    fn logs_permission_error_propagates() {
        // test -f succeeds, tail fails with permission denied in stderr → real error.
        let ssh = MockRemoteExec::new(vec![(1, "", "Permission denied")]);
        let err = run_agent_logs("/home/user", false, &ssh).unwrap_err();
        assert!(
            err.to_string().contains("Permission denied"),
            "permission error must propagate, got: {err}"
        );
    }

    #[test]
    fn logs_follow_returns_not_supported_error() {
        // --follow is gated until a streaming SSH executor exists.
        let ssh = MockRemoteExec::new(vec![]);
        let err = run_agent_logs("/home/user", true, &ssh).unwrap_err();
        assert!(
            err.to_string().contains("not yet supported"),
            "--follow must return a clear 'not yet supported' error, got: {err}"
        );
    }

    #[test]
    fn logs_uses_sh_quote_for_path() {
        let ssh = MockRemoteExec::new(vec![(0, "", "")]);
        let cmds = Rc::clone(&ssh.commands);
        run_agent_logs("/home/user", false, &ssh).unwrap();
        let issued = cmds.borrow();
        assert!(
            issued[0].contains("'/home/user/.mux/agent.log'"),
            "log path must be single-quoted, got: {:?}",
            issued[0]
        );
    }

    // ── run_agent_stop ────────────────────────────────────────────────────────

    #[test]
    fn stop_no_lock_is_noop() {
        // No lock file — cat exits 1, stdout empty.
        let ssh = MockRemoteExec::new(vec![(1, "", "")]);
        let cmds = Rc::clone(&ssh.commands);
        run_agent_stop("/home/user", &ssh).unwrap();
        let issued = cmds.borrow();
        // Only one command: cat the lock file.
        assert_eq!(issued.len(), 1, "only lock-read command should be issued: {issued:?}");
        assert!(issued[0].contains("agent.lock"), "expected lock-file read, got: {:?}", issued[0]);
    }

    #[test]
    fn stop_dead_process_is_noop() {
        // Lock file exists but process is dead (kill -0 returns non-zero).
        let lock_json = r#"{"pid":12345,"tcp_url":"tcp://127.0.0.1:50001"}"#;
        let ssh = MockRemoteExec::new(vec![
            (0, lock_json, ""), // cat lock
            (1, "", ""),        // kill -0 → no such process
        ]);
        let cmds = Rc::clone(&ssh.commands);
        run_agent_stop("/home/user", &ssh).unwrap();
        // Exactly two commands: lock-read + kill-0.
        let issued = cmds.borrow();
        assert_eq!(issued.len(), 2, "only two commands expected: {issued:?}");
        assert!(issued[1].contains("kill -0"), "second cmd must be kill -0: {:?}", issued[1]);
    }

    #[test]
    fn stop_sigterm_is_sufficient() {
        // Process alive; SIGTERM is enough — no SIGKILL issued.
        let lock_json = r#"{"pid":22222}"#;
        let ssh = MockRemoteExec::new(vec![
            (0, lock_json, ""), // cat lock
            (0, "", ""),        // kill -0 → alive
            (0, "", ""),        // kill -TERM
            (1, "", ""),        // poll → agent gone after TERM
        ]);
        let cmds = Rc::clone(&ssh.commands);
        run_agent_stop("/home/user", &ssh).unwrap();
        let issued = cmds.borrow();
        // Commands: cat lock, kill -0, kill -TERM, poll. No SIGKILL.
        assert_eq!(issued.len(), 4, "expected exactly 4 commands: {issued:?}");
        assert!(issued[2].contains("TERM"), "third command must be SIGTERM: {:?}", issued[2]);
        assert!(!issued.iter().any(|c| c.contains("KILL")), "SIGKILL must not be issued: {issued:?}");
    }

    #[test]
    fn stop_sigkill_fallback_when_sigterm_insufficient() {
        // Process survives SIGTERM poll — escalate to SIGKILL; final kill -0 confirms dead.
        let lock_json = r#"{"pid":33333}"#;
        let ssh = MockRemoteExec::new(vec![
            (0, lock_json, ""), // cat lock
            (0, "", ""),        // kill -0 → alive
            (0, "", ""),        // kill -TERM
            (0, "", ""),        // poll → still alive after TERM
            (0, "", ""),        // kill -KILL
            (1, "", ""),        // kill -0 final check → dead
        ]);
        let cmds = Rc::clone(&ssh.commands);
        run_agent_stop("/home/user", &ssh).unwrap();
        let issued = cmds.borrow();
        assert!(
            issued.iter().any(|c| c.contains("KILL")),
            "SIGKILL must be issued when SIGTERM is not enough: {issued:?}"
        );
    }

    #[test]
    fn stop_sigkill_fails_returns_error() {
        // SIGKILL also fails to stop the process → honest error, not silent success.
        let lock_json = r#"{"pid":55555}"#;
        let ssh = MockRemoteExec::new(vec![
            (0, lock_json, ""), // cat lock
            (0, "", ""),        // kill -0 → alive
            (0, "", ""),        // kill -TERM
            (0, "", ""),        // poll → still alive
            (0, "", ""),        // kill -KILL
            (0, "", ""),        // kill -0 final check → still alive!
        ]);
        let err = run_agent_stop("/home/user", &ssh).unwrap_err();
        assert!(
            err.to_string().contains("may still be running"),
            "must surface honest error when SIGKILL fails: {err}"
        );
    }

    #[test]
    fn stop_with_tcp_url_in_lock_uses_kill_path() {
        // Even when the lock has a tcp_url, stop uses SIGTERM (no RPC — SSH tunnel not yet
        // established; the agent's RPC port binds to remote loopback, not local loopback).
        let lock_json = r#"{"pid":44444,"tcp_url":"tcp://127.0.0.1:59998"}"#;
        let ssh = MockRemoteExec::new(vec![
            (0, lock_json, ""), // cat lock
            (0, "", ""),        // kill -0 → alive
            (0, "", ""),        // kill -TERM
            (1, "", ""),        // poll → agent gone after TERM
        ]);
        let cmds = Rc::clone(&ssh.commands);
        run_agent_stop("/home/user", &ssh).unwrap();
        let issued = cmds.borrow();
        assert!(
            issued.iter().any(|c| c.contains("TERM")),
            "SIGTERM must be used as the stop mechanism: {issued:?}"
        );
        // No RPC attempt — no connection to 127.0.0.1:59998.
        assert_eq!(issued.len(), 4, "expected exactly 4 commands (cat/kill-0/TERM/poll): {issued:?}");
    }

    #[test]
    fn parse_lock_tcp_url_empty_returns_none() {
        assert_eq!(parse_lock_tcp_url(r#"{"pid":1,"tcp_url":""}"#), None);
    }

    // ── select_agent_binary ───────────────────────────────────────────────────
    //
    // The adjacent-binary success path (current_exe() + "mux-agent-{arch}") is not
    // unit-testable: current_exe() resolves to the test runner binary, not mux.
    // That path is covered by integration tests (mux-zpx). Unit tests here cover
    // the MUX_AGENT_BINARY env-var path and the adjacent-lookup error paths.

    #[test]
    fn select_agent_binary_env_var_existing_path_succeeds() {
        let _lock = env_lock();
        let dir = TempDir::new().unwrap();
        let bin_path = dir.path().join("mux-agent-custom");
        std::fs::write(&bin_path, b"fake").unwrap();
        std::env::set_var("MUX_AGENT_BINARY", bin_path.to_str().unwrap());
        let _guard = EnvGuard("MUX_AGENT_BINARY");

        let (path, _version) = select_agent_binary("amd64").unwrap();
        assert_eq!(path, bin_path, "should return the path set in MUX_AGENT_BINARY");
    }

    #[test]
    fn select_agent_binary_env_var_nonexistent_path_errors() {
        let _lock = env_lock();
        std::env::set_var("MUX_AGENT_BINARY", "/nonexistent/mux-agent");
        let _guard = EnvGuard("MUX_AGENT_BINARY");

        let err = select_agent_binary("amd64").unwrap_err();
        assert!(
            err.to_string().contains("MUX_AGENT_BINARY"),
            "error should mention MUX_AGENT_BINARY, got: {err}"
        );
    }

    #[test]
    fn select_agent_binary_empty_env_var_falls_through_to_adjacent_lookup() {
        let _lock = env_lock();
        // Empty string: treated as "not set" — falls through to adjacent-exe lookup.
        std::env::set_var("MUX_AGENT_BINARY", "");
        let _guard = EnvGuard("MUX_AGENT_BINARY");

        // Adjacent binary won't exist (test runner is not mux), so we get the
        // "no agent binary found" error from the adjacent lookup — not a
        // "path does not exist" error from the MUX_AGENT_BINARY branch.
        let err = select_agent_binary("amd64").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no agent binary found"),
            "empty MUX_AGENT_BINARY should fall through to adjacent lookup error, got: {msg}"
        );
        assert!(
            !msg.contains("does not exist"),
            "empty MUX_AGENT_BINARY should not hit the MUX_AGENT_BINARY path, got: {msg}"
        );
    }

    #[test]
    fn select_agent_binary_unset_env_var_reports_helpful_error() {
        let _lock = env_lock();
        std::env::remove_var("MUX_AGENT_BINARY");

        // Adjacent binary won't exist (test runner is not mux), so we get an error
        // naming the expected binary and telling the user to set MUX_AGENT_BINARY.
        let err = select_agent_binary("amd64").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("MUX_AGENT_BINARY"), "error should hint at MUX_AGENT_BINARY, got: {err}");
        assert!(msg.contains("mux-agent-amd64"), "error should name the expected binary, got: {err}");
    }

    #[test]
    fn select_agent_binary_arm64_not_found_names_expected_binary() {
        let _lock = env_lock();
        std::env::remove_var("MUX_AGENT_BINARY");

        let err = select_agent_binary("arm64").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("mux-agent-arm64"), "error should name mux-agent-arm64, got: {err}");
        assert!(msg.contains("MUX_AGENT_BINARY"), "error should hint at env var, got: {err}");
    }

    #[test]
    fn detect_version_falls_back_to_package_version_for_non_executable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fake-mux-agent");
        std::fs::write(&path, b"not-a-real-binary").unwrap();
        // Explicitly remove the execute bit so Command::output() returns Err(EACCES),
        // not just a non-ELF format error, making the precondition unambiguous.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let version = detect_version(&path);
        assert_eq!(
            version,
            env!("CARGO_PKG_VERSION"),
            "detect_version should fall back to CARGO_PKG_VERSION for non-executable"
        );
    }

    #[test]
    fn is_local_arch_normalised_mux_names_match_uname_names() {
        // Verify that the mux arch names (amd64, arm64) are accepted as equivalent
        // to the uname names (x86_64, aarch64) by is_local_arch.
        // Run the subset of assertions relevant to the current machine's architecture.
        let local = std::env::consts::ARCH;
        match local {
            "x86_64" => {
                assert!(is_local_arch("amd64"), "amd64 should be local on x86_64");
                assert!(is_local_arch("x86_64"), "x86_64 should be local on x86_64");
                assert!(!is_local_arch("arm64"), "arm64 should not be local on x86_64");
                assert!(!is_local_arch("aarch64"), "aarch64 should not be local on x86_64");
            }
            "aarch64" => {
                assert!(is_local_arch("arm64"), "arm64 should be local on aarch64");
                assert!(is_local_arch("aarch64"), "aarch64 should be local on aarch64");
                assert!(!is_local_arch("amd64"), "amd64 should not be local on aarch64");
                assert!(!is_local_arch("x86_64"), "x86_64 should not be local on aarch64");
            }
            _ => {
                // Unknown host arch (e.g. RISC-V): verify neither mux arch is local
                // and the function doesn't panic.
                assert!(!is_local_arch("amd64"), "amd64 should not match {local}");
                assert!(!is_local_arch("arm64"), "arm64 should not match {local}");
            }
        }
    }

    #[test]
    fn version_for_arch_cross_arch_uses_package_version() {
        // version_for_arch skips binary execution for non-local arches and returns
        // CARGO_PKG_VERSION directly. The binary content is irrelevant — the
        // is_local_arch check short-circuits before any Command::output() call.
        let dir = TempDir::new().unwrap();
        let fake_path = dir.path().join("mux-agent-fake");
        std::fs::write(&fake_path, b"cross-arch-binary").unwrap();

        // Pick an arch that is NOT the local machine's arch.
        let cross_arch = if std::env::consts::ARCH == "x86_64" { "arm64" } else { "amd64" };
        let version = version_for_arch(&fake_path, cross_arch);
        assert_eq!(
            version,
            env!("CARGO_PKG_VERSION"),
            "cross-arch deploy should use CARGO_PKG_VERSION, not attempt binary execution"
        );
    }
}
