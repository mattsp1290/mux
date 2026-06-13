//! Spec: docs/01 §mux kill, docs/07 §Kill flow

use anyhow::{bail, Result};
use rusqlite::Connection;

use mux_core::error::MuxError;
use mux_rpc::client::RpcClient;
use mux_rpc::schema::KillSessionRequest;
use mux_ssh::trust::{check_host_key, trust_fingerprint, TrustCheckResult};
use mux_state::{host_repo, session_repo};
use mux_state::model::Session;

use crate::agent_start::AgentStarter;
use crate::create::SshHost;

// ── Public API ────────────────────────────────────────────────────────────────

/// Execution context for `mux kill`.
pub struct KillContext<'a, S: SshHost> {
    pub conn: &'a Connection,
    /// SSH executor targeting the session's host.
    pub ssh: S,
    /// UUID or shortname of the session to kill.
    pub selector: String,
    /// Whether a TTY is attached (allows interactive TOFU prompts).
    pub is_interactive: bool,
}

/// Kill a session.
///
/// Implements the kill flow from docs/07:
/// 1. Resolve selector (UUID first, then shortname).
/// 2. Gate on local status: dead → silent no-op; unreachable → error.
/// 3. TOFU host-key verification (no mutation on mismatch).
/// 4. Connect to the running agent.
/// 5. Send `KillSession { uuid, repo_slug }`.
/// 6. Mark session dead only if the agent reports `tmux_killed OR workdir_removed`.
///
/// NOTE: step 4 uses synchronous SSH calls inside an async fn. A real production
/// implementation must wrap them in `tokio::task::spawn_blocking`.
pub async fn run_kill<S: SshHost>(ctx: KillContext<'_, S>) -> Result<()> {
    // Step 1 — resolve selector
    let session = resolve_session(ctx.conn, &ctx.selector)?;

    // Step 2 — gate on local status
    match session.status.as_str() {
        "dead" => {
            println!("mux: session already dead");
            return Ok(());
        }
        "unreachable" => {
            bail!("mux: host unreachable; verify connectivity and retry");
        }
        _ => {} // "active" | "orphaned" → continue
    }

    // Step 3 — TOFU host-key verification
    let host = host_repo::get_by_id(ctx.conn, session.host_id)?
        .ok_or_else(|| anyhow::anyhow!("host not found for session (host_id={})", session.host_id))?;

    let key_info = ctx.ssh.host_key()?;
    let tofu = check_host_key(ctx.conn, host.id, &key_info.algorithm, &key_info.fingerprint)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    match tofu {
        TrustCheckResult::Trusted => {}
        TrustCheckResult::Mismatch { algorithm, stored, received } => {
            bail!(
                "host key mismatch for '{}': {} fingerprint stored={} received={}",
                host.alias, algorithm, stored, received
            );
        }
        TrustCheckResult::FirstContact { algorithm, fingerprint } => {
            if !ctx.is_interactive {
                bail!(
                    "host '{}' not yet trusted (non-interactive); run `mux host trust {}` first",
                    host.alias, host.alias
                );
            }
            eprintln!("New host '{}' ({}:{}):", host.alias, host.addr, host.port);
            eprintln!("  Algorithm:   {}", algorithm);
            eprintln!("  Fingerprint: {}", fingerprint);
            eprint!("Trust this host and continue? (yes/no): ");
            if !read_yes_no()? {
                bail!("host key not trusted; operation aborted");
            }
            trust_fingerprint(ctx.conn, host.id, &algorithm, &fingerprint)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }

    // Step 4 — connect to running agent
    // TODO: establish SSH port-forward before connecting; use session.transport_mode
    // to select streamlocal vs TCP channel (docs/04 §Transport probing).
    let home = host.home.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "host '{}' has no home dir; run `mux host test {}` first",
            host.alias, host.alias
        )
    })?;
    let starter = AgentStarter::new(home, &ctx.ssh);
    let agent_urls = starter.ensure_running().map_err(|e| anyhow::anyhow!("{e}"))?;

    // Step 5 — send KillSession RPC
    let rpc = RpcClient::tcp("127.0.0.1", agent_urls.tcp_port());
    let resp = rpc
        .kill_session(KillSessionRequest {
            uuid: session.uuid.clone(),
            repo_slug: session.repo_slug.clone(),
        })
        .await;

    // Step 6 — handle response and conditionally mark dead
    match resp {
        Err(MuxError::AgentError(ref msg)) if msg.starts_with("not_owned") => {
            bail!("mux: session not owned by this client");
        }
        Err(e) => return Err(anyhow::anyhow!("{e}")),
        Ok(kill_resp) => {
            if kill_resp.tmux_killed || kill_resp.workdir_removed {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                session_repo::set_status(ctx.conn, &session.uuid, "dead", now)?;
            }
            // else: no effect reported → leave local state unchanged (no-op per spec)
        }
    }

    Ok(())
}

// ── Selector resolution ───────────────────────────────────────────────────────

/// Resolve a kill selector to a Session.
///
/// UUID-format selectors that don't exist return a hard error (no shortname
/// fallback). Non-UUID selectors are treated as shortnames; ambiguous shortnames
/// (same name on multiple hosts) return an error directing the user to use a UUID.
pub fn resolve_session(conn: &Connection, selector: &str) -> Result<Session> {
    if uuid::Uuid::parse_str(selector).is_ok() {
        return session_repo::get_by_uuid(conn, selector)?
            .ok_or_else(|| anyhow::anyhow!("session not found: {}", selector));
    }
    // Not UUID-format → shortname lookup
    let matches = session_repo::get_by_shortname_global(conn, selector)?;
    match matches.len() {
        0 => bail!("session not found: {}", selector),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => bail!(
            "ambiguous shortname '{}': found {} sessions; use a UUID to disambiguate",
            selector,
            matches.len()
        ),
    }
}

// ── I/O helper ───────────────────────────────────────────────────────────────

fn read_yes_no() -> Result<bool> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    Ok(trimmed == "yes" || trimmed == "y")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mux_state::store::Store;
    use mux_state::session_repo::{ReserveParams, activate};
    use mux_state::host_repo;
    use tempfile::TempDir;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::create::{HostKeyInfo, SshHost};
    use crate::agent_start::RemoteExec;

    // ── MockSshHost ───────────────────────────────────────────────────────────

    struct MockSshHost {
        responses: RefCell<VecDeque<(i32, String, String)>>,
        host_key_result: Result<HostKeyInfo, MuxError>,
    }

    impl MockSshHost {
        fn with_key(
            fingerprint: impl Into<String>,
            responses: Vec<(i32, String, String)>,
        ) -> Self {
            MockSshHost {
                responses: RefCell::new(responses.into()),
                host_key_result: Ok(HostKeyInfo {
                    algorithm: "ssh-ed25519".to_owned(),
                    fingerprint: fingerprint.into(),
                }),
            }
        }

        fn with_key_err(e: MuxError) -> Self {
            MockSshHost {
                responses: RefCell::new(VecDeque::new()),
                host_key_result: Err(e),
            }
        }
    }

    impl RemoteExec for MockSshHost {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| MuxError::Other(anyhow::anyhow!("no more responses")))
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

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store = Store::open(&db_path).unwrap();
        (dir, store)
    }

    fn insert_host_with_home(conn: &Connection) -> i64 {
        let id = host_repo::insert(conn, "myhost", "user", "192.0.2.1", 22, 1_000_000).unwrap();
        host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user"), Some("tcp")).unwrap();
        id
    }

    fn insert_active_session(conn: &Connection, host_id: i64, uuid: &str, shortname: &str) {
        session_repo::reserve(conn, &ReserveParams {
            uuid,
            host_id,
            shortname,
            repo_slug: "owner/repo",
            branch: "main",
            created_at: 1_000_000,
        }).unwrap();
        activate(conn, uuid, "mux-myapp", "/home/user/repo", "tcp", 1_000_001).unwrap();
    }

    fn trust_host_key(conn: &Connection, host_id: i64) {
        mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "FINGERPRINT").unwrap();
    }

    // ── resolve_session tests ─────────────────────────────────────────────────

    #[test]
    fn resolve_by_uuid_found() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host_with_home(conn);
        insert_active_session(conn, host_id, "11111111-1111-1111-1111-111111111111", "myapp");
        let s = resolve_session(conn, "11111111-1111-1111-1111-111111111111").unwrap();
        assert_eq!(s.uuid, "11111111-1111-1111-1111-111111111111");
    }

    #[test]
    fn resolve_by_uuid_not_found_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = resolve_session(conn, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn resolve_by_shortname_found() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host_with_home(conn);
        insert_active_session(conn, host_id, "22222222-2222-2222-2222-222222222222", "myapp");
        let s = resolve_session(conn, "myapp").unwrap();
        assert_eq!(s.shortname, "myapp");
    }

    #[test]
    fn resolve_by_shortname_not_found_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = resolve_session(conn, "no-such-session").unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn resolve_ambiguous_shortname_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        // Two different hosts, same shortname
        let h1 = host_repo::insert(conn, "host1", "u", "1.1.1.1", 22, 1_000_000).unwrap();
        let h2 = host_repo::insert(conn, "host2", "u", "2.2.2.2", 22, 1_000_000).unwrap();
        insert_active_session(conn, h1, "33333333-3333-3333-3333-333333333333", "shared");
        insert_active_session(conn, h2, "44444444-4444-4444-4444-444444444444", "shared");
        let err = resolve_session(conn, "shared").unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "{err}");
    }

    // ── run_kill tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn kill_already_dead_is_noop() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host_with_home(conn);
        let uuid = "55555555-5555-5555-5555-555555555555";
        insert_active_session(conn, host_id, uuid, "deadapp");
        session_repo::set_status(conn, uuid, "dead", 2_000_000).unwrap();

        let ssh = MockSshHost::with_key("FINGERPRINT", vec![]);
        let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };
        run_kill(ctx).await.unwrap();
        // Status unchanged (stays dead)
        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "dead");
    }

    #[tokio::test]
    async fn kill_unreachable_session_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host_with_home(conn);
        let uuid = "66666666-6666-6666-6666-666666666666";
        insert_active_session(conn, host_id, uuid, "unreachapp");
        session_repo::set_status(conn, uuid, "unreachable", 2_000_000).unwrap();

        let ssh = MockSshHost::with_key("FINGERPRINT", vec![]);
        let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };
        let err = run_kill(ctx).await.unwrap_err();
        assert!(err.to_string().contains("unreachable"), "{err}");
    }

    #[tokio::test]
    async fn kill_tofu_mismatch_refuses_mutation() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host_with_home(conn);
        let uuid = "77777777-7777-7777-7777-777777777777";
        insert_active_session(conn, host_id, uuid, "mismatchapp");
        // Trust a different fingerprint
        mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "STORED_FP").unwrap();

        // SSH mock returns a different fingerprint
        let ssh = MockSshHost::with_key("RECEIVED_FP", vec![]);
        let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };
        let err = run_kill(ctx).await.unwrap_err();
        assert!(err.to_string().contains("mismatch"), "{err}");
        // Session still active (no mutation)
        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "active");
    }

    #[tokio::test]
    async fn kill_first_contact_non_interactive_refuses() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host_with_home(conn);
        let uuid = "88888888-8888-8888-8888-888888888888";
        insert_active_session(conn, host_id, uuid, "newapp");
        // No stored fingerprint (first contact)

        let ssh = MockSshHost::with_key("SOME_FP", vec![]);
        let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };
        let err = run_kill(ctx).await.unwrap_err();
        assert!(err.to_string().contains("not yet trusted") || err.to_string().contains("non-interactive"), "{err}");
        // Session still active
        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "active");
    }

    // NOTE: run_kill steps 4-6 (agent connection + RPC) require a live SSH host and
    // running mux-agent, so end-to-end kill flow tests live in the integration suite
    // (tests/integration/ — see mux-4ku). The TOFU gate and selector resolution above
    // are fully unit-testable without SSH.
}
