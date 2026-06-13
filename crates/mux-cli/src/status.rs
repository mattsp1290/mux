//! Spec: docs/01 §mux status, docs/07 §Status flow

use anyhow::Result;
use rusqlite::Connection;

use mux_core::error::MuxError;
use mux_rpc::client::RpcClient;
use mux_rpc::schema::{GetSessionRequest, SessionStatusValue};
use mux_state::host_repo;
use mux_state::model::Session;

use crate::agent_start::{AgentStarter, RemoteExec};
use crate::kill::resolve_session;

// ── Public API ────────────────────────────────────────────────────────────────

/// Execution context for `mux status`.
pub struct StatusContext<'a, E: RemoteExec> {
    pub conn: &'a Connection,
    /// Shell executor on the session's remote host (used to probe agent lock).
    pub ssh: E,
    /// UUID or shortname of the session.
    pub selector: String,
}

/// Show session status.
///
/// Implements the status flow from docs/07:
/// 1. Resolve selector (UUID first; UUID format not found → hard error).
/// 2. Load session and host from SQLite.
/// 3. Attempt `GetSession` RPC via the running agent.
///    - Success: display live data.
///    - Agent not running or host unreachable: display local SQLite data, note it.
///    - Other RPC error: surface it.
/// 4. No mutation of session status.
///
/// No TOFU host-key check — status is a read-only, best-effort refresh.
pub async fn run_status<E: RemoteExec>(ctx: StatusContext<'_, E>) -> Result<()> {
    // Step 1 — resolve selector
    let session = resolve_session(ctx.conn, &ctx.selector)?;

    // Step 2 — load host
    let host = host_repo::get_by_id(ctx.conn, session.host_id)?
        .ok_or_else(|| anyhow::anyhow!("mux: host record missing for session '{}'", ctx.selector))?;

    // Step 3 — attempt live GetSession RPC (no TOFU; read-only probe)
    let home = host.home.as_deref().unwrap_or("/tmp");
    let starter = AgentStarter::new(home, ctx.ssh);
    let live_data = match starter.probe_existing() {
        Ok(Some(agent_urls)) => {
            let rpc = RpcClient::tcp("127.0.0.1", agent_urls.tcp_port());
            match rpc.get_session(GetSessionRequest { uuid: session.uuid.clone() }).await {
                Ok(resp) => Some(resp),
                Err(MuxError::AgentError(ref msg)) if msg.starts_with("not_found") => {
                    // Session unknown to agent — fall through to local display
                    None
                }
                Err(e) => return Err(anyhow::anyhow!("{e}")),
            }
        }
        Ok(None) | Err(_) => {
            // No agent running or SSH probe failed — fall back to local data
            None
        }
    };

    // Step 4 — display
    if let Some(live) = live_data {
        print_session_live(&session, &host.alias, status_to_str(&live.status), &live.tmux_name);
    } else {
        print_session_local(&session, &host.alias);
    }

    Ok(())
}

fn status_to_str(s: &SessionStatusValue) -> &'static str {
    match s {
        SessionStatusValue::Active => "active",
        SessionStatusValue::Dead => "dead",
        SessionStatusValue::Unreachable => "unreachable",
        SessionStatusValue::Orphaned => "orphaned",
    }
}

fn print_session_live(session: &Session, host_alias: &str, live_status: &str, live_tmux: &str) {
    println!("uuid:      {}", session.uuid);
    println!("shortname: {}", session.shortname);
    println!("host:      {}", host_alias);
    println!("status:    {}", live_status);
    println!("tmux:      {}", live_tmux);
    if let Some(ref workdir) = session.workdir {
        println!("workdir:   {}", workdir);
    }
    println!("branch:    {}", session.branch);
    println!("repo:      {}", session.repo_slug);
}

fn print_session_local(session: &Session, host_alias: &str) {
    println!("uuid:      {}", session.uuid);
    println!("shortname: {}", session.shortname);
    println!("host:      {}", host_alias);
    println!("status:    {} (local)", session.status);
    if let Some(ref tmux) = session.tmux_name {
        println!("tmux:      {}", tmux);
    }
    if let Some(ref workdir) = session.workdir {
        println!("workdir:   {}", workdir);
    }
    println!("branch:    {}", session.branch);
    println!("repo:      {}", session.repo_slug);
    println!("note:      using local data (host unreachable or agent not running)");
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

    use crate::agent_start::RemoteExec;

    // ── MockRemoteExec ────────────────────────────────────────────────────────

    struct MockRemoteExec {
        responses: RefCell<VecDeque<(i32, String, String)>>,
    }

    impl MockRemoteExec {
        fn new(responses: Vec<(i32, String, String)>) -> Self {
            MockRemoteExec {
                responses: RefCell::new(responses.into()),
            }
        }

        fn unreachable() -> Self {
            // Returns error immediately — simulates SSH connection failure
            MockRemoteExec {
                responses: RefCell::new(VecDeque::new()),
            }
        }
    }

    impl RemoteExec for MockRemoteExec {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| MuxError::Other(anyhow::anyhow!("SSH unreachable (mock)")))
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

    fn make_ctx<'a, E: RemoteExec>(
        conn: &'a rusqlite::Connection,
        ssh: E,
        selector: &str,
    ) -> StatusContext<'a, E> {
        StatusContext { conn, ssh, selector: selector.to_owned() }
    }

    // ── resolve_session paths (reused from kill; tested here at status level) ──

    #[tokio::test]
    async fn status_unknown_uuid_returns_error() {
        let (_dir, store) = open_store();
        let conn = store.conn();

        let err = run_status(make_ctx(
            conn,
            MockRemoteExec::unreachable(),
            "99999999-9999-9999-9999-999999999999",
        ))
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn status_shortname_not_found_returns_error() {
        let (_dir, store) = open_store();
        let conn = store.conn();

        let err = run_status(make_ctx(conn, MockRemoteExec::unreachable(), "nosuchapp"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    // ── unreachable host fallback ─────────────────────────────────────────────

    #[tokio::test]
    async fn status_unreachable_host_displays_local_data() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        insert_active_session(conn, host_id, uuid, "myapp");

        // SSH probe errors → unreachable path
        run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
            .await
            .unwrap();
        // No panic = success; local data was displayed
    }

    #[tokio::test]
    async fn status_no_agent_lock_falls_back_to_local() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        insert_active_session(conn, host_id, uuid, "myapp2");

        // read_lock returns empty (no agent running)
        let ssh = MockRemoteExec::new(vec![(0, String::new(), String::new())]);
        run_status(make_ctx(conn, ssh, uuid)).await.unwrap();
    }

    #[tokio::test]
    async fn status_shortname_selector_resolves() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        insert_active_session(conn, host_id, uuid, "shortapp");

        run_status(make_ctx(conn, MockRemoteExec::unreachable(), "shortapp"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn status_no_mutation_on_dead_session() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        insert_active_session(conn, host_id, uuid, "deadapp");
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        session_repo::set_status(conn, uuid, "dead", now).unwrap();

        // Status must succeed even on dead sessions (no mutation)
        run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
            .await
            .unwrap();
        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "dead", "status must not be mutated");
    }

    #[tokio::test]
    async fn status_no_mutation_on_unreachable_session() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        insert_active_session(conn, host_id, uuid, "unreachapp");
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        session_repo::set_status(conn, uuid, "unreachable", now).unwrap();

        run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
            .await
            .unwrap();
        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "unreachable", "status must not be mutated");
    }
}
