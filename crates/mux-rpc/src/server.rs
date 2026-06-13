//! RPC server — TCP listener with length-prefix framing.
//!
//! Wire format: [u32 LE length][UTF-8 JSON body] in each direction.
//! All requests carry an `"op"` tag for dispatch (see `schema::Request`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use mux_tmux::adapter::{SessionInfo as TmuxSessionInfo, TmuxAdapter, TmuxError};

use crate::schema::{
    CreateSessionResponse, GetSessionResponse, HealthResponse, KillSessionResponse,
    ListSessionsResponse, RpcError, RpcResult, Request, SessionInfo, SessionStatusValue,
    ShutdownResponse,
};

// ── Codec constants ───────────────────────────────────────────────────────────

/// Maximum frame body size (4 MiB). Rejects attacker-controlled length prefixes
/// before allocating, preventing OOM from a forged large-frame attack.
const MAX_FRAME_LEN: usize = 4 * 1024 * 1024;

// ── TmuxOps trait ─────────────────────────────────────────────────────────────

/// Boxed future alias used by `TmuxOps` methods.
type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Server-internal interface for tmux operations. Implemented by `TmuxAdapter`
/// in production and by `MockTmuxOps` in tests.
///
/// Uses boxed futures so the `Send` bound is explicit at the trait level,
/// allowing `handle_connection<T: TmuxOps>` to be spawned on a `tokio` thread pool.
trait TmuxOps: Send + Sync + 'static {
    /// Create a new detached tmux session `name` rooted at `workdir`.
    fn new_session<'a>(
        &'a self,
        name: &'a str,
        workdir: &'a str,
    ) -> BoxFut<'a, Result<(), TmuxError>>;

    /// Kill the tmux session named `name`.
    fn kill_session<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<(), TmuxError>>;

    /// Return all mux-prefixed tmux sessions visible to this adapter.
    fn list_sessions(&self) -> BoxFut<'_, Result<Vec<TmuxSessionInfo>, TmuxError>>;
}

impl TmuxOps for TmuxAdapter {
    fn new_session<'a>(
        &'a self,
        name: &'a str,
        workdir: &'a str,
    ) -> BoxFut<'a, Result<(), TmuxError>> {
        // status_right is always None for agent-created sessions (spec §CreateSession).
        Box::pin(TmuxAdapter::new_session(self, name, workdir, None))
    }

    fn kill_session<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<(), TmuxError>> {
        Box::pin(TmuxAdapter::kill_session(self, name))
    }

    fn list_sessions(&self) -> BoxFut<'_, Result<Vec<TmuxSessionInfo>, TmuxError>> {
        Box::pin(TmuxAdapter::list_sessions(self))
    }
}

// ── Ownership map ─────────────────────────────────────────────────────────────

struct OwnedSession {
    shortname: String,
    tmux_name: String,
    repo_slug: String,
    workdir: String,
    /// True for sessions created via CreateSession (agent manages workdir removal).
    /// False for imported sessions (workdir must never be deleted by the agent).
    mux_created: bool,
}

type OwnershipMap = Arc<Mutex<HashMap<String, OwnedSession>>>;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct RpcServer<T = TmuxAdapter> {
    tmux: T,
    bind_addr: String,
}

pub struct BoundRpcServer<T = TmuxAdapter> {
    listener: TcpListener,
    tmux: Arc<T>,
    ownership: OwnershipMap,
    pub shutdown_flag: Arc<AtomicBool>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl RpcServer {
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            tmux: TmuxAdapter::new(),
            bind_addr: bind_addr.into(),
        }
    }

    pub fn new_with_tmux(bind_addr: impl Into<String>, tmux: TmuxAdapter) -> Self {
        Self {
            tmux,
            bind_addr: bind_addr.into(),
        }
    }
}

impl<T: TmuxOps> RpcServer<T> {
    pub fn with_backend(bind_addr: impl Into<String>, tmux: T) -> Self {
        Self {
            tmux,
            bind_addr: bind_addr.into(),
        }
    }

    pub async fn bind(self) -> anyhow::Result<BoundRpcServer<T>> {
        let listener = TcpListener::bind(&self.bind_addr).await?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Ok(BoundRpcServer {
            listener,
            tmux: Arc::new(self.tmux),
            ownership: Arc::new(Mutex::new(HashMap::new())),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_tx,
            shutdown_rx,
        })
    }
}

impl<T: TmuxOps> BoundRpcServer<T> {
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.listener.local_addr().expect("listener has a local addr")
    }

    pub async fn serve(mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                result = self.listener.accept() => {
                    match result {
                        Ok((stream, peer)) => {
                            tracing::debug!(%peer, "accepted connection");
                            let tmux = Arc::clone(&self.tmux);
                            let ownership = Arc::clone(&self.ownership);
                            let shutdown_flag = Arc::clone(&self.shutdown_flag);
                            let shutdown_tx = self.shutdown_tx.clone();
                            tokio::spawn(async move {
                                handle_connection(stream, tmux, ownership, shutdown_flag, shutdown_tx).await;
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "accept error");
                        }
                    }
                }
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        tracing::info!("shutdown signal received, stopping accept loop");
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

// ── Codec helpers ─────────────────────────────────────────────────────────────

async fn read_message(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    anyhow::ensure!(
        len <= MAX_FRAME_LEN,
        "frame too large: {len} bytes (max {MAX_FRAME_LEN})"
    );
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await?;
    Ok(body)
}

async fn write_message(stream: &mut TcpStream, body: &[u8]) -> anyhow::Result<()> {
    let len = u32::try_from(body.len())
        .map_err(|_| anyhow::anyhow!("response body too large to frame: {} bytes", body.len()))?;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}

// ── Connection handler ────────────────────────────────────────────────────────

async fn handle_connection<T: TmuxOps>(
    mut stream: TcpStream,
    tmux: Arc<T>,
    ownership: OwnershipMap,
    shutdown_flag: Arc<AtomicBool>,
    shutdown_tx: watch::Sender<bool>,
) {
    loop {
        let raw = match read_message(&mut stream).await {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(error = %e, "connection read error, dropping");
                return;
            }
        };

        let request: Request = match serde_json::from_slice(&raw) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "failed to parse request, dropping connection");
                return;
            }
        };

        // Set to true when Shutdown is dispatched; triggers watch signal after response.
        let mut trigger_shutdown = false;

        let response_bytes: Vec<u8> = match request {
            Request::Health(_) => {
                let resp: RpcResult<HealthResponse> = RpcResult::Ok(HealthResponse { ok: true });
                serde_json::to_vec(&resp).unwrap_or_default()
            }

            Request::CreateSession(req) => {
                if shutdown_flag.load(Ordering::SeqCst) {
                    let err: RpcResult<CreateSessionResponse> =
                        RpcResult::Err(RpcError::internal("agent is shutting down"));
                    serde_json::to_vec(&err).unwrap_or_default()
                } else {
                    let tmux_name = format!("mux-{}", req.shortname);
                    let workdir = format!("{}/{}", req.workdir_parent, req.repo_leaf);

                    match tmux.new_session(&tmux_name, &workdir).await {
                        Err(e) => {
                            let err: RpcResult<CreateSessionResponse> =
                                RpcResult::Err(RpcError::tmux_error(e.to_string()));
                            serde_json::to_vec(&err).unwrap_or_default()
                        }
                        Ok(()) => {
                            {
                                let mut map = ownership.lock().unwrap();
                                map.insert(
                                    req.uuid.clone(),
                                    OwnedSession {
                                        shortname: req.shortname.clone(),
                                        tmux_name: tmux_name.clone(),
                                        repo_slug: req.repo_slug,
                                        workdir,
                                        mux_created: true,
                                    },
                                );
                            }
                            let resp: RpcResult<CreateSessionResponse> =
                                RpcResult::Ok(CreateSessionResponse {
                                    uuid: req.uuid,
                                    shortname: req.shortname,
                                    tmux_name,
                                });
                            serde_json::to_vec(&resp).unwrap_or_default()
                        }
                    }
                }
            }

            Request::ListSessions(_) => {
                match tmux.list_sessions().await {
                    Err(e) => {
                        let err: RpcResult<ListSessionsResponse> =
                            RpcResult::Err(RpcError::tmux_error(e.to_string()));
                        serde_json::to_vec(&err).unwrap_or_default()
                    }
                    Ok(live) => {
                        let live_names: std::collections::HashSet<String> =
                            live.into_iter().map(|s| s.name).collect();

                        let sessions: Vec<SessionInfo> = {
                            let map = ownership.lock().unwrap();
                            map.iter()
                                .map(|(uuid, entry)| {
                                    let status = if live_names.contains(&entry.tmux_name) {
                                        SessionStatusValue::Active
                                    } else {
                                        SessionStatusValue::Dead
                                    };
                                    SessionInfo {
                                        uuid: uuid.clone(),
                                        shortname: entry.shortname.clone(),
                                        tmux_name: entry.tmux_name.clone(),
                                        workdir: entry.workdir.clone(),
                                        status,
                                    }
                                })
                                .collect()
                        };
                        let resp: RpcResult<ListSessionsResponse> =
                            RpcResult::Ok(ListSessionsResponse { sessions });
                        serde_json::to_vec(&resp).unwrap_or_default()
                    }
                }
            }

            Request::GetSession(req) => {
                let entry_info = {
                    let map = ownership.lock().unwrap();
                    map.get(&req.uuid).map(|e| (e.shortname.clone(), e.tmux_name.clone()))
                };

                match entry_info {
                    None => {
                        let err: RpcResult<GetSessionResponse> =
                            RpcResult::Err(RpcError::not_found(format!(
                                "session {} not found",
                                req.uuid
                            )));
                        serde_json::to_vec(&err).unwrap_or_default()
                    }
                    Some((shortname, tmux_name)) => {
                        match tmux.list_sessions().await {
                            Err(e) => {
                                let err: RpcResult<GetSessionResponse> =
                                    RpcResult::Err(RpcError::tmux_error(e.to_string()));
                                serde_json::to_vec(&err).unwrap_or_default()
                            }
                            Ok(live) => {
                                let live_names: std::collections::HashSet<String> =
                                    live.into_iter().map(|s| s.name).collect();
                                let status = if live_names.contains(&tmux_name) {
                                    SessionStatusValue::Active
                                } else {
                                    SessionStatusValue::Dead
                                };
                                let resp: RpcResult<GetSessionResponse> =
                                    RpcResult::Ok(GetSessionResponse {
                                        uuid: req.uuid,
                                        shortname,
                                        tmux_name,
                                        status,
                                    });
                                serde_json::to_vec(&resp).unwrap_or_default()
                            }
                        }
                    }
                }
            }

            Request::KillSession(req) => {
                let entry_info = {
                    let map = ownership.lock().unwrap();
                    map.get(&req.uuid).map(|e| {
                        (e.tmux_name.clone(), e.repo_slug.clone(), e.workdir.clone(), e.mux_created)
                    })
                };

                match entry_info {
                    None => {
                        let err: RpcResult<KillSessionResponse> =
                            RpcResult::Err(RpcError::not_owned("session not in ownership map"));
                        serde_json::to_vec(&err).unwrap_or_default()
                    }
                    Some((tmux_name, repo_slug, workdir, mux_created)) => {
                        if req.repo_slug != repo_slug {
                            let err: RpcResult<KillSessionResponse> =
                                RpcResult::Err(RpcError::not_owned("repo_slug mismatch"));
                            serde_json::to_vec(&err).unwrap_or_default()
                        } else {
                            let tmux_killed = match tmux.kill_session(&tmux_name).await {
                                Ok(()) => true,
                                Err(e) => {
                                    // Session already gone counts as killed.
                                    let msg = e.to_string().to_ascii_lowercase();
                                    if msg.contains("no server running")
                                        || msg.contains("no sessions")
                                        || msg.contains("session not found")
                                        || msg.contains("can't find session")
                                    {
                                        true
                                    } else {
                                        tracing::warn!(error = %e, "kill_session tmux error");
                                        false
                                    }
                                }
                            };

                            // Only remove mux-created workdirs; never touch imported ones.
                            let workdir_removed = if mux_created {
                                let workdir_clone = workdir.clone();
                                match tokio::task::spawn_blocking(move || {
                                    std::fs::remove_dir_all(&workdir_clone)
                                })
                                .await
                                {
                                    Ok(Ok(())) => true,
                                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => false,
                                    Ok(Err(e)) => {
                                        tracing::warn!(error = %e, path = %workdir, "remove_dir_all failed");
                                        false
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "spawn_blocking join error");
                                        false
                                    }
                                }
                            } else {
                                false
                            };

                            {
                                let mut map = ownership.lock().unwrap();
                                map.remove(&req.uuid);
                            }

                            let resp: RpcResult<KillSessionResponse> =
                                RpcResult::Ok(KillSessionResponse {
                                    tmux_killed,
                                    workdir_removed,
                                });
                            serde_json::to_vec(&resp).unwrap_or_default()
                        }
                    }
                }
            }

            Request::Shutdown(_) => {
                shutdown_flag.store(true, Ordering::SeqCst);
                trigger_shutdown = true;
                let resp: RpcResult<ShutdownResponse> = RpcResult::Ok(ShutdownResponse {});
                serde_json::to_vec(&resp).unwrap_or_default()
            }

            Request::StreamSessionEvents(_) => {
                let err: RpcResult<crate::schema::ShutdownResponse> =
                    RpcResult::Err(RpcError::internal("streaming not implemented"));
                serde_json::to_vec(&err).unwrap_or_default()
            }
        };

        if let Err(e) = write_message(&mut stream, &response_bytes).await {
            tracing::debug!(error = %e, "failed to write response");
            return;
        }

        if trigger_shutdown {
            let _ = shutdown_tx.send(true);
            return;
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mux_tmux::adapter::TmuxError;
    use std::sync::atomic::AtomicUsize;

    // ── MockTmuxOps ───────────────────────────────────────────────────────────

    /// Configurable tmux backend for unit tests. All fields are `Send + Sync`.
    struct MockTmuxOps {
        /// Sessions returned by list_sessions.
        live_sessions: Arc<Mutex<Vec<TmuxSessionInfo>>>,
        /// Error to return from new_session (None = Ok(())).
        new_session_error: Option<String>,
        /// Error to return from kill_session (None = Ok(())).
        kill_session_error: Option<String>,
        /// Calls recorded for new_session: (name, workdir).
        new_session_calls: Arc<Mutex<Vec<(String, String)>>>,
        /// Calls recorded for kill_session: name.
        kill_session_calls: Arc<Mutex<Vec<String>>>,
        /// Incremented each time list_sessions is called.
        list_sessions_count: Arc<AtomicUsize>,
    }

    impl MockTmuxOps {
        fn new() -> Self {
            Self {
                live_sessions: Arc::new(Mutex::new(Vec::new())),
                new_session_error: None,
                kill_session_error: None,
                new_session_calls: Arc::new(Mutex::new(Vec::new())),
                kill_session_calls: Arc::new(Mutex::new(Vec::new())),
                list_sessions_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_live_sessions(mut self, sessions: Vec<TmuxSessionInfo>) -> Self {
            *self.live_sessions.lock().unwrap() = sessions;
            self
        }

        fn with_new_session_error(mut self, msg: impl Into<String>) -> Self {
            self.new_session_error = Some(msg.into());
            self
        }

        fn with_kill_session_error(mut self, msg: impl Into<String>) -> Self {
            self.kill_session_error = Some(msg.into());
            self
        }
    }

    fn make_tmux_session(name: &str) -> TmuxSessionInfo {
        TmuxSessionInfo {
            name: name.to_owned(),
            created: 1_700_000_000,
            activity: 1_700_000_001,
        }
    }

    impl TmuxOps for MockTmuxOps {
        fn new_session<'a>(
            &'a self,
            name: &'a str,
            workdir: &'a str,
        ) -> BoxFut<'a, Result<(), TmuxError>> {
            self.new_session_calls
                .lock()
                .unwrap()
                .push((name.to_owned(), workdir.to_owned()));
            let result = if let Some(ref msg) = self.new_session_error {
                Err(TmuxError::TmuxFailed {
                    command: vec!["tmux".to_owned()],
                    exit_code: Some(1),
                    stderr: msg.clone(),
                })
            } else {
                Ok(())
            };
            Box::pin(async move { result })
        }

        fn kill_session<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<(), TmuxError>> {
            self.kill_session_calls.lock().unwrap().push(name.to_owned());
            let result = if let Some(ref msg) = self.kill_session_error {
                Err(TmuxError::TmuxFailed {
                    command: vec!["tmux".to_owned()],
                    exit_code: Some(1),
                    stderr: msg.clone(),
                })
            } else {
                Ok(())
            };
            Box::pin(async move { result })
        }

        fn list_sessions(&self) -> BoxFut<'_, Result<Vec<TmuxSessionInfo>, TmuxError>> {
            self.list_sessions_count.fetch_add(1, Ordering::SeqCst);
            let sessions = self.live_sessions.lock().unwrap().clone();
            Box::pin(async move { Ok(sessions) })
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    async fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client_res, server_res) =
            tokio::join!(TcpStream::connect(addr), listener.accept());
        (client_res.unwrap(), server_res.unwrap().0)
    }

    async fn start_mock_server(mock: MockTmuxOps) -> std::net::SocketAddr {
        let server = RpcServer::with_backend("127.0.0.1:0", mock);
        let bound = server.bind().await.unwrap();
        let addr = bound.local_addr();
        tokio::spawn(async move {
            let _ = bound.serve().await;
        });
        addr
    }

    async fn start_test_server() -> std::net::SocketAddr {
        start_mock_server(MockTmuxOps::new()).await
    }

    async fn send_request(stream: &mut TcpStream, req: &serde_json::Value) -> serde_json::Value {
        let bytes = serde_json::to_vec(req).unwrap();
        write_message(stream, &bytes).await.unwrap();
        let resp_bytes = read_message(stream).await.unwrap();
        serde_json::from_slice(&resp_bytes).unwrap()
    }

    // ── Codec ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_write_message_roundtrip() {
        let (mut client, mut server) = loopback_pair().await;
        let payload = b"hello, world";
        write_message(&mut client, payload).await.unwrap();
        let received = read_message(&mut server).await.unwrap();
        assert_eq!(received, payload);
    }

    #[tokio::test]
    async fn read_write_message_empty_body() {
        let (mut client, mut server) = loopback_pair().await;
        write_message(&mut client, b"").await.unwrap();
        let received = read_message(&mut server).await.unwrap();
        assert_eq!(received, b"");
    }

    #[tokio::test]
    async fn read_write_message_large_payload() {
        let (mut client, mut server) = loopback_pair().await;
        let payload: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();
        write_message(&mut client, &payload).await.unwrap();
        let received = read_message(&mut server).await.unwrap();
        assert_eq!(received, payload);
    }

    // ── Health ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_request_returns_ok_true() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(&mut stream, &serde_json::json!({"op": "Health"})).await;
        assert_eq!(resp["ok"], true);
        assert!(resp.get("error").is_none());
    }

    // ── StreamSessionEvents ───────────────────────────────────────────────────

    #[tokio::test]
    async fn stream_session_events_returns_internal_error() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "StreamSessionEvents"}),
        )
        .await;
        assert_eq!(resp["error"], "internal");
        assert!(
            resp["message"].as_str().unwrap().contains("streaming"),
            "message should mention 'streaming', got: {}",
            resp["message"]
        );
    }

    // ── CreateSession ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_session_after_shutdown_returns_internal_error() {
        let server = RpcServer::with_backend("127.0.0.1:0", MockTmuxOps::new());
        let bound = server.bind().await.unwrap();
        let addr = bound.local_addr();
        bound.shutdown_flag.store(true, Ordering::SeqCst);
        tokio::spawn(async move {
            let _ = bound.serve().await;
        });

        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "test-uuid",
                "shortname": "test",
                "repo_slug": "test-repo",
                "branch": "main",
                "workdir_parent": "/tmp/mux-test",
                "repo_leaf": "repo"
            }),
        )
        .await;
        assert_eq!(resp["error"], "internal");
        assert!(resp["message"].as_str().unwrap().contains("shutting down"));
    }

    #[tokio::test]
    async fn create_session_tmux_success_returns_correct_fields() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "uuid-1",
                "shortname": "my-session",
                "repo_slug": "my-repo",
                "branch": "main",
                "workdir_parent": "/home/user/.mux/uuid-1",
                "repo_leaf": "my-repo"
            }),
        )
        .await;
        assert!(resp.get("error").is_none(), "expected success, got error: {resp}");
        assert_eq!(resp["uuid"], "uuid-1");
        assert_eq!(resp["shortname"], "my-session");
        // tmux_name must be "mux-<shortname>"
        assert_eq!(resp["tmux_name"], "mux-my-session");
    }

    #[tokio::test]
    async fn create_session_tmux_name_has_mux_prefix() {
        let mock = MockTmuxOps::new();
        let calls = Arc::clone(&mock.new_session_calls);
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "u1",
                "shortname": "foo",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp/p",
                "repo_leaf": "leaf"
            }),
        )
        .await;
        let issued = calls.lock().unwrap();
        assert_eq!(issued.len(), 1, "new_session called once");
        assert_eq!(issued[0].0, "mux-foo", "tmux session name must be 'mux-<shortname>'");
    }

    #[tokio::test]
    async fn create_session_workdir_is_parent_slash_leaf() {
        let mock = MockTmuxOps::new();
        let calls = Arc::clone(&mock.new_session_calls);
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "u2",
                "shortname": "s",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/home/user/.mux/u2",
                "repo_leaf": "my-project"
            }),
        )
        .await;
        let issued = calls.lock().unwrap();
        assert_eq!(
            issued[0].1, "/home/user/.mux/u2/my-project",
            "workdir must be workdir_parent/repo_leaf"
        );
    }

    #[tokio::test]
    async fn create_session_tmux_failure_returns_tmux_error() {
        let mock = MockTmuxOps::new().with_new_session_error("tmux: session creation failed");
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "u3",
                "shortname": "s",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;
        assert_eq!(resp["error"], "tmux_error");
    }

    #[tokio::test]
    async fn create_session_unknown_field_drops_connection() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        // Send a CreateSession with an unknown field — `deny_unknown_fields` causes parse error.
        // The server drops the connection without sending a response.
        let bytes = serde_json::to_vec(&serde_json::json!({
            "op": "CreateSession",
            "uuid": "u",
            "shortname": "s",
            "repo_slug": "r",
            "branch": "main",
            "workdir_parent": "/tmp",
            "repo_leaf": "r",
            "unknown_field": "rejected"
        }))
        .unwrap();
        write_message(&mut stream, &bytes).await.unwrap();
        // Connection is dropped by server — read should return EOF.
        let result = read_message(&mut stream).await;
        assert!(result.is_err(), "server should have dropped the connection");
    }

    // ── ListSessions ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_empty_ownership_map_returns_empty_vec() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(&mut stream, &serde_json::json!({"op": "ListSessions"})).await;
        assert!(resp.get("error").is_none(), "expected success, got: {resp}");
        assert_eq!(
            resp["sessions"].as_array().unwrap().len(),
            0,
            "no sessions in map → empty list"
        );
    }

    #[tokio::test]
    async fn list_sessions_registered_session_appears_as_active() {
        // Create a session, then confirm it appears in ListSessions as Active
        // (mock returns it in the live list).
        let mock = MockTmuxOps::new()
            .with_live_sessions(vec![make_tmux_session("mux-proj")]);
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        // Register the session first.
        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "list-uuid",
                "shortname": "proj",
                "repo_slug": "my-repo",
                "branch": "main",
                "workdir_parent": "/tmp/mux/list-uuid",
                "repo_leaf": "repo"
            }),
        )
        .await;

        let resp = send_request(&mut stream, &serde_json::json!({"op": "ListSessions"})).await;
        let sessions = resp["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "one session in map");
        assert_eq!(sessions[0]["uuid"], "list-uuid");
        assert_eq!(sessions[0]["shortname"], "proj");
        assert_eq!(sessions[0]["tmux_name"], "mux-proj");
        assert_eq!(sessions[0]["status"], "active");
    }

    #[tokio::test]
    async fn list_sessions_registered_session_is_dead_when_not_in_tmux() {
        // tmux reports no live sessions → owned session is Dead.
        let mock = MockTmuxOps::new(); // empty live_sessions
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "dead-uuid",
                "shortname": "dead",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;

        let resp = send_request(&mut stream, &serde_json::json!({"op": "ListSessions"})).await;
        let sessions = resp["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["status"], "dead");
    }

    // ── GetSession ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_session_unknown_uuid_returns_not_found() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "GetSession", "uuid": "nonexistent-uuid-xxxx"}),
        )
        .await;
        assert_eq!(resp["error"], "not_found");
    }

    #[tokio::test]
    async fn get_session_returns_active_when_tmux_reports_live() {
        let mock = MockTmuxOps::new()
            .with_live_sessions(vec![make_tmux_session("mux-alive")]);
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "alive-uuid",
                "shortname": "alive",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;

        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "GetSession", "uuid": "alive-uuid"}),
        )
        .await;
        assert!(resp.get("error").is_none(), "expected success, got: {resp}");
        assert_eq!(resp["uuid"], "alive-uuid");
        assert_eq!(resp["shortname"], "alive");
        assert_eq!(resp["tmux_name"], "mux-alive");
        assert_eq!(resp["status"], "active");
    }

    #[tokio::test]
    async fn get_session_returns_dead_when_tmux_does_not_list_it() {
        let mock = MockTmuxOps::new(); // empty live sessions
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "gone-uuid",
                "shortname": "gone",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;

        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "GetSession", "uuid": "gone-uuid"}),
        )
        .await;
        assert!(resp.get("error").is_none(), "expected success, got: {resp}");
        assert_eq!(resp["status"], "dead");
    }

    // ── KillSession ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn kill_session_unknown_uuid_returns_not_owned() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(
            &mut stream,
            &serde_json::json!({
                "op": "KillSession",
                "uuid": "no-such-uuid",
                "repo_slug": "any-repo"
            }),
        )
        .await;
        assert_eq!(resp["error"], "not_owned");
        assert!(
            resp["message"].as_str().unwrap().contains("ownership map"),
            "message should mention ownership map, got: {}",
            resp["message"]
        );
    }

    #[tokio::test]
    async fn kill_session_repo_slug_mismatch_returns_not_owned() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        // Create with repo_slug "correct-repo".
        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "kill-uuid",
                "shortname": "s",
                "repo_slug": "correct-repo",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;

        // Kill with wrong repo_slug.
        let resp = send_request(
            &mut stream,
            &serde_json::json!({
                "op": "KillSession",
                "uuid": "kill-uuid",
                "repo_slug": "wrong-repo"
            }),
        )
        .await;
        assert_eq!(resp["error"], "not_owned");
        assert!(
            resp["message"].as_str().unwrap().contains("mismatch"),
            "message should mention 'mismatch', got: {}",
            resp["message"]
        );
    }

    #[tokio::test]
    async fn kill_session_removes_session_from_ownership_map() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "rm-uuid",
                "shortname": "s",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;

        // Kill it.
        let kill_resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "KillSession", "uuid": "rm-uuid", "repo_slug": "r"}),
        )
        .await;
        assert!(kill_resp.get("error").is_none(), "kill should succeed: {kill_resp}");

        // GetSession now returns not_found.
        let get_resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "GetSession", "uuid": "rm-uuid"}),
        )
        .await;
        assert_eq!(get_resp["error"], "not_found");

        // ListSessions returns empty.
        let list_resp = send_request(&mut stream, &serde_json::json!({"op": "ListSessions"})).await;
        assert_eq!(list_resp["sessions"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn kill_session_already_dead_in_tmux_counts_as_killed() {
        // kill_session returns "session not found" — already gone. tmux_killed should be true.
        let mock = MockTmuxOps::new()
            .with_kill_session_error("session not found: mux-s");
        let addr = start_mock_server(mock).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "already-dead",
                "shortname": "s",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp",
                "repo_leaf": "r"
            }),
        )
        .await;

        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "KillSession", "uuid": "already-dead", "repo_slug": "r"}),
        )
        .await;
        assert!(resp.get("error").is_none(), "expected success, got: {resp}");
        assert_eq!(resp["tmux_killed"], true, "already-dead session must count as killed");
    }

    #[tokio::test]
    async fn kill_session_mux_created_removes_workdir() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");
        std::fs::create_dir_all(&workdir).unwrap();
        let workdir_parent = tmp.path().to_str().unwrap().to_owned();

        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "wd-uuid",
                "shortname": "s",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": workdir_parent,
                "repo_leaf": "repo"
            }),
        )
        .await;

        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "KillSession", "uuid": "wd-uuid", "repo_slug": "r"}),
        )
        .await;
        assert!(resp.get("error").is_none(), "expected success, got: {resp}");
        assert_eq!(resp["tmux_killed"], true);
        assert_eq!(resp["workdir_removed"], true, "mux_created workdir must be removed");
        assert!(!workdir.exists(), "workdir should have been deleted");
    }

    #[tokio::test]
    async fn kill_session_workdir_already_gone_is_not_an_error() {
        // workdir doesn't exist → remove_dir_all returns NotFound → workdir_removed = false.
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();

        send_request(
            &mut stream,
            &serde_json::json!({
                "op": "CreateSession",
                "uuid": "missing-wd",
                "shortname": "s",
                "repo_slug": "r",
                "branch": "main",
                "workdir_parent": "/tmp/this-parent-does-not-exist",
                "repo_leaf": "repo"
            }),
        )
        .await;

        let resp = send_request(
            &mut stream,
            &serde_json::json!({"op": "KillSession", "uuid": "missing-wd", "repo_slug": "r"}),
        )
        .await;
        assert!(resp.get("error").is_none(), "missing workdir must not cause kill error: {resp}");
        assert_eq!(resp["workdir_removed"], false, "NotFound → workdir_removed is false");
    }

    // ── Shutdown ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn shutdown_returns_ok_response() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(&mut stream, &serde_json::json!({"op": "Shutdown"})).await;
        assert!(resp.get("error").is_none(), "Shutdown must return ok, got: {resp}");
        // ShutdownResponse is an empty object on the wire.
        assert!(resp.is_object());
    }

    #[tokio::test]
    async fn shutdown_stops_accept_loop() {
        // After Shutdown the server breaks its accept loop — new TCP connections are refused.
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(&mut stream, &serde_json::json!({"op": "Shutdown"})).await;
        assert!(resp.get("error").is_none(), "shutdown must return ok, got: {resp}");

        // Give the accept loop time to process the shutdown signal.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // The server should now refuse new connections.
        let result = TcpStream::connect(addr).await;
        assert!(result.is_err(), "server must refuse connections after shutdown");
    }
}
