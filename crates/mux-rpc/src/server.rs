//! RPC server — TCP listener with length-prefix framing.
//!
//! Wire format: [u32 LE length][UTF-8 JSON body] in each direction.
//! All requests carry an `"op"` tag for dispatch (see `schema::Request`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use mux_tmux::adapter::TmuxAdapter;

use crate::schema::{
    CreateSessionResponse, GetSessionResponse, HealthResponse, KillSessionResponse,
    ListSessionsResponse, RpcError, RpcResult, Request, SessionInfo, SessionStatusValue,
    ShutdownResponse,
};

// ── Codec constants ───────────────────────────────────────────────────────────

/// Maximum frame body size (4 MiB). Rejects attacker-controlled length prefixes
/// before allocating, preventing OOM from a forged large-frame attack.
const MAX_FRAME_LEN: usize = 4 * 1024 * 1024;

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

pub struct RpcServer {
    tmux: TmuxAdapter,
    bind_addr: String,
}

pub struct BoundRpcServer {
    listener: TcpListener,
    tmux: Arc<TmuxAdapter>,
    ownership: OwnershipMap,
    shutdown_flag: Arc<AtomicBool>,
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

    pub async fn bind(self) -> anyhow::Result<BoundRpcServer> {
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

impl BoundRpcServer {
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

async fn handle_connection(
    mut stream: TcpStream,
    tmux: Arc<TmuxAdapter>,
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

                    match tmux.new_session(&tmux_name, &workdir, None).await {
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
    // Helper: create a connected loopback TCP pair for testing codec.
    async fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client_res, server_res) =
            tokio::join!(TcpStream::connect(addr), listener.accept());
        (client_res.unwrap(), server_res.unwrap().0)
    }

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

    // Helper: start a server bound to a random port, return its addr.
    async fn start_test_server() -> std::net::SocketAddr {
        let server = RpcServer::new("127.0.0.1:0");
        let bound = server.bind().await.unwrap();
        let addr = bound.local_addr();
        tokio::spawn(async move {
            let _ = bound.serve().await;
        });
        addr
    }

    async fn send_request(stream: &mut TcpStream, req: &serde_json::Value) -> serde_json::Value {
        let bytes = serde_json::to_vec(req).unwrap();
        write_message(stream, &bytes).await.unwrap();
        let resp_bytes = read_message(stream).await.unwrap();
        serde_json::from_slice(&resp_bytes).unwrap()
    }

    #[tokio::test]
    async fn health_request_returns_ok_true() {
        let addr = start_test_server().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let resp = send_request(&mut stream, &serde_json::json!({"op": "Health"})).await;
        assert_eq!(resp["ok"], true);
        assert!(resp.get("error").is_none());
    }

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
    }

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
    async fn create_session_after_shutdown_returns_internal_error() {
        let server = RpcServer::new("127.0.0.1:0");
        let bound = server.bind().await.unwrap();
        let addr = bound.local_addr();
        // Set the shutdown flag before spawning
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
}
