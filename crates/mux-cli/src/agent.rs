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
            let version = detect_version(&path);
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
            "no agent binary found for arch '{arch}'; \
             set MUX_AGENT_BINARY to the path of the mux-agent binary"
        );
    }

    let version = detect_version(&path);
    Ok((path, version))
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

/// Parse sha256 hex digest from either `sha256sum` or `openssl dgst -sha256` output.
///
/// - `sha256sum`:          `<64-hex>  <filename>\n`
/// - `openssl dgst -sha256`: `SHA256(<filename>)= <64-hex>\n`
pub fn parse_sha256_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        // sha256sum: first whitespace-delimited field is the hash
        if let Some(first) = line.split_whitespace().next() {
            if first.len() == 64 && first.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(first.to_lowercase());
            }
        }
        // openssl: "SHA256(path)= <hash>"
        if let Some(after_eq) = line.rfind('=').and_then(|i| line.get(i + 1..)) {
            let hash = after_eq.trim();
            if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(hash.to_lowercase());
            }
        }
    }
    None
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
        .run(&format!("mkdir -p '{remote_dir}'"))
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
        .run(&format!("chmod +x '{remote_path}'"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if code != 0 {
        bail!(
            "mux: chmod +x failed for {remote_path}: {}",
            stderr.trim()
        );
    }

    // ── 9. Persist version (only after verified upload) ───────────────────────
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

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Attempt graceful stop of the running agent, if any.
///
/// 1. Read lock file for PID.
/// 2. Verify process is alive (`kill -0`).
/// 3. Send SIGTERM; poll up to ~3 s.
/// 4. If still alive, send SIGKILL.
fn stop_agent_if_running<S: RemoteExec>(ssh: &S, lock_path: &str) -> Result<()> {
    // Read lock file (best-effort — missing lock means no agent).
    let (code, stdout, _) = ssh
        .run(&format!("cat '{lock_path}' 2>/dev/null"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if code != 0 || stdout.trim().is_empty() {
        return Ok(());
    }

    let pid = match parse_lock_pid(&stdout) {
        Some(p) if p > 0 => p,
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
        .run(&format!("wc -c < '{remote_path}'"))
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
    let (code, stdout, _) = ssh
        .run(&format!(
            "sha256sum '{remote_path}' 2>/dev/null || openssl dgst -sha256 '{remote_path}'"
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

    // ── Fake binary fixture ───────────────────────────────────────────────────

    /// Write a small fake binary file and set MUX_AGENT_BINARY to its path.
    /// Returns (TempDir, content_bytes, sha256_hex).
    fn setup_fake_binary() -> (TempDir, Vec<u8>, String) {
        let dir = TempDir::new().unwrap();
        let content: Vec<u8> = b"fake-mux-agent-binary-content-for-testing".to_vec();
        let path = dir.path().join("mux-agent-fake");
        std::fs::write(&path, &content).unwrap();
        std::env::set_var("MUX_AGENT_BINARY", path.to_str().unwrap());
        let hash = sha256_hex(&content);
        (dir, content, hash)
    }

    // ── Happy path ────────────────────────────────────────────────────────────

    #[test]
    fn deploy_happy_path_persists_version() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_bin_dir, content, hash) = setup_fake_binary();
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
        let (_bin_dir, content, hash) = setup_fake_binary();
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
        let (_bin_dir, content, _hash) = setup_fake_binary();

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
        let (_bin_dir, content, _hash) = setup_fake_binary();
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
        let _bin_dir = setup_fake_binary();

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

    // ── Graceful stop ─────────────────────────────────────────────────────────

    #[test]
    fn deploy_stops_running_agent_before_upload() {
        let (_store_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_with_probe(conn, "prod");
        let (_bin_dir, content, hash) = setup_fake_binary();
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
        let (_bin_dir, content, hash) = setup_fake_binary();
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
}
