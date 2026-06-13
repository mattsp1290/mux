//! Integration tests for `mux status` — docs/07 §Status flow, docs/08
//!
//! Proves the full status command contract:
//!   1. UUID-vs-shortname resolution (UUID tried first; UUID format → no fallback).
//!   2. Host-alias strings are not valid session selectors → "not found" error.
//!   3. Live GetSession RPC displayed when agent is reachable.
//!   4. Local data displayed (with note) when host is unreachable.
//!   5. No mutation of session status under any path.
//!   6. Missing session → error.

use std::cell::RefCell;
use std::collections::VecDeque;

use mux_cli::agent_start::RemoteExec;
use mux_cli::status::{StatusContext, run_status};
use mux_core::error::MuxError;
use mux_rpc::schema::{GetSessionResponse, RpcError, RpcResult, SessionStatusValue};
use mux_state::session_repo::{activate, ReserveParams};
use mux_state::{host_repo, session_repo};
use mux_state::store::Store;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ── MockRemoteExec ────────────────────────────────────────────────────────────

struct MockRemoteExec {
    responses: RefCell<VecDeque<(i32, String, String)>>,
}

impl MockRemoteExec {
    fn new(responses: Vec<(i32, String, String)>) -> Self {
        MockRemoteExec { responses: RefCell::new(responses.into()) }
    }

    fn unreachable() -> Self {
        MockRemoteExec { responses: RefCell::new(VecDeque::new()) }
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

fn insert_host_no_probe(conn: &rusqlite::Connection) -> i64 {
    host_repo::insert(conn, "rawhost", "user", "192.0.2.2", 22, 1_000_000).unwrap()
    // no update_probe — host.home is None
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

// ── RPC server helpers ────────────────────────────────────────────────────────

fn encode_get_session_ok(resp: GetSessionResponse) -> Vec<u8> {
    let result: RpcResult<GetSessionResponse> = RpcResult::Ok(resp);
    let body = mux_rpc::codec::encode(&result).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

fn encode_get_session_err(err: RpcError) -> Vec<u8> {
    let result: RpcResult<GetSessionResponse> = RpcResult::Err(err);
    let body = mux_rpc::codec::encode(&result).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

async fn spawn_rpc_server(response_frame: Vec<u8>) -> u16 {
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
    // Two responses in order:
    //   [0] → `cat agent.lock` returns the lock JSON (agent is running at port)
    //   [1] → `kill -0 <pid>` returns exit 0 (process is alive)
    vec![(0, lock_json(port), String::new()), (0, String::new(), String::new())]
}

// ── 1. UUID-vs-shortname resolution ──────────────────────────────────────────

#[tokio::test]
async fn status_resolves_by_uuid() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    insert_active_session(conn, host_id, uuid, "myapp");

    run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
        .await
        .expect("status by UUID must succeed");
}

#[tokio::test]
async fn status_resolves_by_shortname() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    insert_active_session(conn, host_id, uuid, "shortapp");

    run_status(make_ctx(conn, MockRemoteExec::unreachable(), "shortapp"))
        .await
        .expect("status by shortname must succeed");
}

#[tokio::test]
async fn status_uuid_format_no_shortname_fallback() {
    // Proves: UUID-format selector → UUID lookup only; no fallback to shortname.
    //
    // The session's shortname is set to the SAME string as the UUID selector.
    // If resolve_session fell back to shortname lookup, run_status would succeed
    // (shortname matches). Since there's no fallback, it returns "not found".
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let selector = "00000000-0000-0000-0000-000000000000";
    // Real UUID differs from selector; shortname equals selector.
    insert_active_session(conn, host_id, "cccccccc-cccc-cccc-cccc-cccccccccccc", selector);

    let err = run_status(make_ctx(conn, MockRemoteExec::unreachable(), selector))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "got: {err}");
}

// ── 2. Host-alias strings are not valid session selectors ─────────────────────

#[tokio::test]
async fn status_host_alias_not_found_as_session() {
    // A host alias ("myhost") must not resolve as a session selector.
    // Even though "myhost" is a known host alias, there is no session with
    // shortname "myhost", so the command returns "not found".
    let (_dir, store) = open_store();
    let conn = store.conn();
    insert_host(conn); // host alias = "myhost"

    let err = run_status(make_ctx(conn, MockRemoteExec::unreachable(), "myhost"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "got: {err}");
}

// ── 3. Live GetSession RPC ────────────────────────────────────────────────────

#[tokio::test]
async fn status_live_path_success() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "dddddddd-dddd-dddd-dddd-dddddddddddd";
    insert_active_session(conn, host_id, uuid, "liveapp");

    let resp = GetSessionResponse {
        uuid: uuid.to_owned(),
        shortname: "liveapp".to_owned(),
        tmux_name: "mux-liveapp".to_owned(),
        status: SessionStatusValue::Active,
    };
    let port = spawn_rpc_server(encode_get_session_ok(resp)).await;
    let ssh = MockRemoteExec::new(agent_running_responses(port));

    run_status(make_ctx(conn, ssh, uuid))
        .await
        .expect("live path must succeed");

    // docs/07 §5: no mutation during status, even on the live path.
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active", "live path must not mutate session status");
}

#[tokio::test]
async fn status_live_rpc_error_is_surfaced() {
    // docs/07 §4: non-not_found RPC errors must propagate up, not be swallowed.
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "d1d1d1d1-d1d1-d1d1-d1d1-d1d1d1d1d1d1";
    insert_active_session(conn, host_id, uuid, "errapp");

    let port = spawn_rpc_server(encode_get_session_err(RpcError::internal("agent_panic: boom")))
        .await;
    let ssh = MockRemoteExec::new(agent_running_responses(port));

    let err = run_status(make_ctx(conn, ssh, uuid))
        .await
        .unwrap_err();
    // Error must surface (not be silently swallowed as "local fallback").
    let msg = err.to_string();
    assert!(!msg.is_empty(), "non-not_found RPC error must propagate");
}

#[tokio::test]
async fn status_live_not_found_falls_back_to_local_no_mutation() {
    // Agent running but reports session not found → AgentNotFound path.
    // Local data must be displayed; session state must not be mutated.
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
    insert_active_session(conn, host_id, uuid, "orphanedapp");

    let port = spawn_rpc_server(encode_get_session_err(RpcError::not_found(
        "not_found: no such session",
    )))
    .await;
    let ssh = MockRemoteExec::new(agent_running_responses(port));

    run_status(make_ctx(conn, ssh, uuid))
        .await
        .expect("not_found from agent must not propagate as error");

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active", "status must not be mutated on agent not_found");
}

// ── 4. Unreachable host → local fallback ─────────────────────────────────────

#[tokio::test]
async fn status_unreachable_host_displays_local_data() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    insert_active_session(conn, host_id, uuid, "unreachapp");

    // SSH probe fails immediately → ProbeError path
    run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
        .await
        .expect("unreachable host must not error; local data shown instead");
}

#[tokio::test]
async fn status_no_agent_lock_displays_local_data() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "11111111-1111-1111-1111-111111111111";
    insert_active_session(conn, host_id, uuid, "noagentapp");

    // read_lock returns empty string → NoAgent path
    let ssh = MockRemoteExec::new(vec![(0, String::new(), String::new())]);
    run_status(make_ctx(conn, ssh, uuid))
        .await
        .expect("no agent lock must not error; local data shown instead");
}

#[tokio::test]
async fn status_host_not_probed_displays_local_data() {
    // host.home is None (no 'mux host test' run) → HostNotProbed path
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_no_probe(conn);
    let uuid = "22222222-2222-2222-2222-222222222222";
    insert_active_session(conn, host_id, uuid, "rawapp");

    run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
        .await
        .expect("host not probed must display local data, not error");
}

// ── 5. No mutation under any path ─────────────────────────────────────────────

#[tokio::test]
async fn status_does_not_mutate_dead_session() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "33333333-3333-3333-3333-333333333333";
    insert_active_session(conn, host_id, uuid, "deadapp");
    session_repo::set_status(conn, uuid, "dead", 2_000_000).unwrap();

    run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
        .await
        .expect("status on dead session must succeed");

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "dead", "status must not mutate dead → something else");
}

#[tokio::test]
async fn status_does_not_mutate_unreachable_session() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host(conn);
    let uuid = "44444444-4444-4444-4444-444444444444";
    insert_active_session(conn, host_id, uuid, "reachapp");
    session_repo::set_status(conn, uuid, "unreachable", 2_000_000).unwrap();

    run_status(make_ctx(conn, MockRemoteExec::unreachable(), uuid))
        .await
        .expect("status on unreachable session must succeed");

    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "unreachable", "status must not mutate unreachable → active");
}

// ── 6. Missing session → error ────────────────────────────────────────────────

#[tokio::test]
async fn status_unknown_uuid_errors() {
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
async fn status_unknown_shortname_errors() {
    let (_dir, store) = open_store();
    let conn = store.conn();

    let err = run_status(make_ctx(conn, MockRemoteExec::unreachable(), "no-such-app"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "got: {err}");
}

#[tokio::test]
async fn status_ambiguous_shortname_errors() {
    // Same shortname on two hosts → ambiguous; must error with "ambiguous".
    let (_dir, store) = open_store();
    let conn = store.conn();
    let h1 = host_repo::insert(conn, "host1", "u", "1.1.1.1", 22, 1_000_000).unwrap();
    let h2 = host_repo::insert(conn, "host2", "u", "2.2.2.2", 22, 1_000_000).unwrap();
    insert_active_session(conn, h1, "55555555-5555-5555-5555-555555555555", "shared");
    insert_active_session(conn, h2, "66666666-6666-6666-6666-666666666666", "shared");

    let err = run_status(make_ctx(conn, MockRemoteExec::unreachable(), "shared"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ambiguous"), "got: {err}");
}
