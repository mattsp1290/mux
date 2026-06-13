use anyhow::{bail, Result};
use rusqlite::Connection;
use std::str::FromStr;

use mux_core::types::{HostAlias, Port};
use mux_state::host_repo;

use crate::HostAction;

pub async fn run_host(action: HostAction, conn: &Connection) -> Result<()> {
    match action {
        HostAction::Add { alias, user_at_addr, port } => cmd_add(conn, alias, user_at_addr, port),
        HostAction::List => cmd_list(conn),
        HostAction::Remove { alias, yes } => cmd_remove(conn, alias, yes).await,
        // TODO: wire to cmd_test_core once a real SshHost SSH impl lands (host
        // lookup via get_by_alias, build executor, derive is_interactive from
        // std::io::IsTerminal).
        HostAction::Test { alias } => bail!("SSH not yet implemented (host test for '{alias}')"),
        // TODO: wire to cmd_trust_core once a real SshHost SSH impl lands.
        HostAction::Trust { alias } => bail!("SSH not yet implemented (host trust for '{alias}')"),
    }
}

fn cmd_add(conn: &Connection, alias: String, user_at_addr: String, port: u16) -> Result<()> {
    // Validate alias
    let alias = HostAlias::from_str(&alias)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Split user@addr on first @
    let at = user_at_addr.find('@')
        .ok_or_else(|| anyhow::anyhow!("expected user@addr, got: {:?}", user_at_addr))?;
    let user = &user_at_addr[..at];
    let addr = &user_at_addr[at + 1..];

    if user.is_empty() { bail!("user part of user@addr must not be empty"); }
    if addr.is_empty() { bail!("addr part of user@addr must not be empty"); }

    // Tilde expansion: if addr starts with ~, expand to $HOME/<rest>.
    // Error on empty HOME rather than silently producing a malformed path.
    let addr = if addr.starts_with('~') {
        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| anyhow::anyhow!("tilde expansion requires HOME to be set"))?;
        let rest = addr.strip_prefix('~').unwrap_or("");
        if rest.is_empty() { home } else { format!("{home}{rest}") }
    } else {
        addr.to_owned()
    };

    // Validate port
    let port = Port::from_str(&port.to_string())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    match host_repo::insert(conn, alias.as_str(), user, &addr, port.value() as i64, created_at) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Check full error chain (anyhow wraps the SQLite error with context)
            let msg = format!("{e:#}").to_lowercase();
            if msg.contains("unique constraint failed") {
                bail!("host '{}' already exists", alias.as_str())
            }
            Err(e)
        }
    }
}

fn cmd_list(conn: &Connection) -> Result<()> {
    let hosts = host_repo::list(conn)?;

    if hosts.is_empty() {
        println!("No hosts configured. Use 'mux host add' to add one.");
        return Ok(());
    }

    let alias_w = hosts.iter().map(|h| h.alias.len()).max().unwrap_or(5).max(5);
    let user_addr_w = hosts.iter().map(|h| format!("{}@{}", h.user, h.addr).len()).max().unwrap_or(12).max("USER@ADDR".len());
    let port_w = hosts.iter().map(|h| h.port.to_string().len()).max().unwrap_or(4).max("PORT".len());
    let arch_w = "ARCH".len().max(7);

    println!(
        "{:<alias_w$}  {:<user_addr_w$}  {:<port_w$}  {:<arch_w$}  HOME",
        "ALIAS", "USER@ADDR", "PORT", "ARCH"
    );

    for host in &hosts {
        let user_addr = format!("{}@{}", host.user, host.addr);
        let arch = host.arch.as_deref().unwrap_or("-");
        let home = host.home.as_deref().unwrap_or("-");
        println!(
            "{:<alias_w$}  {:<user_addr_w$}  {:<port_w$}  {:<arch_w$}  {}",
            host.alias, user_addr, host.port, arch, home
        );
    }

    Ok(())
}

async fn cmd_remove(conn: &Connection, alias: String, yes: bool) -> Result<()> {
    let alias = HostAlias::from_str(&alias)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let host = host_repo::get_by_alias(conn, alias.as_str())?
        .ok_or_else(|| anyhow::anyhow!("host '{}' not found", alias.as_str()))?;

    if !yes {
        eprint!(
            "Remove host '{}' ({}@{}:{})? [y/N] ",
            host.alias, host.user, host.addr, host.port
        );
        use std::io::BufRead;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let response = line.trim().to_lowercase();
        if response != "y" {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    host_repo::delete(conn, host.id)?;
    println!("Removed host '{}'.", host.alias);
    Ok(())
}

// ── sh_quote ──────────────────────────────────────────────────────────────────

/// Single-quote a string for safe use as one shell word.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ── Preflight sentinel constants ──────────────────────────────────────────────

const SENTINEL_START: &str = "MUX_SENTINEL_V1";
const SENTINEL_END: &str = "MUX_SENTINEL_V1_END";

/// Result of running the preflight sentinel command on a remote host.
#[derive(Debug)]
pub struct PreflightResult {
    pub arch: String,
    pub home: String,
    pub tmux_version: String,
}

/// Run the preflight sentinel command and parse the output.
///
/// The command is designed to discard MOTD noise by only consuming output
/// between the two sentinel markers.
pub fn run_preflight<R: crate::create::SshHost>(
    ssh: &R,
) -> Result<PreflightResult> {
    let cmd = concat!(
        "printf '%s\\n' 'MUX_SENTINEL_V1' && ",
        "uname -m && ",
        "printf '%s\\n' \"$HOME\" && ",
        "(tmux -V 2>&1 || echo 'tmux-not-found') && ",
        "printf '%s\\n' 'MUX_SENTINEL_V1_END'"
    );

    let (code, stdout, stderr) = ssh.run(cmd)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if code != 0 {
        bail!("preflight command failed (exit {}): {}", code, stderr.trim());
    }

    parse_preflight_output(&stdout)
}

/// Parse the preflight stdout: discard everything before SENTINEL_START, take
/// the lines between the two sentinels.
pub fn parse_preflight_output(output: &str) -> Result<PreflightResult> {
    let lines: Vec<&str> = output.lines().collect();

    // Find start sentinel
    let start_idx = lines.iter().position(|l| *l == SENTINEL_START)
        .ok_or_else(|| anyhow::anyhow!("preflight: missing start sentinel"))?;

    // Find end sentinel after start
    let end_idx = lines[start_idx + 1..].iter().position(|l| *l == SENTINEL_END)
        .map(|i| i + start_idx + 1)
        .ok_or_else(|| anyhow::anyhow!("preflight: missing end sentinel"))?;

    let inner: Vec<&str> = lines[start_idx + 1..end_idx].to_vec();

    if inner.len() < 3 {
        bail!("preflight: expected 3 lines between sentinels, got {}", inner.len());
    }

    let raw_arch = inner[0].trim();
    let home = inner[1].trim().to_owned();
    let tmux_line = inner[2].trim();

    let arch = normalize_arch(raw_arch);
    let tmux_version = parse_tmux_version(tmux_line)?;

    Ok(PreflightResult { arch, home, tmux_version })
}

/// Normalize architecture strings to common mux names.
pub fn normalize_arch(raw: &str) -> String {
    match raw {
        "x86_64" => "amd64".to_owned(),
        "aarch64" => "arm64".to_owned(),
        other => other.to_owned(),
    }
}

/// Parse tmux version from `tmux X.Y[suffix]` output.
///
/// Returns the version string (e.g. "3.3a") or an error if:
/// - the string is "tmux-not-found"
/// - the version is < 3.0
/// - the format is unrecognizable
pub fn parse_tmux_version(line: &str) -> Result<String> {
    if line == "tmux-not-found" {
        bail!("tmux is not installed on the remote host");
    }

    // Expected format: "tmux X.Y[suffix]"
    let version_part = line.strip_prefix("tmux ").ok_or_else(|| {
        anyhow::anyhow!("unrecognized tmux version line: {:?}", line)
    })?;

    // Parse major version number
    let major_str = version_part.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();

    let major: u32 = major_str.parse()
        .map_err(|_| anyhow::anyhow!("could not parse tmux major version from: {:?}", line))?;

    if major < 3 {
        bail!("tmux version {} is too old (require >= 3.0)", version_part);
    }

    Ok(version_part.to_owned())
}

/// Core logic for `mux host test`: verifies SSH agent, TOFU host key, runs
/// preflight, probes transport, and persists the results.
///
/// `is_interactive` controls whether a first-contact host key prompt is shown.
pub fn cmd_test_core<S: crate::create::SshHost>(
    conn: &Connection,
    host: &mux_state::model::Host,
    ssh: &S,
    is_interactive: bool,
) -> Result<()> {
    // Step 1: Verify SSH agent is loaded.
    mux_ssh::trust::list_agent_keys()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Step 2: TOFU host key check.
    let key_info = ssh.host_key().map_err(|e| anyhow::anyhow!("{e}"))?;
    let trust_result = mux_ssh::trust::check_host_key(
        conn,
        host.id,
        &key_info.algorithm,
        &key_info.fingerprint,
    )?;

    use mux_ssh::trust::TrustCheckResult;
    match trust_result {
        TrustCheckResult::Trusted => {
            // Proceed silently — idempotent.
        }
        TrustCheckResult::FirstContact { algorithm, fingerprint } => {
            if !is_interactive {
                bail!(
                    "Host '{}' has no stored fingerprint and session is non-interactive. \
                     Run 'mux host test {}' in an interactive terminal to trust it.",
                    host.alias,
                    host.alias
                );
            }
            eprintln!(
                "The authenticity of host '{}' can't be established.",
                host.alias
            );
            eprintln!("{algorithm} key fingerprint is {fingerprint}");
            eprint!("Are you sure you want to continue connecting? (yes/no): ");
            let accepted = read_yes_no()?;
            if !accepted {
                bail!("host key rejected by user");
            }
            mux_ssh::trust::trust_fingerprint(conn, host.id, &algorithm, &fingerprint)?;
        }
        TrustCheckResult::Mismatch { algorithm, stored, received } => {
            bail!(
                "WARNING: Host key mismatch for '{}'!\n\
                 Algorithm:  {}\n\
                 Stored:     {}\n\
                 Received:   {}\n\
                 Refusing to connect.",
                host.alias,
                algorithm,
                stored,
                received
            );
        }
    }

    // Step 3: Run preflight sentinel commands.
    let preflight = run_preflight(ssh)?;

    // Step 4 (arch normalization already done in run_preflight/normalize_arch).
    let arch = preflight.arch;
    let home = preflight.home;

    // Step 5: Probe transport.
    // exit 0 → socket exists → streamlocal; exit 1 → no socket → tcp;
    // any other exit code → command failed (permission denied, not a shell, etc.) → error.
    let sock_path = format!("{}/.mux/agent.sock", home);
    let transport = {
        let (code, _, stderr) = ssh.run(&format!("test -S {}", sh_quote(&sock_path)))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        match code {
            0 => "streamlocal",
            1 => "tcp",
            _ => bail!("transport probe failed (exit {}): {}", code, stderr.trim()),
        }
    };

    // Step 6: Persist via host_repo::update_probe.
    // TODO: also persist tmux_version (spec §host-test: "Persists: …tmux_version, tool
    // availability"). Requires adding a tmux_version column to the hosts table and
    // extending host_repo::update_probe — deferred to a schema migration iteration.
    mux_state::host_repo::update_probe(conn, host.id, Some(&arch), Some(&home), Some(transport))?;

    // Step 7: Print confirmation.
    println!(
        "Host '{}' configured: arch={}, home={}, transport={}, tmux={}",
        host.alias, arch, home, transport, preflight.tmux_version
    );

    Ok(())
}

/// Core logic for `mux host trust`: shows stored fingerprints, fetches the
/// current remote key, and prompts the user to accept changes.
pub fn cmd_trust_core<S: crate::create::SshHost>(
    conn: &Connection,
    host: &mux_state::model::Host,
    ssh: &S,
) -> Result<()> {
    // Step 1: Load stored fingerprints.
    let stored = mux_state::fingerprint_repo::list_for_host(conn, host.id)?;

    // Step 2: If empty, instruct user to run test first.
    if stored.is_empty() {
        println!(
            "No stored fingerprint for '{}'. Run `mux host test {}` first.",
            host.alias, host.alias
        );
        return Ok(());
    }

    // Step 3: Print current fingerprints.
    println!("Stored fingerprints for '{}':", host.alias);
    for fp in &stored {
        println!("  {} {}", fp.algorithm, fp.fingerprint);
    }

    // Step 4: Fetch current remote key.
    let key_info = ssh.host_key().map_err(|e| anyhow::anyhow!("{e}"))?;

    // Step 5: Check against stored.
    let trust_result = mux_ssh::trust::check_host_key(
        conn,
        host.id,
        &key_info.algorithm,
        &key_info.fingerprint,
    )?;

    use mux_ssh::trust::TrustCheckResult;
    match trust_result {
        TrustCheckResult::Trusted => {
            println!("Host key for '{}' is unchanged.", host.alias);
        }
        TrustCheckResult::Mismatch { algorithm, stored: stored_fp, received } => {
            println!("Host key changed for '{}'!", host.alias);
            println!("  Algorithm: {}", algorithm);
            println!("  Old:       {}", stored_fp);
            println!("  New:       {}", received);
            eprint!("Trust new key? (yes/no): ");
            let accepted = read_yes_no()?;
            if !accepted {
                bail!("key rotation declined");
            }
            mux_ssh::trust::trust_fingerprint(conn, host.id, &algorithm, &received)?;
            println!("New key trusted for '{}'.", host.alias);
        }
        TrustCheckResult::FirstContact { algorithm, fingerprint } => {
            println!("New algorithm '{}' seen for '{}':", algorithm, host.alias);
            println!("  Fingerprint: {}", fingerprint);
            eprint!("Trust this key? (yes/no): ");
            let accepted = read_yes_no()?;
            if !accepted {
                bail!("key rotation declined");
            }
            mux_ssh::trust::trust_fingerprint(conn, host.id, &algorithm, &fingerprint)?;
            println!("Key trusted for '{}'.", host.alias);
        }
    }

    Ok(())
}

/// Read a yes/no answer from stdin. Returns `true` for "yes"/"y".
fn read_yes_no() -> Result<bool> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    Ok(trimmed == "yes" || trimmed == "y")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux_core::error::MuxError;
    use mux_state::store::Store;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use tempfile::TempDir;

    use crate::create::{HostKeyInfo, SshHost};
    use crate::agent_start::RemoteExec;

    // ── MockSshHost ───────────────────────────────────────────────────────────

    struct MockSshHost {
        responses: RefCell<VecDeque<(i32, String, String)>>,
        host_key_result: Result<HostKeyInfo, MuxError>,
    }

    impl MockSshHost {
        fn new(
            responses: Vec<(i32, &str, &str)>,
            host_key_result: Result<HostKeyInfo, MuxError>,
        ) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(c, o, e)| (c, o.to_owned(), e.to_owned()))
                        .collect(),
                ),
                host_key_result,
            }
        }

        fn with_trusted_key(responses: Vec<(i32, &str, &str)>) -> Self {
            Self::new(
                responses,
                Ok(HostKeyInfo {
                    algorithm: "ssh-ed25519".to_owned(),
                    fingerprint: "SHA256:AAAA".to_owned(),
                }),
            )
        }
    }

    impl RemoteExec for MockSshHost {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
            let mut q = self.responses.borrow_mut();
            Ok(q.pop_front().unwrap_or((1, String::new(), "mock: no more responses".to_owned())))
        }
    }

    impl SshHost for MockSshHost {
        fn host_key(&self) -> Result<HostKeyInfo, MuxError> {
            match &self.host_key_result {
                Ok(info) => Ok(info.clone()),
                Err(e) => Err(match e {
                    MuxError::HostKeyMismatch => MuxError::HostKeyMismatch,
                    MuxError::TofuNonInteractive => MuxError::TofuNonInteractive,
                    _ => MuxError::HostKeyMismatch,
                }),
            }
        }
    }

    // ── DB helpers ────────────────────────────────────────────────────────────

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store = Store::open(&db_path).unwrap();
        (dir, store)
    }

    fn insert_host(conn: &Connection, alias: &str) -> mux_state::model::Host {
        let id = mux_state::host_repo::insert(conn, alias, "user", "10.0.0.1", 22, 1_000_000).unwrap();
        mux_state::host_repo::get_by_id(conn, id).unwrap().unwrap()
    }

    fn trust_key(conn: &Connection, host_id: i64) {
        mux_state::fingerprint_repo::upsert(
            conn, host_id, "ssh-ed25519", "SHA256:AAAA", 1_000_000,
        ).unwrap();
    }

    // ── existing tests ────────────────────────────────────────────────────────

    #[test]
    fn add_success_inserts_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "myhost".to_owned(), "user@10.0.0.1".to_owned(), 22).unwrap();
        let host = host_repo::get_by_alias(conn, "myhost").unwrap().expect("should exist");
        assert_eq!(host.alias, "myhost");
        assert_eq!(host.user, "user");
        assert_eq!(host.addr, "10.0.0.1");
        assert_eq!(host.port, 22);
    }

    #[test]
    fn add_duplicate_alias_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "myhost".to_owned(), "user@10.0.0.1".to_owned(), 22).unwrap();
        let err = cmd_add(conn, "myhost".to_owned(), "user2@10.0.0.2".to_owned(), 22)
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "got: {err}");
    }

    #[test]
    fn add_invalid_alias_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_add(conn, "has.dot".to_owned(), "user@10.0.0.1".to_owned(), 22)
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn add_missing_at_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_add(conn, "myhost".to_owned(), "noatsign".to_owned(), 22)
            .unwrap_err();
        assert!(err.to_string().contains("user@addr"), "got: {err}");
    }

    #[test]
    fn add_tilde_expansion() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        // Set HOME to a known value for the test
        std::env::set_var("HOME", "/home/testuser");
        cmd_add(conn, "tildehost".to_owned(), "user@~/workspace".to_owned(), 22).unwrap();
        let host = host_repo::get_by_alias(conn, "tildehost").unwrap().expect("should exist");
        assert_eq!(host.addr, "/home/testuser/workspace");
    }

    #[test]
    fn list_empty_prints_message() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        // Should not panic
        cmd_list(conn).unwrap();
    }

    #[test]
    fn list_one_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "prod".to_owned(), "alice@prod.example".to_owned(), 2222).unwrap();
        // Should not panic and should list the host
        cmd_list(conn).unwrap();
    }

    #[tokio::test]
    async fn remove_yes_removes_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "toremove".to_owned(), "user@10.0.0.5".to_owned(), 22).unwrap();
        cmd_remove(conn, "toremove".to_owned(), true).await.unwrap();
        let result = host_repo::get_by_alias(conn, "toremove").unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn remove_yes_host_not_found() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_remove(conn, "nosuchhost".to_owned(), true).await.unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn remove_yes_cascade() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "cascadehost".to_owned(), "user@10.0.0.6".to_owned(), 22).unwrap();
        let host = host_repo::get_by_alias(conn, "cascadehost").unwrap().unwrap();
        // Insert a fingerprint for this host
        conn.execute(
            "INSERT INTO known_host_fingerprints (host_id, algorithm, fingerprint, trusted_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![host.id, "ed25519", "AAAA1234", 1_000_000i64],
        ).unwrap();
        // Remove the host
        cmd_remove(conn, "cascadehost".to_owned(), true).await.unwrap();
        // Fingerprint should be gone
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM known_host_fingerprints WHERE host_id = ?1",
            rusqlite::params![host.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "fingerprints should be cascade-deleted");
    }

    // ── add validation edge cases ─────────────────────────────────────────────

    #[test]
    fn add_empty_user_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_add(conn, "myhost".to_owned(), "@10.0.0.1".to_owned(), 22).unwrap_err();
        // Use "user part" not just "user" — both messages contain "user@addr" as a substring.
        assert!(err.to_string().contains("user part"), "got: {err}");
    }

    #[test]
    fn add_empty_addr_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_add(conn, "myhost".to_owned(), "user@".to_owned(), 22).unwrap_err();
        // Use "addr part" not just "addr" — both messages contain "user@addr" as a substring.
        assert!(err.to_string().contains("addr part"), "got: {err}");
    }

    // ── list: multiple hosts ──────────────────────────────────────────────────

    #[test]
    fn list_multiple_hosts_does_not_error() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "alpha".to_owned(), "alice@10.0.0.1".to_owned(), 22).unwrap();
        cmd_add(conn, "beta".to_owned(), "bob@10.0.0.2".to_owned(), 2222).unwrap();
        // Verify cmd_list doesn't error with multiple hosts in the table.
        cmd_list(conn).unwrap();
        let hosts = host_repo::list(conn).unwrap();
        assert_eq!(hosts.len(), 2, "both hosts should be in inventory");
    }

    // ── remove: confirmation decline ─────────────────────────────────────────

    #[tokio::test]
    async fn remove_no_confirmation_aborts_without_removing() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "keephost".to_owned(), "user@10.0.0.7".to_owned(), 22).unwrap();
        // yes=false; cargo test provides empty/EOF stdin so read_line returns "" →
        // trimmed != "y" → abort path taken. Same convention as cmd_trust_mismatch_declined.
        cmd_remove(conn, "keephost".to_owned(), false).await.unwrap();
        let host = host_repo::get_by_alias(conn, "keephost").unwrap();
        assert!(host.is_some(), "host should still exist after declined confirmation");
        let all = host_repo::list(conn).unwrap();
        assert_eq!(all.len(), 1, "no other rows should have been deleted");
    }

    // ── new: preflight parsing ────────────────────────────────────────────────

    #[test]
    fn run_preflight_extracts_fields() {
        // MOTD noise before sentinel is discarded
        let output = "Welcome to Ubuntu 22.04\nLast login: ...\nMUX_SENTINEL_V1\nx86_64\n/home/user\ntmux 3.3a\nMUX_SENTINEL_V1_END\n";
        let result = parse_preflight_output(output).unwrap();
        assert_eq!(result.arch, "amd64");
        assert_eq!(result.home, "/home/user");
        assert_eq!(result.tmux_version, "3.3a");
    }

    #[test]
    fn normalize_arch_x86_64() {
        assert_eq!(normalize_arch("x86_64"), "amd64");
    }

    #[test]
    fn normalize_arch_aarch64() {
        assert_eq!(normalize_arch("aarch64"), "arm64");
    }

    #[test]
    fn normalize_arch_unknown_passthrough() {
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn parse_tmux_version_ok() {
        assert_eq!(parse_tmux_version("tmux 3.3a").unwrap(), "3.3a");
    }

    #[test]
    fn parse_tmux_version_too_old() {
        let err = parse_tmux_version("tmux 2.9").unwrap_err();
        assert!(err.to_string().contains("too old"), "got: {err}");
    }

    #[test]
    fn parse_tmux_version_not_found() {
        let err = parse_tmux_version("tmux-not-found").unwrap_err();
        assert!(err.to_string().contains("not installed"), "got: {err}");
    }

    // ── new: cmd_test_core ────────────────────────────────────────────────────

    /// Preflight parse + arch normalize + update_probe integration.
    #[test]
    fn cmd_test_components_happy_path() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host(conn, "prod");

        // TOFU check: stored key matches → Trusted.
        trust_key(conn, host.id);
        let trust_result = mux_ssh::trust::check_host_key(
            conn, host.id, "ssh-ed25519", "SHA256:AAAA",
        ).unwrap();
        assert_eq!(trust_result, mux_ssh::trust::TrustCheckResult::Trusted);

        // Preflight parse with MOTD noise.
        let output = "Welcome!\nMUX_SENTINEL_V1\nx86_64\n/home/user\ntmux 3.3a\nMUX_SENTINEL_V1_END\n";
        let preflight = parse_preflight_output(output).unwrap();
        assert_eq!(preflight.arch, "amd64");
        assert_eq!(preflight.home, "/home/user");
        assert_eq!(preflight.tmux_version, "3.3a");

        // Persist and verify.
        mux_state::host_repo::update_probe(
            conn, host.id, Some("amd64"), Some("/home/user"), Some("tcp"),
        ).unwrap();
        let updated = mux_state::host_repo::get_by_id(conn, host.id).unwrap().unwrap();
        assert_eq!(updated.arch.as_deref(), Some("amd64"));
        assert_eq!(updated.home.as_deref(), Some("/home/user"));
        assert_eq!(updated.transport.as_deref(), Some("tcp"));
    }

    // ── cmd_trust_core tests ───────────────────────────────────────────────────

    /// No stored fingerprint → instruct user to run host test first.
    #[test]
    fn cmd_trust_no_stored_fingerprint_prints_message() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host(conn, "newhost");
        let ssh = MockSshHost::with_trusted_key(vec![]);
        // Should succeed (not error) and print a message.
        cmd_trust_core(conn, &host, &ssh).unwrap();
    }

    /// Trusted key → unchanged message, no prompt.
    #[test]
    fn cmd_trust_trusted_key_unchanged() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host(conn, "trustedhost");
        trust_key(conn, host.id);
        // MockSshHost returns SHA256:AAAA → matches stored → Trusted.
        let ssh = MockSshHost::with_trusted_key(vec![]);
        cmd_trust_core(conn, &host, &ssh).unwrap();
        // Fingerprint should still be SHA256:AAAA (unchanged).
        let fp = mux_state::fingerprint_repo::get(conn, host.id, "ssh-ed25519")
            .unwrap().unwrap();
        assert_eq!(fp.fingerprint, "SHA256:AAAA");
    }

    // ── parse_preflight_output edge cases ─────────────────────────────────────

    #[test]
    fn parse_preflight_output_no_start_sentinel_errors() {
        let output = "x86_64\n/home/user\ntmux 3.3a\nMUX_SENTINEL_V1_END\n";
        let err = parse_preflight_output(output).unwrap_err();
        assert!(err.to_string().contains("missing start sentinel"), "got: {err}");
    }

    #[test]
    fn parse_preflight_output_no_end_sentinel_errors() {
        let output = "Welcome!\nMUX_SENTINEL_V1\nx86_64\n/home/user\ntmux 3.3a\n";
        let err = parse_preflight_output(output).unwrap_err();
        assert!(err.to_string().contains("missing end sentinel"), "got: {err}");
    }

    #[test]
    fn parse_preflight_output_too_few_inner_lines_errors() {
        // Only 2 lines between sentinels (need 3)
        let output = "MUX_SENTINEL_V1\nx86_64\n/home/user\nMUX_SENTINEL_V1_END\n";
        let err = parse_preflight_output(output).unwrap_err();
        assert!(err.to_string().contains("expected 3 lines"), "got: {err}");
    }

    /// Mismatch with no stdin → read_yes_no reads empty line → rotation declined.
    #[test]
    fn cmd_trust_mismatch_declined() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host(conn, "mismatchhost");
        // Store a DIFFERENT fingerprint so MockSshHost's SHA256:AAAA causes Mismatch.
        mux_state::fingerprint_repo::upsert(
            conn, host.id, "ssh-ed25519", "SHA256:DIFFERENT", 1_000_000,
        ).unwrap();
        // MockSshHost returns SHA256:AAAA → Mismatch.
        let ssh = MockSshHost::with_trusted_key(vec![]);
        // read_yes_no will read from stdin — in a non-interactive test stdin is empty,
        // so read_line returns "" → trimmed == "" → accepted = false → bail.
        let err = cmd_trust_core(conn, &host, &ssh).unwrap_err();
        assert!(
            err.to_string().contains("declined") || err.to_string().contains("rotation"),
            "expected decline error, got: {err}"
        );
        // Fingerprint should remain SHA256:DIFFERENT (not rotated).
        let fp = mux_state::fingerprint_repo::get(conn, host.id, "ssh-ed25519")
            .unwrap().unwrap();
        assert_eq!(fp.fingerprint, "SHA256:DIFFERENT", "fingerprint should not be updated on decline");
    }
}
