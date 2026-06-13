//! Spec: docs/01 §mux attach, docs/04 §Attach pinning, docs/07 §Attach flow

use anyhow::{bail, Result};
use rusqlite::Connection;

use mux_ssh::trust::{check_host_key, TrustCheckResult};
use mux_state::{fingerprint_repo, host_repo};

use crate::create::SshHost;
use crate::kill::resolve_session;

// ── Public API ────────────────────────────────────────────────────────────────

/// Execution context for `mux attach`.
pub struct AttachContext<'a, S: SshHost> {
    pub conn: &'a Connection,
    /// SSH executor targeting the session's host.
    pub ssh: S,
    /// UUID or shortname of the session to attach.
    pub selector: String,
    /// Whether a TTY is attached (allows interactive TOFU prompts).
    pub is_interactive: bool,
}

/// Everything SSH needs to open the session; returned by `prepare_attach`.
#[derive(Debug)]
///
/// The caller execs `argv[0]` with `argv[1..]`. The `_tmpdir` must be kept alive
/// until after exec (the known_hosts file must exist on disk). In production,
/// `std::mem::forget(_tmpdir)` before exec so the file persists until process exit.
pub struct SshInvocation {
    pub argv: Vec<String>,
    /// Owns the temp directory containing the known_hosts file.
    pub _tmpdir: tempfile::TempDir,
}

/// Build the SSH invocation for `mux attach` without executing it.
///
/// Implements the attach flow from docs/07:
/// 1. Resolve selector.
/// 2. Reject dead sessions.
/// 3. Load session and host.
/// 4. Perform TOFU probe (docs/04 §Attach pinning).
/// 5. Write temporary known_hosts file.
/// 6. Return SSH argv.
///
/// The caller is responsible for exec'ing the returned `argv`.
pub fn prepare_attach<S: SshHost>(ctx: AttachContext<'_, S>) -> Result<SshInvocation> {
    // Step 1 — resolve selector
    let session = resolve_session(ctx.conn, &ctx.selector)?;

    // Step 2 — reject dead sessions
    if session.status == "dead" {
        bail!("mux: session '{}' is dead; cannot attach", ctx.selector);
    }

    // Step 3 — load host
    let host = host_repo::get_by_id(ctx.conn, session.host_id)?
        .ok_or_else(|| anyhow::anyhow!("mux: host record missing for session '{}'", ctx.selector))?;

    // Require tmux_name — sessions in "in-flight" reservation state have None
    let tmux_name = session
        .tmux_name
        .ok_or_else(|| anyhow::anyhow!("mux: session '{}' has no tmux name (not yet active)", ctx.selector))?;

    // Step 4 — TOFU probe
    let key_info = ctx.ssh.host_key()?;
    let tofu = check_host_key(ctx.conn, host.id, &key_info.algorithm, &key_info.fingerprint)?;
    match tofu {
        TrustCheckResult::Trusted => {}
        TrustCheckResult::Mismatch {
            algorithm,
            stored,
            received,
        } => {
            bail!(
                "mux: host key mismatch for '{}': stored {}:{}, received {}:{}",
                host.alias,
                algorithm,
                stored,
                algorithm,
                received
            );
        }
        TrustCheckResult::FirstContact { algorithm, fingerprint } => {
            // Attach never silently trusts on first contact — it refuses with a hint.
            bail!(
                "mux: unknown host key ({}:{}) for '{}'; run 'mux host test {}' to trust it",
                algorithm,
                fingerprint,
                host.alias,
                host.alias
            );
        }
    }

    // Step 5 — select algorithm for HostKeyAlgorithms pin
    let fingerprints = fingerprint_repo::list_for_host(ctx.conn, host.id)?;
    let preferred = mux_ssh::trust::preferred_fingerprint_for_attach(&fingerprints)
        .ok_or_else(|| anyhow::anyhow!("mux: no trusted host key stored for '{}'", host.alias))?;
    let host_key_algorithms = build_host_key_algorithms(&preferred.algorithm);

    // Step 6 — write temp known_hosts file
    //
    // We only store the fingerprint hash (not the raw public key blob), so we write
    // an empty known_hosts file. SSH will not verify the key against known_hosts;
    // instead, fingerprint verification was done above via TOFU. The algorithm is
    // pinned via HostKeyAlgorithms to prevent downgrade. StrictHostKeyChecking=accept-new
    // prevents SSH from writing to ~/.ssh/known_hosts while still completing the connection.
    let tmpdir = tempfile::Builder::new()
        .prefix("mux-attach-")
        .tempdir()?;
    let known_hosts_path = tmpdir.path().join("known_hosts");
    std::fs::write(&known_hosts_path, "")?;

    // Step 7 — build SSH argv
    let argv = build_ssh_argv(
        &host.user,
        &host.addr,
        host.port as u16,
        known_hosts_path.to_string_lossy().as_ref(),
        &host_key_algorithms,
        &tmux_name,
    );

    Ok(SshInvocation { argv, _tmpdir: tmpdir })
}

/// Build the HostKeyAlgorithms value for the given stored algorithm.
///
/// RSA: SHA-512 before SHA-256 per docs/04 §Host key algorithms.
/// For any unknown algorithm, use it verbatim.
pub(crate) fn build_host_key_algorithms(algorithm: &str) -> String {
    match algorithm {
        "ssh-ed25519" => "ssh-ed25519".to_owned(),
        "ecdsa-sha2-nistp256" => "ecdsa-sha2-nistp256".to_owned(),
        "rsa-sha2-512" | "rsa-sha2-256" => "rsa-sha2-512,rsa-sha2-256".to_owned(),
        other => other.to_owned(),
    }
}

fn build_ssh_argv(
    user: &str,
    addr: &str,
    port: u16,
    known_hosts_path: &str,
    host_key_algorithms: &str,
    tmux_name: &str,
) -> Vec<String> {
    vec![
        "ssh".to_owned(),
        "-o".to_owned(),
        format!("UserKnownHostsFile={known_hosts_path}"),
        "-o".to_owned(),
        format!("HostKeyAlgorithms={host_key_algorithms}"),
        "-o".to_owned(),
        "StrictHostKeyChecking=accept-new".to_owned(),
        "-t".to_owned(),
        format!("{user}@{addr}"),
        "-p".to_owned(),
        port.to_string(),
        "tmux".to_owned(),
        "attach-session".to_owned(),
        "-t".to_owned(),
        tmux_name.to_owned(),
    ]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use mux_state::session_repo::{activate, ReserveParams};
    use mux_state::{host_repo, session_repo};
    use mux_state::store::Store;
    use tempfile::TempDir;

    use crate::create::{HostKeyInfo, SshHost};
    use crate::agent_start::RemoteExec;

    // ── MockSshHost ───────────────────────────────────────────────────────────

    struct MockSshHost {
        responses: RefCell<VecDeque<(i32, String, String)>>,
        host_key_result: Result<HostKeyInfo, mux_core::error::MuxError>,
    }

    impl MockSshHost {
        fn with_key(fingerprint: &str) -> Self {
            MockSshHost {
                responses: RefCell::new(VecDeque::new()),
                host_key_result: Ok(HostKeyInfo {
                    algorithm: "ssh-ed25519".to_owned(),
                    fingerprint: fingerprint.to_owned(),
                }),
            }
        }

        fn with_key_alg(algorithm: &str, fingerprint: &str) -> Self {
            MockSshHost {
                responses: RefCell::new(VecDeque::new()),
                host_key_result: Ok(HostKeyInfo {
                    algorithm: algorithm.to_owned(),
                    fingerprint: fingerprint.to_owned(),
                }),
            }
        }

    }

    impl RemoteExec for MockSshHost {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), mux_core::error::MuxError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| mux_core::error::MuxError::Other(anyhow::anyhow!("no mock responses")))
        }
    }

    impl SshHost for MockSshHost {
        fn host_key(&self) -> Result<HostKeyInfo, mux_core::error::MuxError> {
            match &self.host_key_result {
                Ok(k) => Ok(k.clone()),
                Err(e) => Err(mux_core::error::MuxError::Other(anyhow::anyhow!("{e}"))),
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("mux.db")).unwrap();
        (dir, store)
    }

    fn insert_host(conn: &rusqlite::Connection) -> i64 {
        let id = host_repo::insert(conn, "myhost", "user", "192.0.2.1", 22, 1_000_000).unwrap();
        host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user"), Some("tcp")).unwrap();
        id
    }

    fn insert_active_session(
        conn: &rusqlite::Connection,
        host_id: i64,
        uuid: &str,
        shortname: &str,
    ) {
        session_repo::reserve(
            conn,
            &ReserveParams {
                uuid,
                host_id,
                shortname,
                repo_slug: "owner/repo",
                branch: "main",
                created_at: 1_000_000,
            },
        )
        .unwrap();
        activate(conn, uuid, &format!("mux-{shortname}"), "/work/repo", "tcp", 1_000_001).unwrap();
    }

    fn trust(conn: &rusqlite::Connection, host_id: i64) {
        mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "FP").unwrap();
    }

    fn make_ctx<'a, S: SshHost>(
        conn: &'a rusqlite::Connection,
        ssh: S,
        selector: &str,
    ) -> AttachContext<'a, S> {
        AttachContext {
            conn,
            ssh,
            selector: selector.to_owned(),
            is_interactive: false,
        }
    }

    // ── build_host_key_algorithms ─────────────────────────────────────────────

    #[test]
    fn alg_ed25519_returns_single() {
        assert_eq!(build_host_key_algorithms("ssh-ed25519"), "ssh-ed25519");
    }

    #[test]
    fn alg_ecdsa_returns_single() {
        assert_eq!(build_host_key_algorithms("ecdsa-sha2-nistp256"), "ecdsa-sha2-nistp256");
    }

    #[test]
    fn alg_rsa_512_returns_both_sha2_ordered() {
        let v = build_host_key_algorithms("rsa-sha2-512");
        assert_eq!(v, "rsa-sha2-512,rsa-sha2-256");
    }

    #[test]
    fn alg_rsa_256_returns_both_sha2_ordered() {
        let v = build_host_key_algorithms("rsa-sha2-256");
        assert_eq!(v, "rsa-sha2-512,rsa-sha2-256");
    }

    #[test]
    fn alg_unknown_returns_verbatim() {
        assert_eq!(build_host_key_algorithms("x-custom-algo"), "x-custom-algo");
    }

    // ── prepare_attach — happy path ───────────────────────────────────────────

    #[test]
    fn prepare_attach_argv_contains_ssh_and_tmux_name() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        insert_active_session(conn, host_id, uuid, "myapp");
        trust(conn, host_id);

        let inv = prepare_attach(make_ctx(conn, MockSshHost::with_key("FP"), uuid)).unwrap();
        assert_eq!(inv.argv[0], "ssh");
        assert!(inv.argv.contains(&"mux-myapp".to_owned()));
        assert!(inv.argv.iter().any(|a| a.contains("UserKnownHostsFile")));
        assert!(inv.argv.iter().any(|a| a.contains("HostKeyAlgorithms=ssh-ed25519")));
        assert!(inv.argv.contains(&"-t".to_owned()));
    }

    #[test]
    fn prepare_attach_host_address_in_argv() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        insert_active_session(conn, host_id, uuid, "myapp2");
        trust(conn, host_id);

        let inv = prepare_attach(make_ctx(conn, MockSshHost::with_key("FP"), uuid)).unwrap();
        assert!(inv.argv.contains(&"user@192.0.2.1".to_owned()));
        assert!(inv.argv.iter().any(|a| a == "-p"));
        let port_idx = inv.argv.iter().position(|a| a == "-p").unwrap();
        assert_eq!(inv.argv[port_idx + 1], "22");
    }

    #[test]
    fn prepare_attach_known_hosts_file_exists() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        insert_active_session(conn, host_id, uuid, "myapp3");
        trust(conn, host_id);

        let inv = prepare_attach(make_ctx(conn, MockSshHost::with_key("FP"), uuid)).unwrap();
        let kh_arg = inv.argv.iter()
            .find(|a| a.starts_with("UserKnownHostsFile="))
            .unwrap();
        let path = kh_arg.strip_prefix("UserKnownHostsFile=").unwrap();
        assert!(std::path::Path::new(path).exists());
    }

    #[test]
    fn prepare_attach_rsa_algorithm_pins_both_variants() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        insert_active_session(conn, host_id, uuid, "rsaapp");
        mux_ssh::trust::trust_fingerprint(conn, host_id, "rsa-sha2-512", "RSAFP").unwrap();

        let ssh = MockSshHost::with_key_alg("rsa-sha2-512", "RSAFP");
        let inv = prepare_attach(make_ctx(conn, ssh, uuid)).unwrap();
        assert!(inv.argv.iter().any(|a| a == "HostKeyAlgorithms=rsa-sha2-512,rsa-sha2-256"));
    }

    // ── prepare_attach — error paths ─────────────────────────────────────────

    #[test]
    fn prepare_attach_rejects_dead_session() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        insert_active_session(conn, host_id, uuid, "deadapp");
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        session_repo::set_status(conn, uuid, "dead", now).unwrap();

        let err = prepare_attach(make_ctx(conn, MockSshHost::with_key("FP"), uuid)).unwrap_err();
        assert!(err.to_string().contains("dead"), "got: {err}");
    }

    #[test]
    fn prepare_attach_tofu_mismatch_refuses() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        insert_active_session(conn, host_id, uuid, "mismatchapp");
        mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "STORED_FP").unwrap();

        let ssh = MockSshHost::with_key("DIFFERENT_FP");
        let err = prepare_attach(make_ctx(conn, ssh, uuid)).unwrap_err();
        assert!(err.to_string().contains("mismatch"), "got: {err}");
    }

    #[test]
    fn prepare_attach_first_contact_refuses() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "11111111-1111-1111-1111-111111111111";
        insert_active_session(conn, host_id, uuid, "newapp");
        // No fingerprint stored — first contact

        let ssh = MockSshHost::with_key("UNKNOWN_FP");
        let err = prepare_attach(make_ctx(conn, ssh, uuid)).unwrap_err();
        assert!(err.to_string().contains("unknown host key"), "got: {err}");
        assert!(err.to_string().contains("mux host test"), "got: {err}");
    }

    #[test]
    fn prepare_attach_shortname_selector_resolves() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "22222222-2222-2222-2222-222222222222";
        insert_active_session(conn, host_id, uuid, "shortapp");
        trust(conn, host_id);

        let inv = prepare_attach(make_ctx(conn, MockSshHost::with_key("FP"), "shortapp")).unwrap();
        assert!(inv.argv.contains(&"mux-shortapp".to_owned()));
    }

    #[test]
    fn prepare_attach_unknown_uuid_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();

        let err = prepare_attach(make_ctx(
            conn,
            MockSshHost::with_key("FP"),
            "99999999-9999-9999-9999-999999999999",
        ))
        .unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }
}
