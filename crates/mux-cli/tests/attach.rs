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

    // MockSshHost::host_key would panic if called; run() errors. The dead-session
    // gate must fire before any SSH interaction.
    let err = prepare_attach(make_ctx(conn, MockSshHost::ed25519("FP"), uuid)).unwrap_err();
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
    assert!(
        inv.argv.contains(&"mux-myapp".to_owned()),
        "argv must contain tmux_name 'mux-myapp'; got: {:?}",
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
    assert!(
        inv.argv.contains(&"-t".to_owned()),
        "argv must contain -t for pseudo-TTY; got: {:?}",
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
    assert!(
        std::path::Path::new(path).exists(),
        "known_hosts file must exist on disk at {path}"
    );
}

// ── 10. IPv6 address gets brackets in SSH target ──────────────────────────────

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
