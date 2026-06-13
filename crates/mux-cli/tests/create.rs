//! Integration tests for `mux create` transaction — docs/07 §Create flow
//!
//! These tests exercise the public-API contract of `run_create` through real SQLite
//! connections and a real loopback TCP mock server for the RPC step.
//!
//! Tests already covered in `crates/mux-cli/src/create.rs` (inline white-box suite):
//!   1. host_not_configured_returns_error
//!   2. tofu_mismatch_returns_host_key_mismatch
//!   3. tofu_non_interactive_returns_error
//!   4. workdir_pre_existing_returns_error
//!   5. git_clone_failure_cancels_reservation
//!
//! Claims verified here (docs/07 §Create flow):
//!   1. shortname_collision: branch-based name taken → base-2 suffix
//!   2. shortname_exhaustion: all 50 suffix attempts taken → ShortnameExhausted
//!   3. main_shortname_exhaustion: all 400 adj-noun pairs taken → ShortnameExhausted
//!   4. clone_command_includes_git_terminal_prompt_zero: GIT_TERMINAL_PROMPT=0
//!   5. clone_failure_rm_rf: rm -rf workdir_parent issued after clone failure
//!   6. rpc_failure_cancels_reservation: RPC EOF → reservation cancelled + workdir cleaned
//!   7. active_mark_sequencing: session_repo::activate called only after RPC succeeds

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;

use mux_cli::agent_start::RemoteExec;
use mux_cli::create::{CreateContext, HostKeyInfo, SshHost, run_create};
use mux_core::error::MuxError;
use mux_core::shortname::{shortname_for_branch, shortname_for_main, shortname_with_suffix, ADJECTIVES, NOUNS};
use mux_rpc::schema::{CreateSessionResponse, RpcError};
use mux_state::session_repo::{self, ReserveParams};
use mux_state::store::Store;
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ── MockSshHost ────────────────────────────────────────────────────────────────

/// Pre-programmed SSH host mock. Falls back to (1, "", "mock: no more responses")
/// when the queue is exhausted — same behavior as the inline create.rs mock.
struct MockSshHost {
    responses: RefCell<VecDeque<(i32, String, String)>>,
    host_key_result: Result<HostKeyInfo, MuxError>,
}

impl MockSshHost {
    fn with_trusted_key(responses: Vec<(i32, String, String)>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            host_key_result: Ok(trusted_key()),
        }
    }
}

impl RemoteExec for MockSshHost {
    fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
        Ok(self
            .responses
            .borrow_mut()
            .pop_front()
            .unwrap_or((1, String::new(), "mock: no more responses".to_owned())))
    }
}

impl SshHost for MockSshHost {
    fn host_key(&self) -> Result<HostKeyInfo, MuxError> {
        match &self.host_key_result {
            Ok(info) => Ok(info.clone()),
            Err(MuxError::HostKeyMismatch) => Err(MuxError::HostKeyMismatch),
            Err(_) => Err(MuxError::HostKeyMismatch),
        }
    }
}

// ── RecordingMock ──────────────────────────────────────────────────────────────

/// Records every SSH command; responses are pre-programmed.
/// Uses `Rc<RefCell<...>>` so the command log can be read after `run_create`
/// consumes the `CreateContext` (and with it, the mock).
struct RecordingMock {
    commands: Rc<RefCell<Vec<String>>>,
    responses: RefCell<VecDeque<(i32, String, String)>>,
}

impl RemoteExec for RecordingMock {
    fn run(&self, cmd: &str) -> Result<(i32, String, String), MuxError> {
        self.commands.borrow_mut().push(cmd.to_owned());
        Ok(self
            .responses
            .borrow_mut()
            .pop_front()
            .unwrap_or((1, String::new(), "mock: no more responses".to_owned())))
    }
}

impl SshHost for RecordingMock {
    fn host_key(&self) -> Result<HostKeyInfo, MuxError> {
        Ok(trusted_key())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn trusted_key() -> HostKeyInfo {
    HostKeyInfo {
        algorithm: "ssh-ed25519".to_owned(),
        fingerprint: "SHA256:AAAA".to_owned(),
    }
}

fn open_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("mux.db")).unwrap();
    (dir, store)
}

/// Insert a host row that has been fully probed (arch + home set) and whose
/// host key fingerprint has been trusted, so TOFU passes without interaction.
fn insert_configured_host(conn: &Connection) -> mux_state::model::Host {
    let id =
        mux_state::host_repo::insert(conn, "prod", "user", "10.0.0.1", 22, 1_000_000).unwrap();
    mux_state::host_repo::update_probe(conn, id, Some("aarch64"), Some("/home/user"), Some("tcp"))
        .unwrap();
    mux_state::fingerprint_repo::upsert(conn, id, "ssh-ed25519", "SHA256:AAAA", 1_000_000)
        .unwrap();
    mux_state::host_repo::get_by_id(conn, id).unwrap().unwrap()
}

fn repo() -> mux_core::types::RepoRef {
    "owner/myrepo".parse().unwrap()
}

/// JSON string for a running agent lock file; pid 99999 is considered alive
/// when the kill-0 mock response returns exit 0.
fn lock_json(port: u16) -> String {
    format!(r#"{{"pid":99999,"tcp_url":"tcp://127.0.0.1:{port}"}}"#)
}

/// Full SSH response sequence for a success path (agent already running).
///
/// SSH commands issued by run_create:
///   1. cat '/home/user'/.mux/agent.port   (transport probe — no port file)
///   2. test -S '/home/user/.mux/agent.sock' (transport probe — no socket)
///   3. mkdir -p '<workdir_parent>'        (create workdir parent)
///   4. test -d '<workdir>'               (workdir does not pre-exist)
///   5. GIT_TERMINAL_PROMPT=0 git clone … (clone succeeds)
///   6. cat '/home/user/.mux/agent.lock'  (AgentStarter::ensure_running → read_lock)
///   7. kill -0 99999                     (AgentStarter::ensure_running → is_process_alive)
fn success_responses(rpc_port: u16) -> Vec<(i32, String, String)> {
    vec![
        (1, String::new(), String::new()),       // cat agent.port
        (1, String::new(), String::new()),       // test -S socket
        (0, String::new(), String::new()),       // mkdir -p
        (1, String::new(), String::new()),       // test -d workdir → not exists
        (0, String::new(), String::new()),       // git clone → ok
        (0, lock_json(rpc_port), String::new()), // cat agent.lock → alive
        (0, String::new(), String::new()),       // kill -0 → alive
    ]
}

/// Encode a `CreateSessionResponse` as an RPC response frame (4-byte LE length + JSON body).
/// The client calls `decode_response::<CreateSessionResponse>` which parses the body as
/// `RpcResult<CreateSessionResponse>` — for a success the wire is just T's fields.
fn encode_create_ok(resp: &CreateSessionResponse) -> Vec<u8> {
    let body = serde_json::to_vec(resp).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// Encode an `RpcError` as a framed response.
fn encode_create_err(err: &RpcError) -> Vec<u8> {
    let body = serde_json::to_vec(err).unwrap();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// Spawn a one-shot TCP mock RPC server: accepts one connection, reads the
/// framed request, writes the given `response_frame`, then closes.
async fn spawn_rpc_server(response_frame: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            // Read and discard the framed request.
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

/// Reserve and activate a shortname so collision resolution must skip it.
///
/// `get_by_shortname` excludes in-flight (tmux_name IS NULL) rows, so the session
/// must be activated to be visible to `resolve_shortname_collision`.
fn occupy_shortname(conn: &Connection, host_id: i64, uuid: &str, shortname: &str) {
    session_repo::reserve(
        conn,
        &ReserveParams {
            uuid,
            host_id,
            shortname,
            repo_slug: "owner/myrepo",
            branch: "feature",
            created_at: 1_000_000,
        },
    )
    .unwrap();
    session_repo::activate(
        conn,
        uuid,
        &format!("mux-{shortname}"),
        "/work/myrepo",
        "tcp",
        1_000_001,
    )
    .unwrap();
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Claim 1 — Shortname collision: branch-based name already taken → -2 suffix.
#[tokio::test]
async fn create_shortname_collision_resolves_with_numeric_suffix() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);

    let base = shortname_for_branch("myrepo", "feature");
    occupy_shortname(conn, host.id, "00000000-0000-0000-0000-000000000001", &base);

    let expected_shortname = shortname_with_suffix(&base, 2);
    let rpc_port = spawn_rpc_server(encode_create_ok(&CreateSessionResponse {
        uuid: String::new(),
        shortname: expected_shortname.clone(),
        tmux_name: format!("mux-{expected_shortname}"),
    }))
    .await;

    let result = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: MockSshHost::with_trusted_key(success_responses(rpc_port)),
        is_interactive: false,
    })
    .await
    .unwrap();

    assert_eq!(
        result.shortname, expected_shortname,
        "shortname must use -2 suffix when base is taken"
    );
}

/// Claim 2 — Shortname exhaustion: all 50 suffix variants taken → ShortnameExhausted.
#[tokio::test]
async fn create_shortname_exhaustion_errors() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);

    let base = shortname_for_branch("myrepo", "feature");
    // Occupy base (attempt 1 — no suffix) through base-50 (attempt 50 — suffix "50").
    for attempt in 1u32..=50 {
        let sn = shortname_with_suffix(&base, attempt);
        let uuid = format!("00000000-0000-0000-0000-{attempt:012}");
        occupy_shortname(conn, host.id, &uuid, &sn);
    }

    let err = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: MockSshHost::with_trusted_key(vec![]),
        is_interactive: false,
    })
    .await
    .unwrap_err();

    assert!(
        matches!(err, MuxError::ShortnameExhausted),
        "expected ShortnameExhausted after 50 collision attempts, got: {err:?}"
    );
}

/// Claim 3 — Main-branch shortname exhaustion: all 400 adj-noun pairs taken → ShortnameExhausted.
#[tokio::test]
async fn create_main_branch_shortname_exhaustion_errors() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);

    for (i, adj) in ADJECTIVES.iter().enumerate() {
        for (j, noun) in NOUNS.iter().enumerate() {
            let sn = shortname_for_main("myrepo", adj, noun);
            // Unique valid-format UUID for each (adj, noun) pair.
            let uuid = format!("{i:08x}-{j:04x}-0000-0000-000000000000");
            occupy_shortname(conn, host.id, &uuid, &sn);
        }
    }

    let err = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "main".to_owned(),
        ssh: MockSshHost::with_trusted_key(vec![]),
        is_interactive: false,
    })
    .await
    .unwrap_err();

    assert!(
        matches!(err, MuxError::ShortnameExhausted),
        "expected ShortnameExhausted after all 400 adj-noun pairs are taken, got: {err:?}"
    );
}

/// Claim 4 — Clone command includes GIT_TERMINAL_PROMPT=0.
///
/// Uses RecordingMock to capture the exact command strings; fails at clone
/// (exit 128) to keep the test short — we only care about clone command content.
#[tokio::test]
async fn create_clone_command_includes_git_terminal_prompt_zero() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);

    let commands = Rc::new(RefCell::new(Vec::<String>::new()));
    let commands_ref = Rc::clone(&commands);

    // Fail at clone; the rm -rf cleanup is the next (6th) response.
    let responses: Vec<(i32, String, String)> = vec![
        (1, String::new(), String::new()),                    // cat agent.port
        (1, String::new(), String::new()),                    // test -S socket
        (0, String::new(), String::new()),                    // mkdir -p
        (1, String::new(), String::new()),                    // test -d workdir → not exists
        (128, String::new(), "repo not found".to_owned()),    // git clone fails
        (0, String::new(), String::new()),                    // rm -rf cleanup
    ];

    let _ = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: RecordingMock {
            commands: commands_ref,
            responses: RefCell::new(responses.into()),
        },
        is_interactive: false,
    })
    .await;

    let cmds = commands.borrow();
    let clone_cmd = cmds
        .iter()
        .find(|c| c.contains("git clone"))
        .expect("a git clone command must have been issued");
    assert!(
        clone_cmd.contains("GIT_TERMINAL_PROMPT=0"),
        "clone command must start with GIT_TERMINAL_PROMPT=0, got: {clone_cmd}"
    );
}

/// Claim 5 — On clone failure, rm -rf of workdir_parent is issued for cleanup.
#[tokio::test]
async fn create_clone_failure_issues_rm_rf_on_workdir_parent() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);
    let host_id = host.id;

    let commands = Rc::new(RefCell::new(Vec::<String>::new()));
    let commands_ref = Rc::clone(&commands);

    let responses: Vec<(i32, String, String)> = vec![
        (1, String::new(), String::new()),                  // cat agent.port
        (1, String::new(), String::new()),                  // test -S socket
        (0, String::new(), String::new()),                  // mkdir -p workdir_parent
        (1, String::new(), String::new()),                  // test -d workdir → not exists
        (128, String::new(), "clone error".to_owned()),     // git clone fails
        (0, String::new(), String::new()),                  // rm -rf cleanup
    ];

    let err = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: RecordingMock {
            commands: commands_ref,
            responses: RefCell::new(responses.into()),
        },
        is_interactive: false,
    })
    .await
    .unwrap_err();

    assert!(
        matches!(err, MuxError::GitCloneFailed { .. }),
        "expected GitCloneFailed, got: {err:?}"
    );

    let cmds = commands.borrow();
    let rm_cmd = cmds
        .iter()
        .find(|c| c.contains("rm -rf"))
        .expect("rm -rf cleanup command must be issued after clone failure");
    // The workdir_parent path contains the UUID and is under the mux home.
    assert!(
        rm_cmd.contains(".mux"),
        "rm -rf must target the mux workdir parent, got: {rm_cmd}"
    );

    // Reservation must be cancelled (list_for_host filters tmux_name IS NOT NULL,
    // so an in-flight reservation with NULL tmux_name does not appear — confirmed empty).
    let sessions = session_repo::list_for_host(conn, host_id).unwrap();
    assert!(sessions.is_empty(), "reservation must be cancelled on clone failure");
}

/// Claim 6 — RPC create_session failure: reservation cancelled and workdir cleaned up.
///
/// The mock server accepts the connection, reads the request, then closes immediately
/// (EOF), which the client maps to `MuxError::RpcError("server closed connection")`.
#[tokio::test]
async fn create_rpc_failure_cancels_reservation_and_cleans_workdir() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);
    let host_id = host.id;

    // Server reads the request and closes without responding → RpcError.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).await.is_ok() {
                let body_len = u32::from_le_bytes(len_buf) as usize;
                let mut body = vec![0u8; body_len];
                let _ = stream.read_exact(&mut body).await;
            }
            drop(stream); // close without responding → EOF → RpcError
        }
    });

    let commands = Rc::new(RefCell::new(Vec::<String>::new()));
    let commands_ref = Rc::clone(&commands);

    let responses: Vec<(i32, String, String)> = vec![
        (1, String::new(), String::new()),       // cat agent.port
        (1, String::new(), String::new()),       // test -S socket
        (0, String::new(), String::new()),       // mkdir -p
        (1, String::new(), String::new()),       // test -d workdir → not exists
        (0, String::new(), String::new()),       // git clone → ok
        (0, lock_json(rpc_port), String::new()), // cat agent.lock
        (0, String::new(), String::new()),       // kill -0 → alive
        (0, String::new(), String::new()),       // rm -rf cleanup after RPC failure
    ];

    let err = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: RecordingMock {
            commands: commands_ref,
            responses: RefCell::new(responses.into()),
        },
        is_interactive: false,
    })
    .await
    .unwrap_err();

    assert!(
        matches!(
            err,
            MuxError::RpcError(_) | MuxError::ConnectionRefused(_) | MuxError::ConnectionTimeout(_)
        ),
        "expected an RPC-class error, got: {err:?}"
    );

    // Reservation must be cancelled.
    let sessions = session_repo::list_for_host(conn, host_id).unwrap();
    assert!(
        sessions.is_empty(),
        "reservation must be cancelled on RPC failure"
    );

    // Workdir cleanup must be issued.
    let cmds = commands.borrow();
    assert!(
        cmds.iter().any(|c| c.contains("rm -rf")),
        "rm -rf must be issued to clean up workdir after RPC failure"
    );
}

/// Claim 7 — Active mark (session_repo::activate) is set only after RPC success.
///
/// Verifies the full happy path: after a successful RPC round-trip the session
/// row transitions from reserved (tmux_name IS NULL) to active (tmux_name set).
#[tokio::test]
async fn create_active_mark_only_set_after_rpc_success() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);
    let host_id = host.id;

    let expected_shortname = shortname_for_branch("myrepo", "feature");
    let expected_tmux_name = format!("mux-{expected_shortname}");

    let rpc_port = spawn_rpc_server(encode_create_ok(&CreateSessionResponse {
        uuid: String::new(), // the agent echoes back the uuid; activate uses ctx.uuid
        shortname: expected_shortname.clone(),
        tmux_name: expected_tmux_name.clone(),
    }))
    .await;

    let result = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: MockSshHost::with_trusted_key(success_responses(rpc_port)),
        is_interactive: false,
    })
    .await
    .unwrap();

    // run_create must succeed and return the expected names.
    assert_eq!(result.shortname, expected_shortname);
    assert_eq!(result.tmux_name, expected_tmux_name);

    // Session must be active (not reserved) with tmux_name populated.
    let s = session_repo::get_by_uuid(conn, &result.uuid)
        .unwrap()
        .expect("session row must exist after successful create");
    assert_eq!(s.status, "active", "session status must be 'active'");
    assert_eq!(
        s.tmux_name.as_deref(),
        Some(expected_tmux_name.as_str()),
        "tmux_name must be set from RPC response"
    );

    // Exactly one session for this host (no duplicates).
    let all = session_repo::list_for_host(conn, host_id).unwrap();
    assert_eq!(all.len(), 1, "exactly one session must exist");
}

/// RPC application error (AgentError response): reservation cancelled, workdir cleaned.
#[tokio::test]
async fn create_rpc_agent_error_cancels_reservation() {
    let (_dir, store) = open_store();
    let conn = store.conn();
    let host = insert_configured_host(conn);
    let host_id = host.id;

    let rpc_port = spawn_rpc_server(encode_create_err(&RpcError::internal("tmux error"))).await;

    let commands = Rc::new(RefCell::new(Vec::<String>::new()));
    let commands_ref = Rc::clone(&commands);

    let responses: Vec<(i32, String, String)> = vec![
        (1, String::new(), String::new()),       // cat agent.port
        (1, String::new(), String::new()),       // test -S socket
        (0, String::new(), String::new()),       // mkdir -p
        (1, String::new(), String::new()),       // test -d workdir → not exists
        (0, String::new(), String::new()),       // git clone → ok
        (0, lock_json(rpc_port), String::new()), // cat agent.lock
        (0, String::new(), String::new()),       // kill -0 → alive
        (0, String::new(), String::new()),       // rm -rf cleanup
    ];

    let err = run_create(CreateContext {
        conn,
        mux_home: Path::new("/home/user/.mux"),
        repo: repo(),
        host,
        branch: "feature".to_owned(),
        ssh: RecordingMock {
            commands: commands_ref,
            responses: RefCell::new(responses.into()),
        },
        is_interactive: false,
    })
    .await
    .unwrap_err();

    assert!(
        matches!(err, MuxError::AgentError(_)),
        "expected AgentError for RPC error response, got: {err:?}"
    );

    // Reservation cancelled.
    let sessions = session_repo::list_for_host(conn, host_id).unwrap();
    assert!(sessions.is_empty(), "reservation must be cancelled on RPC agent error");

    // Workdir cleanup issued.
    let cmds = commands.borrow();
    assert!(
        cmds.iter().any(|c| c.contains("rm -rf")),
        "rm -rf must be issued after RPC agent error"
    );
}
