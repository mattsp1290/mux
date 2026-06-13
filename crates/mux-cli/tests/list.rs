//! Integration tests for `mux list` — docs/07 §List flow, docs/08
//!
//! Proves the full list command contract:
//!   1. In-flight reservation rows (tmux_name IS NULL) are excluded from reconciliation.
//!   2. SSH health probe does NOT perform TOFU (only cat + kill -0 are issued).
//!   3. Unreachable host: active sessions marked unreachable; non-active unchanged.
//!   4. Reachable host: reconciliation rules applied per-session.
//!      a. Import unknown live mux- session (imported=1, workdir from agent).
//!      b. Active mux- session absent from agent → orphaned.
//!      c. Active non-mux session absent from agent → unreachable.
//!      d. Unreachable session present in agent → resurrected to active.
//!      e. Status sync: agent-reported status written to DB.
//!   5. Dead and orphaned sessions are never resurfaced by reconciliation.
//!   6. list_for_host returns sessions in created_at ascending order (SQL ORDER BY).
//!   7. Multiple hosts reconciled independently.
//!   8. Non-mux-prefixed agent sessions are not imported.
//!   9. --plain flag: accepted; DB state unaffected (output format not directly asserted here).

use std::cell::RefCell;
use std::collections::VecDeque;

use mux_cli::agent_start::RemoteExec;
use mux_cli::list::{ListContext, run_list};
use mux_core::error::MuxError;
use mux_rpc::schema::{ListSessionsResponse, RpcResult, SessionInfo, SessionStatusValue};
use mux_state::model::Host;
use mux_state::session_repo::{activate, ReserveParams};
use mux_state::{host_repo, session_repo};
use mux_state::store::Store;
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ── MockRemoteExec ────────────────────────────────────────────────────────────

struct MockExec {
    responses: RefCell<VecDeque<(i32, String, String)>>,
}

impl MockExec {
    fn new(responses: Vec<(i32, String, String)>) -> Self {
        MockExec { responses: RefCell::new(responses.into()) }
    }

    fn unreachable() -> Self {
        MockExec { responses: RefCell::new(VecDeque::new()) }
    }
}

impl RemoteExec for MockExec {
    fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| MuxError::Other(anyhow::anyhow!("SSH unreachable (mock)")))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn open_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("mux.db")).unwrap();
    (dir, store)
}

fn insert_host(conn: &Connection) -> i64 {
    let id = host_repo::insert(conn, "myhost", "user", "192.0.2.1", 22, 1_000_000).unwrap();
    host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user"), Some("tcp")).unwrap();
    id
}

fn insert_host2(conn: &Connection) -> i64 {
    let id = host_repo::insert(conn, "host2", "user", "192.0.2.2", 22, 1_000_000).unwrap();
    host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user2"), Some("tcp")).unwrap();
    id
}

fn insert_active_session(
    conn: &Connection,
    host_id: i64,
    uuid: &str,
    shortname: &str,
    created_at: i64,
) {
    session_repo::reserve(
        conn,
        &ReserveParams {
            uuid,
            host_id,
            shortname,
            repo_slug: "owner/repo",
            branch: "main",
            created_at,
        },
    )
    .unwrap();
    activate(conn, uuid, &format!("mux-{shortname}"), "/work/repo", "tcp", created_at + 1).unwrap();
}

// Wrap ListContext construction in a typed helper to satisfy HRTB inference.
fn list_ctx<'a, F>(conn: &'a Connection, make_exec: F, plain: bool) -> ListContext<'a, F>
where
    F: Fn(&Host) -> MockExec,
{
    ListContext { conn, make_exec, plain }
}

// ── RPC server helpers ────────────────────────────────────────────────────────

fn encode_list_response(sessions: Vec<SessionInfo>) -> Vec<u8> {
    let result: RpcResult<ListSessionsResponse> =
        RpcResult::Ok(ListSessionsResponse { sessions });
    let body = mux_rpc::codec::encode(&result).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

async fn spawn_list_server(response_frame: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            use tokio::io::AsyncReadExt;
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).await.is_ok() {
                let body_len = u32::from_le_bytes(len_buf) as usize;
                let mut body = vec![0u8; body_len];
                let _ = stream.read_exact(&mut body).await;
            }
            let _ = stream.write_all(&response_frame).await;
        }
    });
    port
}

fn lock_json(port: u16) -> String {
    format!(r#"{{"pid":99999,"tcp_url":"tcp://127.0.0.1:{port}"}}"#)
}

fn agent_running_responses(port: u16) -> Vec<(i32, String, String)> {
    // [0] → cat agent.lock returns the lock JSON
    // [1] → kill -0 <pid> returns exit 0 (process alive)
    vec![(0, lock_json(port), String::new()), (0, String::new(), String::new())]
}

fn session_info(uuid: &str, shortname: &str, status: SessionStatusValue) -> SessionInfo {
    SessionInfo {
        uuid: uuid.to_owned(),
        shortname: shortname.to_owned(),
        tmux_name: format!("mux-{shortname}"),
        workdir: format!("/remote/work/{shortname}"),
        status,
    }
}

// ── Contract tests ────────────────────────────────────────────────────────────

/// docs/07 §List flow point 1: in-flight reservation rows (tmux_name IS NULL)
/// are excluded from list reconciliation.
///
/// Protection mechanism: `list_for_host` filters `tmux_name IS NOT NULL`, so the
/// in-flight UUID is absent from `all_uuids`. The import path then issues an INSERT
/// that is silently swallowed by the UNIQUE constraint + `INSERT OR IGNORE` in
/// `import_session`, leaving the in-flight row intact.
#[tokio::test]
async fn list_in_flight_reservation_excluded_from_reconciliation() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);

    // Reserve-only row: tmux_name IS NULL (not yet activated).
    let in_flight_uuid = "aaaa0001-0000-0000-0000-000000000000";
    session_repo::reserve(
        conn,
        &ReserveParams {
            uuid: in_flight_uuid,
            host_id,
            shortname: "inflight",
            repo_slug: "owner/repo",
            branch: "main",
            created_at: 1_000_000,
        },
    )
    .unwrap();
    // NOT activated — tmux_name remains NULL.

    // Agent reports the same UUID as active. The UNIQUE constraint + INSERT OR IGNORE
    // prevents duplication; the in-flight reservation row is not mutated.
    let agent_resp = vec![SessionInfo {
        uuid: in_flight_uuid.to_owned(),
        shortname: "inflight".to_owned(),
        tmux_name: "mux-inflight".to_owned(),
        workdir: "/remote/inflight".to_owned(),
        status: SessionStatusValue::Active,
    }];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    // The in-flight reservation must not be mutated.
    let s = session_repo::get_by_uuid(conn, in_flight_uuid).unwrap().unwrap();
    assert!(
        s.tmux_name.is_none(),
        "in-flight reservation (tmux_name IS NULL) must not be activated by list"
    );
    assert!(!s.imported, "in-flight reservation must not be replaced by an import");
}

/// docs/07 §List flow point 2: SSH health probe must NOT perform TOFU.
/// AgentStarter::probe_existing only issues `cat` (read lock) and `kill -0`
/// (PID check) — no fingerprint verification. This is verified structurally by
/// proving that run_list succeeds with exactly those 2 responses pre-loaded.
/// If any additional SSH command were issued, the mock queue would be exhausted,
/// the probe would fail, and sessions would not be correctly reconciled.
#[tokio::test]
async fn list_probe_issues_no_tofu_commands() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "bbbb0001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid, "probe-app", 1_000_000);

    // Agent returns the session as active.
    let agent_resp = vec![session_info(uuid, "probe-app", SessionStatusValue::Active)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    // Only 2 SSH responses loaded: cat agent.lock + kill -0.
    // If a TOFU command (3rd SSH call) were issued, the mock queue would be empty
    // and probe_agent would return None, leaving the session unreachable instead of active.
    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(
        s.status, "active",
        "session must be active (proves probe succeeded with only cat+kill-0 commands)"
    );
}

/// docs/07 rule: import unknown live mux- session → imported=1, workdir from agent.
#[tokio::test]
async fn list_import_sets_imported_flag_and_workdir_from_agent() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let _host_id = insert_host(conn);

    let uuid = "cccc0001-0000-0000-0000-000000000000";
    let agent_resp = vec![SessionInfo {
        uuid: uuid.to_owned(),
        shortname: "imported-svc".to_owned(),
        tmux_name: "mux-imported-svc".to_owned(),
        workdir: "/remote/work/imported-svc".to_owned(),
        status: SessionStatusValue::Active,
    }];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().expect("session must be imported");
    assert_eq!(s.status, "active");
    assert!(s.imported, "imported=1 must be set on imported sessions (docs/07 rule 1)");
    assert_eq!(s.shortname, "imported-svc", "shortname derived by stripping mux- prefix");
    assert_eq!(
        s.workdir.as_deref(),
        Some("/remote/work/imported-svc"),
        "workdir must come from agent response (docs/07 rule 1)"
    );
}

/// docs/07 rule: agent-reported status is synced to DB — covers dead sync.
#[tokio::test]
async fn list_syncs_agent_reported_dead_status_to_db() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "dddd0001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid, "dying-app", 1_000_000);

    let agent_resp = vec![session_info(uuid, "dying-app", SessionStatusValue::Dead)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "dead", "agent-reported dead status must be synced to DB");
}

/// docs/07 rule: agent-reported unreachable status is synced to DB.
#[tokio::test]
async fn list_syncs_agent_reported_unreachable_status_to_db() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "eeee0001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid, "spotty-app", 1_000_000);

    let agent_resp = vec![session_info(uuid, "spotty-app", SessionStatusValue::Unreachable)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "unreachable", "agent-reported unreachable must be synced to DB");
}

/// docs/07 §List flow 2b: unreachable host marks active sessions unreachable.
/// Non-active (orphaned, already-unreachable) sessions must not be touched.
#[tokio::test]
async fn list_unreachable_host_marks_active_sessions_unreachable() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);

    let active_uuid = "ffff0001-0000-0000-0000-000000000000";
    let orp_uuid = "ffff0002-0000-0000-0000-000000000000";
    let unr_uuid = "ffff0003-0000-0000-0000-000000000000";

    insert_active_session(conn, host_id, active_uuid, "act-app", 1_000_000);
    insert_active_session(conn, host_id, orp_uuid, "orp-app", 1_000_001);
    session_repo::set_status(conn, orp_uuid, "orphaned", 2_000_000).unwrap();
    insert_active_session(conn, host_id, unr_uuid, "unr-app", 1_000_002);
    session_repo::set_status(conn, unr_uuid, "unreachable", 2_000_000).unwrap();

    run_list(list_ctx(conn, |_| MockExec::unreachable(), false))
        .await
        .unwrap();

    assert_eq!(
        session_repo::get_by_uuid(conn, active_uuid).unwrap().unwrap().status,
        "unreachable",
        "active session on unreachable host must be marked unreachable"
    );
    assert_eq!(
        session_repo::get_by_uuid(conn, orp_uuid).unwrap().unwrap().status,
        "orphaned",
        "orphaned session must not be changed on unreachable host"
    );
    assert_eq!(
        session_repo::get_by_uuid(conn, unr_uuid).unwrap().unwrap().status,
        "unreachable",
        "already-unreachable session must remain unreachable"
    );
}

/// docs/07 rule 6: dead and orphaned sessions are never resurfaced,
/// even when the agent reports them as active.
#[tokio::test]
async fn list_dead_and_orphaned_sessions_never_resurfaced() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);

    let dead_uuid = "0001dead-0000-0000-0000-000000000000";
    let orp_uuid = "0001orp0-0000-0000-0000-000000000000";

    insert_active_session(conn, host_id, dead_uuid, "deadapp", 1_000_000);
    session_repo::set_status(conn, dead_uuid, "dead", 2_000_000).unwrap();
    insert_active_session(conn, host_id, orp_uuid, "orpapp", 1_000_001);
    session_repo::set_status(conn, orp_uuid, "orphaned", 2_000_000).unwrap();

    // Agent claims both are active.
    let agent_resp = vec![
        session_info(dead_uuid, "deadapp", SessionStatusValue::Active),
        session_info(orp_uuid, "orpapp", SessionStatusValue::Active),
    ];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    assert_eq!(
        session_repo::get_by_uuid(conn, dead_uuid).unwrap().unwrap().status,
        "dead",
        "dead session must stay dead even if agent reports it"
    );
    assert_eq!(
        session_repo::get_by_uuid(conn, orp_uuid).unwrap().unwrap().status,
        "orphaned",
        "orphaned session must not be resurrected by an agent report"
    );
}

/// docs/07 rule 3: active mux- session whose UUID is absent from the agent list → orphaned.
#[tokio::test]
async fn list_active_mux_session_absent_from_agent_marked_orphaned() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "00020001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid, "orphan-cand", 1_000_000);

    // Agent returns an empty list — our session is absent.
    let port = spawn_list_server(encode_list_response(vec![])).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(
        s.status, "orphaned",
        "active mux- session absent from agent's ListSessions must be marked orphaned"
    );
}

/// docs/07 rule 4: active non-mux session absent from agent → unreachable (not orphaned).
#[tokio::test]
async fn list_active_non_mux_session_absent_from_agent_marked_unreachable() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "00030001-0000-0000-0000-000000000000";

    // Activate with a non-mux tmux_name.
    session_repo::reserve(
        conn,
        &ReserveParams {
            uuid,
            host_id,
            shortname: "ext-svc",
            repo_slug: "owner/repo",
            branch: "main",
            created_at: 1_000_000,
        },
    )
    .unwrap();
    activate(conn, uuid, "external-tmux-session", "/work/ext", "tcp", 1_000_001).unwrap();

    let port = spawn_list_server(encode_list_response(vec![])).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(
        s.status, "unreachable",
        "active non-mux session absent from agent must be unreachable, not orphaned"
    );
}

/// docs/07 rule 5: unreachable session resurrected when agent reports it as active.
#[tokio::test]
async fn list_resurrects_unreachable_session_when_agent_reports_it() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "00040001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid, "comeback", 1_000_000);
    session_repo::set_status(conn, uuid, "unreachable", 2_000_000).unwrap();

    let agent_resp = vec![session_info(uuid, "comeback", SessionStatusValue::Active)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(
        s.status, "active",
        "unreachable session reported by agent must be resurrected to active"
    );
}

/// docs/07 point 7: multiple hosts reconciled independently.
/// Unreachable second host must not affect sessions on a reachable first host.
#[tokio::test]
async fn list_multiple_hosts_reconciled_independently() {
    let (_dir, store) = open_store();
    let conn = store.conn();

    let host1_id = insert_host(conn);
    let host2_id = insert_host2(conn);

    let uuid1 = "01000001-0000-0000-0000-000000000000";
    let uuid2 = "02000001-0000-0000-0000-000000000000";

    insert_active_session(conn, host1_id, uuid1, "app-h1", 1_000_000);
    insert_active_session(conn, host2_id, uuid2, "app-h2", 1_000_000);

    // host1 (myhost): agent returns session as active.
    // host2: unreachable.
    let agent_resp = vec![session_info(uuid1, "app-h1", SessionStatusValue::Active)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(
        conn,
        move |host: &Host| {
            if host.alias == "myhost" {
                MockExec::new(agent_running_responses(port))
            } else {
                MockExec::unreachable()
            }
        },
        false,
    ))
    .await
    .unwrap();

    let s1 = session_repo::get_by_uuid(conn, uuid1).unwrap().unwrap();
    let s2 = session_repo::get_by_uuid(conn, uuid2).unwrap().unwrap();

    assert_eq!(s1.status, "active", "session on reachable host must remain active");
    assert_eq!(s2.status, "unreachable", "session on unreachable host must be marked unreachable");
}

/// list_for_host returns sessions ordered by created_at ASC within a host.
/// The display layer iterates the result in this order; the ORDER BY is in the SQL query.
#[tokio::test]
async fn list_for_host_returns_sessions_in_created_at_ascending_order() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);

    // Insert in reverse chronological order to make ordering observable.
    let uuid_late = "03000001-0000-0000-0000-000000000000";
    let uuid_early = "03000002-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid_late, "app-late", 2_000_000);
    insert_active_session(conn, host_id, uuid_early, "app-early", 1_000_000);

    let sessions = session_repo::list_for_host(conn, host_id).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].uuid, uuid_early, "earlier created_at must be first");
    assert_eq!(sessions[1].uuid, uuid_late, "later created_at must be second");
}

/// --plain flag: accepted with sessions present; DB state is unaffected.
/// Output format (tab-separated columns) is not directly asserted here —
/// that requires refactoring display_all to accept a writer.
#[tokio::test]
async fn list_plain_flag_succeeds_with_sessions() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "04000001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, uuid, "plain-app", 1_000_000);

    let agent_resp = vec![session_info(uuid, "plain-app", SessionStatusValue::Active)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), true))
        .await
        .unwrap();

    // --plain does not mutate session state.
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active", "plain mode must not mutate session status");
}

/// Non-mux-prefixed sessions from the agent must not be imported.
/// docs/07: only mux- prefixed sessions are under mux management.
#[tokio::test]
async fn list_non_mux_agent_sessions_not_imported() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);

    let non_mux_uuid = "05000001-0000-0000-0000-000000000000";
    let non_mux_info = SessionInfo {
        uuid: non_mux_uuid.to_owned(),
        shortname: "external".to_owned(),
        tmux_name: "external-session".to_owned(), // no mux- prefix
        workdir: "/some/path".to_owned(),
        status: SessionStatusValue::Active,
    };
    let port = spawn_list_server(encode_list_response(vec![non_mux_info])).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap();

    let s = session_repo::get_by_uuid(conn, non_mux_uuid).unwrap();
    assert!(s.is_none(), "non-mux-prefixed agent session must not be imported");

    // Verify the host has no sessions (import was properly filtered).
    let all = session_repo::list_for_host(conn, host_id).unwrap();
    assert!(all.is_empty(), "no sessions must exist after non-mux-only agent response");
}

/// Regression: dead UUID in agent must not crash run_list (UNIQUE constraint fix).
/// All DB rows (including dead) are used for the import guard to prevent re-import.
#[tokio::test]
async fn list_dead_uuid_in_agent_succeeds_and_row_stays_dead() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let dead_uuid = "06000001-0000-0000-0000-000000000000";
    insert_active_session(conn, host_id, dead_uuid, "ex-app", 1_000_000);
    session_repo::set_status(conn, dead_uuid, "dead", 2_000_000).unwrap();

    // Agent still reports the dead session as active.
    let agent_resp = vec![session_info(dead_uuid, "ex-app", SessionStatusValue::Active)];
    let port = spawn_list_server(encode_list_response(agent_resp)).await;

    run_list(list_ctx(conn, move |_| MockExec::new(agent_running_responses(port)), false))
        .await
        .unwrap(); // must not crash with UNIQUE violation

    let s = session_repo::get_by_uuid(conn, dead_uuid).unwrap().unwrap();
    assert_eq!(s.status, "dead", "dead row must stay dead when agent reports its UUID");
}
