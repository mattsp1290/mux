//! Integration tests for `mux kill` — docs/07 §Kill flow, docs/04 §TOFU, docs/08
//!
//! These tests exercise the full kill flow down to the RPC layer using a
//! real in-process TCP server, giving confidence in the state-mutation gates
//! that the unit tests cannot reach without a live agent.

use std::cell::RefCell;
use std::collections::VecDeque;

use mux_cli::agent_start::RemoteExec;
use mux_cli::create::{HostKeyInfo, SshHost};
use mux_cli::kill::{KillContext, run_kill};
use mux_core::error::MuxError;
use mux_rpc::schema::{KillSessionResponse, RpcError, RpcResult};
use mux_state::session_repo::{activate, ReserveParams};
use mux_state::{host_repo, session_repo};
use mux_state::store::Store;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ── MockSshHost ───────────────────────────────────────────────────────────────

struct MockSshHost {
    responses: RefCell<VecDeque<(i32, String, String)>>,
    host_key_result: Result<HostKeyInfo, MuxError>,
}

impl MockSshHost {
    fn with_key(fingerprint: impl Into<String>, responses: Vec<(i32, String, String)>) -> Self {
        MockSshHost {
            responses: RefCell::new(responses.into()),
            host_key_result: Ok(HostKeyInfo {
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: fingerprint.into(),
            }),
        }
    }
}

impl RemoteExec for MockSshHost {
    fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| MuxError::Other(anyhow::anyhow!("no more mock responses")))
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

// ── Test helpers ──────────────────────────────────────────────────────────────

fn open_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("mux.db");
    let store = Store::open(&db_path).unwrap();
    (dir, store)
}

fn insert_host_with_home(conn: &rusqlite::Connection) -> i64 {
    let id = host_repo::insert(conn, "myhost", "user", "192.0.2.1", 22, 1_000_000).unwrap();
    host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user"), Some("tcp")).unwrap();
    id
}

fn insert_active_session(
    conn: &rusqlite::Connection,
    host_id: i64,
    uuid: &str,
    shortname: &str,
    repo_slug: &str,
    imported: bool,
) {
    if imported {
        session_repo::import_session(
            conn,
            &mux_state::session_repo::ImportParams {
                uuid,
                host_id,
                shortname,
                tmux_name: Some(&format!("mux-{shortname}")),
                repo_slug,
                branch: "main",
                workdir: Some("/remote/path"),
                transport_mode: Some("tcp"),
                created_at: 1_000_000,
                updated_at: 1_000_000,
            },
        )
        .unwrap();
    } else {
        session_repo::reserve(
            conn,
            &ReserveParams {
                uuid,
                host_id,
                shortname,
                repo_slug,
                branch: "main",
                created_at: 1_000_000,
            },
        )
        .unwrap();
        activate(conn, uuid, &format!("mux-{shortname}"), "/work/repo", "tcp", 1_000_001)
            .unwrap();
    }
}

/// Encode a KillSessionResponse as a framed RPC response.
fn encode_kill_response(resp: KillSessionResponse) -> Vec<u8> {
    let result = RpcResult::Ok(resp);
    let body = mux_rpc::codec::encode(&result).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// Encode an RpcError as a framed RPC error response.
fn encode_rpc_error(err: RpcError) -> Vec<u8> {
    let result: RpcResult<KillSessionResponse> = RpcResult::Err(err);
    let body = mux_rpc::codec::encode(&result).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// Spawn a one-shot TCP RPC server returning the given framed response.
/// Returns the bound port.
async fn spawn_kill_server(response_frame: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            // Read the request frame (discard it — we don't need to decode for these tests)
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

/// Build agent.lock JSON for the given TCP port.
fn lock_json(port: u16) -> String {
    format!(r#"{{"pid":99999,"tcp_url":"tcp://127.0.0.1:{port}"}}"#)
}

/// Build MockSshHost responses for probe_existing finding an agent at `port`.
fn agent_running_responses(port: u16) -> Vec<(i32, String, String)> {
    // probe_existing calls:
    // 1. read_lock: cat ~/.mux/agent.lock 2>/dev/null
    // 2. is_process_alive: kill -0 <pid> 2>/dev/null
    vec![(0, lock_json(port), String::new()), (0, String::new(), String::new())]
}

fn trust_fingerprint(conn: &rusqlite::Connection, host_id: i64) {
    mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "FP").unwrap();
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// When the agent returns not_owned, the session must remain active.
#[tokio::test]
async fn kill_not_owned_refuses_mutation() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    insert_active_session(conn, host_id, uuid, "myapp", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    let port = spawn_kill_server(encode_rpc_error(RpcError::not_owned("not in map"))).await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    let err = run_kill(ctx).await.unwrap_err();
    assert!(
        err.to_string().contains("not owned by this client"),
        "expected 'not owned by this client', got: {err}"
    );
    // Status must not have been mutated
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active", "status should remain active after not_owned");
}

/// When tmux_killed=true, the session must be marked dead.
#[tokio::test]
async fn kill_tmux_killed_marks_session_dead() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    insert_active_session(conn, host_id, uuid, "myapp2", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    let port = spawn_kill_server(encode_kill_response(KillSessionResponse {
        tmux_killed: true,
        workdir_removed: false,
    }))
    .await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    run_kill(ctx).await.unwrap();
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "dead", "status should be dead after tmux_killed");
}

/// When workdir_removed=true (and tmux_killed=false), session must still be marked dead.
#[tokio::test]
async fn kill_workdir_removed_marks_session_dead() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    insert_active_session(conn, host_id, uuid, "myapp3", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    let port = spawn_kill_server(encode_kill_response(KillSessionResponse {
        tmux_killed: false,
        workdir_removed: true,
    }))
    .await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    run_kill(ctx).await.unwrap();
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "dead", "status should be dead after workdir_removed");
}

/// When neither tmux_killed nor workdir_removed, session status must not change.
#[tokio::test]
async fn kill_no_effect_leaves_state_unchanged() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "dddddddd-dddd-dddd-dddd-dddddddddddd";
    insert_active_session(conn, host_id, uuid, "myapp4", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    let port = spawn_kill_server(encode_kill_response(KillSessionResponse {
        tmux_killed: false,
        workdir_removed: false,
    }))
    .await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    run_kill(ctx).await.unwrap();
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active", "status must not change when no effect reported");
}

/// Imported sessions: agent always reports workdir_removed=false (docs/07 step 8).
/// When tmux_killed=false and workdir_removed=false, local state must not change.
#[tokio::test]
async fn kill_imported_session_no_local_cleanup() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
    insert_active_session(conn, host_id, uuid, "importapp", "owner/repo", true);
    trust_fingerprint(conn, host_id);

    // Agent reports both false (imported session — agent never removes external workdir)
    let port = spawn_kill_server(encode_kill_response(KillSessionResponse {
        tmux_killed: false,
        workdir_removed: false,
    }))
    .await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    run_kill(ctx).await.unwrap();
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    // Imported sessions with no effect must remain active — no dead-mark
    assert_eq!(s.status, "active");
    assert!(s.imported);
}

/// not_found from agent: local state reconciled to dead.
#[tokio::test]
async fn kill_not_found_reconciles_to_dead() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    insert_active_session(conn, host_id, uuid, "goneapp", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    let port =
        spawn_kill_server(encode_rpc_error(RpcError::not_found("session unknown"))).await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    run_kill(ctx).await.unwrap();
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(
        s.status, "dead",
        "not_found should reconcile local session to dead"
    );
}

/// When no agent is running, kill must fail without starting one.
#[tokio::test]
async fn kill_no_agent_running_errors_without_starting() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "11111111-2222-3333-4444-555555555555";
    insert_active_session(conn, host_id, uuid, "sleepapp", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    // read_lock returns empty (no agent.lock)
    let ssh = MockSshHost::with_key("FP", vec![(0, String::new(), String::new())]);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    let err = run_kill(ctx).await.unwrap_err();
    assert!(
        err.to_string().contains("no agent running"),
        "expected 'no agent running', got: {err}"
    );
    // Session must still be active
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active");
}

/// Verify that the kill flow marks sessions dead when both effects are reported.
#[tokio::test]
async fn kill_both_effects_marks_dead() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "12121212-1212-1212-1212-121212121212";
    insert_active_session(conn, host_id, uuid, "bothapp", "owner/repo", false);
    trust_fingerprint(conn, host_id);

    let port = spawn_kill_server(encode_kill_response(KillSessionResponse {
        tmux_killed: true,
        workdir_removed: true,
    }))
    .await;
    let responses = agent_running_responses(port);
    let ssh = MockSshHost::with_key("FP", responses);
    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    run_kill(ctx).await.unwrap();
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "dead");
}

/// TOFU mismatch before mutation gate: session stays active, no RPC attempt.
#[tokio::test]
async fn kill_tofu_mismatch_never_reaches_rpc() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host_id = insert_host_with_home(conn);
    let uuid = "abababab-abab-abab-abab-abababababab";
    insert_active_session(conn, host_id, uuid, "mismatchapp2", "owner/repo", false);
    // Trust "STORED_FP" but SSH will return "DIFFERENT_FP"
    mux_ssh::trust::trust_fingerprint(conn, host_id, "ssh-ed25519", "STORED_FP").unwrap();

    // No responses needed — should bail before any SSH commands for lock
    let ssh = MockSshHost::with_key("DIFFERENT_FP", vec![]);

    let ctx = KillContext { conn, ssh, selector: uuid.to_owned(), is_interactive: false };

    let err = run_kill(ctx).await.unwrap_err();
    assert!(err.to_string().contains("mismatch"), "{err}");
    let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
    assert_eq!(s.status, "active", "status must remain active after TOFU mismatch");
}
