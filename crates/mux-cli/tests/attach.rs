//! Integration tests for mux attach — docs/04 §TOFU, docs/07 §Attach flow, docs/08

use mux_cli::agent_start::RemoteExec;
use mux_cli::attach::{prepare_attach, AttachContext, SshInvocation};
use mux_cli::create::{HostKeyInfo, SshHost};
use mux_core::error::MuxError;
use mux_state::session_repo::{activate, ReserveParams};
use mux_state::{host_repo, session_repo};
use mux_state::store::Store;
use tempfile::TempDir;

// ── MockSshHost ───────────────────────────────────────────────────────────────
// Integration crates cannot reach #[cfg(test)] items in the lib, so this
// duplicates the inline MockSshHost from src/attach.rs mod tests.

struct MockSshHost {
    host_key_result: Result<HostKeyInfo, MuxError>,
}

impl MockSshHost {
    fn with_key(algorithm: impl Into<String>, fingerprint: impl Into<String>) -> Self {
        MockSshHost {
            host_key_result: Ok(HostKeyInfo {
                algorithm: algorithm.into(),
                fingerprint: fingerprint.into(),
            }),
        }
    }

    fn ed25519(fingerprint: impl Into<String>) -> Self {
        Self::with_key("ssh-ed25519", fingerprint)
    }
}

impl RemoteExec for MockSshHost {
    fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
        Err(MuxError::Other(anyhow::anyhow!("attach does not call run()")))
    }
}

impl SshHost for MockSshHost {
    fn host_key(&self) -> Result<HostKeyInfo, MuxError> {
        match &self.host_key_result {
            Ok(k) => Ok(k.clone()),
            Err(e) => Err(MuxError::Other(anyhow::anyhow!("{e}"))),
        }
    }
}

// PanicOnProbe enforces that neither host_key() nor run() is called.
// Used to make the "gate fires first" invariant observable, not incidental.
struct PanicOnProbe;

impl RemoteExec for PanicOnProbe {
    fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
        panic!("attach must not call run() before the dead-session gate");
    }
}

impl SshHost for PanicOnProbe {
    fn host_key(&self) -> Result<HostKeyInfo, MuxError> {
        panic!("attach must not probe host_key() before the dead-session gate");
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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

fn insert_host_ipv6(conn: &rusqlite::Connection) -> i64 {
    let id = host_repo::insert(conn, "ipv6host", "user", "2001:db8::1", 22, 1_000_000).unwrap();
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

fn trust_ed25519(conn: &rusqlite::Connection, host_id: i64) {
    mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "FP").unwrap();
}

fn make_ctx<'a, S: SshHost>(
    conn: &'a rusqlite::Connection,
    ssh: S,
    selector: &str,
) -> AttachContext<'a, S> {
    AttachContext { conn, ssh, selector: selector.to_owned() }
}

fn find_opt(inv: &SshInvocation, prefix: &str) -> Option<String> {
    inv.argv.iter().find(|a| a.starts_with(prefix)).cloned()
}

// ── 1. Dead session → error before any SSH attempt ───────────────────────────

#[test]
fn dead_session_errors_before_ssh() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    insert_active_session(conn, host_id, uuid, "myapp");
    session_repo::set_status(conn, uuid, "dead", 1_000_002).unwrap();

    // PanicOnProbe panics if host_key() or run() is reached; the dead-session
    // gate at src/attach.rs:59 must fire before any SSH interaction.
    let err = prepare_attach(make_ctx(conn, PanicOnProbe, uuid)).unwrap_err();
    assert!(err.to_string().contains("dead"), "expected dead error, got: {err}");
}

// ── 2. UUID-format selector with no match → error, no shortname fallback ─────

#[test]
fn uuid_selector_no_match_errors() {
    let (_dir, store) = open_store();
    let conn = store.conn();

    let err = prepare_attach(make_ctx(
        conn,
        MockSshHost::ed25519("FP"),
        "99999999-9999-9999-9999-999999999999",
    ))
    .unwrap_err();
    assert!(err.to_string().contains("not found"), "expected not found error, got: {err}");
}

// ── 3. TOFU first contact → refuse ───────────────────────────────────────────

#[test]
fn first_contact_refused_on_attach() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    insert_active_session(conn, host_id, uuid, "myapp");
    // No fingerprint stored → first contact.

    let err = prepare_attach(make_ctx(conn, MockSshHost::ed25519("UNKNOWN_FP"), uuid)).unwrap_err();
    assert!(
        err.to_string().contains("unknown host key"),
        "expected unknown host key error, got: {err}"
    );
    assert!(
        err.to_string().contains("mux host test"),
        "error should hint at 'mux host test', got: {err}"
    );
}

// ── 4. TOFU mismatch → refuse ─────────────────────────────────────────────────

#[test]
fn tofu_mismatch_refused_on_attach() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    insert_active_session(conn, host_id, uuid, "myapp");
    mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "STORED_FP").unwrap();

    let err =
        prepare_attach(make_ctx(conn, MockSshHost::ed25519("DIFFERENT_FP"), uuid)).unwrap_err();
    assert!(err.to_string().contains("mismatch"), "expected mismatch error, got: {err}");
}

// ── 5. SSH argv uses StrictHostKeyChecking=accept-new (not "no") ──────────────

#[test]
fn ssh_argv_uses_accept_new_not_no() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "dddddddd-dddd-dddd-dddd-dddddddddddd";
    insert_active_session(conn, host_id, uuid, "myapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    assert!(
        inv.argv.contains(&"StrictHostKeyChecking=accept-new".to_owned()),
        "argv must contain StrictHostKeyChecking=accept-new; got: {:?}",
        inv.argv
    );
    assert!(
        !inv.argv.iter().any(|a| a == "StrictHostKeyChecking=no"),
        "argv must not contain StrictHostKeyChecking=no; got: {:?}",
        inv.argv
    );
}

// ── 6. SSH argv pins HostKeyAlgorithms to the verified algorithm ──────────────

#[test]
fn ssh_argv_pins_verified_host_key_algorithm() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
    insert_active_session(conn, host_id, uuid, "myapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    assert!(
        find_opt(&inv, "HostKeyAlgorithms=").as_deref() == Some("HostKeyAlgorithms=ssh-ed25519"),
        "HostKeyAlgorithms must pin ssh-ed25519; got: {:?}",
        inv.argv
    );
}

// ── 7. SSH argv uses stored tmux_name as the tmux session target ──────────────

#[test]
fn ssh_argv_uses_stored_tmux_name() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    insert_active_session(conn, host_id, uuid, "myapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    let tail = &inv.argv[inv.argv.len() - 4..];
    assert_eq!(
        tail,
        &["tmux", "attach-session", "-t", "mux-myapp"],
        "argv tail must be 'tmux attach-session -t mux-myapp'; got: {:?}",
        inv.argv
    );
}

// ── 8. SSH argv has -t for pseudo-TTY ────────────────────────────────────────

#[test]
fn ssh_argv_has_pseudo_tty_flag() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "11111111-1111-1111-1111-111111111111";
    insert_active_session(conn, host_id, uuid, "myapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    // argv contains -t twice: SSH pseudo-TTY flag and `tmux attach-session -t <name>`.
    // Assert the SSH -t appears *before* the user@host target to distinguish them.
    let target_idx = inv.argv.iter().position(|a| a.starts_with("user@")).unwrap();
    let ssh_t = inv.argv[..target_idx].iter().any(|a| a == "-t");
    assert!(
        ssh_t,
        "argv must contain SSH -t (pseudo-TTY) before the target; got: {:?}",
        inv.argv
    );
}

// ── 9. Known_hosts file exists (not /dev/null) ────────────────────────────────

#[test]
fn known_hosts_file_exists_on_disk() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "22222222-2222-2222-2222-222222222222";
    insert_active_session(conn, host_id, uuid, "myapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    let kh_arg = find_opt(&inv, "UserKnownHostsFile=").expect("UserKnownHostsFile must be present");
    let path = kh_arg.strip_prefix("UserKnownHostsFile=").unwrap();
    assert_ne!(path, "/dev/null", "known_hosts must not be /dev/null");
    let meta = std::fs::metadata(path).expect("known_hosts file must exist on disk");
    assert!(meta.is_file(), "known_hosts path must be a file");
    // File must be empty: mux stores only the fingerprint hash, not the raw key blob.
    // Writing key material would defeat the empty-known_hosts security model.
    assert_eq!(meta.len(), 0, "known_hosts must be empty (no raw key blob)");
}

// ── 10. IPv6 address gets brackets in SSH target ─────────────────────────────

#[test]
fn ipv6_address_bracketed_in_ssh_target() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_ipv6(conn);
    let uuid = "33333333-3333-3333-3333-333333333333";
    insert_active_session(conn, host_id, uuid, "myapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    assert!(
        inv.argv.contains(&"user@[2001:db8::1]".to_owned()),
        "IPv6 addr must be bracketed in target; got: {:?}",
        inv.argv
    );
    // IPv6 path must still emit all security options (no different argv branch).
    assert!(find_opt(&inv, "HostKeyAlgorithms=").is_some(), "HostKeyAlgorithms must be present on IPv6 path");
    assert!(find_opt(&inv, "UserKnownHostsFile=").is_some(), "UserKnownHostsFile must be present on IPv6 path");
}

// ── 11. RSA pins both SHA-2 variants in correct order ────────────────────────

#[test]
fn rsa_algorithm_pins_both_sha2_variants_ordered() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "44444444-4444-4444-4444-444444444444";
    insert_active_session(conn, host_id, uuid, "rsaapp");
    mux_ssh::trust::trust_fingerprint(conn, host_id, "rsa-sha2-512", "RSAFP").unwrap();

    let inv =
        prepare_attach(make_ctx(conn, MockSshHost::with_key("rsa-sha2-512", "RSAFP"), uuid))
            .unwrap();
    assert!(
        inv.argv.contains(&"HostKeyAlgorithms=rsa-sha2-512,rsa-sha2-256".to_owned()),
        "RSA must pin rsa-sha2-512,rsa-sha2-256 (512 first); got: {:?}",
        inv.argv
    );
}

// ── 12. Unknown algorithm passes through verbatim ─────────────────────────────

#[test]
fn unknown_algorithm_passes_through_verbatim() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "55555555-5555-5555-5555-555555555555";
    insert_active_session(conn, host_id, uuid, "customapp");
    mux_ssh::trust::trust_fingerprint(conn, host_id, "x-custom-algo", "CUSTOMFP").unwrap();

    let inv = prepare_attach(make_ctx(
        conn,
        MockSshHost::with_key("x-custom-algo", "CUSTOMFP"),
        uuid,
    ))
    .unwrap();
    assert!(
        inv.argv.contains(&"HostKeyAlgorithms=x-custom-algo".to_owned()),
        "unknown algorithm must pass through verbatim; got: {:?}",
        inv.argv
    );
}

// ── 13. ECDSA algorithm pins single variant ───────────────────────────────────

#[test]
fn ecdsa_algorithm_pins_single_variant() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "66666666-6666-6666-6666-666666666666";
    insert_active_session(conn, host_id, uuid, "ecdsaapp");
    mux_ssh::trust::trust_fingerprint(conn, host_id, "ecdsa-sha2-nistp256", "ECDSAFP").unwrap();

    let inv = prepare_attach(make_ctx(
        conn,
        MockSshHost::with_key("ecdsa-sha2-nistp256", "ECDSAFP"),
        uuid,
    ))
    .unwrap();
    assert!(
        inv.argv.contains(&"HostKeyAlgorithms=ecdsa-sha2-nistp256".to_owned()),
        "ECDSA must pin ecdsa-sha2-nistp256; got: {:?}",
        inv.argv
    );
}

// ── 14. Port appears in argv ──────────────────────────────────────────────────

#[test]
fn ssh_argv_contains_port() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "77777777-7777-7777-7777-777777777777";
    insert_active_session(conn, host_id, uuid, "portapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap();
    let port_idx = inv.argv.iter().position(|a| a == "-p").expect("-p must be present");
    assert_eq!(inv.argv[port_idx + 1], "22", "port must be 22; got: {:?}", inv.argv);
}

// ── 15. Shortname selector resolves ──────────────────────────────────────────

#[test]
fn shortname_selector_resolves_to_session() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "88888888-8888-8888-8888-888888888888";
    insert_active_session(conn, host_id, uuid, "shortapp");
    trust_ed25519(conn, host_id);

    let inv = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), "shortapp")).unwrap();
    let tail = &inv.argv[inv.argv.len() - 4..];
    assert_eq!(tail, &["tmux", "attach-session", "-t", "mux-shortapp"]);
}

// ── 16. Reserved-but-not-active session errors with "no tmux name" ───────────

#[test]
fn reserved_session_without_tmux_name_errors() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "99999999-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    // Reserve but do NOT activate — tmux_name stays None.
    session_repo::reserve(
        conn,
        &ReserveParams {
            uuid,
            host_id,
            shortname: "inflightapp",
            repo_slug: "owner/repo",
            branch: "main",
            created_at: 1_000_000,
        },
    )
    .unwrap();
    trust_ed25519(conn, host_id);

    let err = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap_err();
    assert!(
        err.to_string().contains("no tmux name"),
        "expected 'no tmux name' error for reserved session, got: {err}"
    );
}
