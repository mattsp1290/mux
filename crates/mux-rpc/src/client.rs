//! RPC client over SSH-forwarded streamlocal and TCP fallback channels.
//!
//! Wire format: `[u32 LE length][UTF-8 JSON body]` — delegated to `codec`.

use std::path::PathBuf;
use std::time::Duration;

use mux_core::{error::MuxError, types::TransportMode};
use serde::de::DeserializeOwned;
use tokio::net::{TcpStream, UnixStream};
use tokio::time::timeout;

use crate::codec;
use crate::schema::{
    CreateSessionRequest, CreateSessionResponse, GetSessionRequest, GetSessionResponse,
    HealthRequest, HealthResponse, KillSessionRequest, KillSessionResponse, ListSessionsRequest,
    ListSessionsResponse, Request, RpcResult, ShutdownRequest, ShutdownResponse,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ── Endpoint ──────────────────────────────────────────────────────────────────

enum RpcEndpoint {
    Streamlocal(PathBuf),
    Tcp { host: String, port: u16 },
}

// ── Client ────────────────────────────────────────────────────────────────────

pub struct RpcClient {
    endpoint: RpcEndpoint,
}

impl RpcClient {
    /// Connect over a Unix domain socket (streamlocal transport).
    pub fn streamlocal(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            endpoint: RpcEndpoint::Streamlocal(socket_path.into()),
        }
    }

    /// Connect over TCP loopback (tcp transport).
    pub fn tcp(host: impl Into<String>, port: u16) -> Self {
        Self {
            endpoint: RpcEndpoint::Tcp {
                host: host.into(),
                port,
            },
        }
    }

    /// Construct from a `TransportMode` plus both endpoint options.
    pub fn from_transport(
        mode: TransportMode,
        streamlocal_path: impl Into<PathBuf>,
        tcp_host: impl Into<String>,
        tcp_port: u16,
    ) -> Self {
        match mode {
            TransportMode::Streamlocal => Self::streamlocal(streamlocal_path),
            TransportMode::Tcp => Self::tcp(tcp_host, tcp_port),
            // #[non_exhaustive]: fall back to streamlocal for any future variant.
            _ => {
                tracing::warn!("unknown TransportMode variant; defaulting to streamlocal");
                Self::streamlocal(streamlocal_path)
            }
        }
    }

    fn endpoint_display(&self) -> String {
        match &self.endpoint {
            RpcEndpoint::Streamlocal(p) => format!("unix:{}", p.display()),
            RpcEndpoint::Tcp { host, port } => format!("{host}:{port}"),
        }
    }

    // ── Internal transport ────────────────────────────────────────────────────

    async fn send_request(&self, req: Request) -> Result<Vec<u8>, MuxError> {
        let json = codec::encode(&req)
            .map_err(|e| MuxError::RpcError(e.to_string()))?;

        let display = self.endpoint_display();
        timeout(REQUEST_TIMEOUT, async {
            match &self.endpoint {
                RpcEndpoint::Streamlocal(path) => {
                    let mut stream = UnixStream::connect(path).await.map_err(|_| {
                        MuxError::ConnectionRefused(path.display().to_string())
                    })?;
                    codec::write_message(&mut stream, &json).await
                        .map_err(|e| MuxError::RpcError(e.to_string()))?;
                    codec::read_message(&mut stream).await
                        .map_err(|e| MuxError::RpcError(e.to_string()))?
                        .ok_or_else(|| MuxError::RpcError("server closed connection".into()))
                }
                RpcEndpoint::Tcp { host, port } => {
                    let addr = format!("{host}:{port}");
                    let mut stream = TcpStream::connect(&addr).await
                        .map_err(|_| MuxError::ConnectionRefused(addr.clone()))?;
                    codec::write_message(&mut stream, &json).await
                        .map_err(|e| MuxError::RpcError(e.to_string()))?;
                    codec::read_message(&mut stream).await
                        .map_err(|e| MuxError::RpcError(e.to_string()))?
                        .ok_or_else(|| MuxError::RpcError("server closed connection".into()))
                }
            }
        })
        .await
        .map_err(|_| MuxError::ConnectionTimeout(display))?
    }

    // ── Public operations ─────────────────────────────────────────────────────

    pub async fn health(&self) -> Result<HealthResponse, MuxError> {
        let bytes = self.send_request(Request::Health(HealthRequest {})).await?;
        decode_response(&bytes)
    }

    pub async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<CreateSessionResponse, MuxError> {
        let bytes = self
            .send_request(Request::CreateSession(req))
            .await?;
        decode_response(&bytes)
    }

    pub async fn list_sessions(&self) -> Result<ListSessionsResponse, MuxError> {
        let bytes = self
            .send_request(Request::ListSessions(ListSessionsRequest {}))
            .await?;
        decode_response(&bytes)
    }

    pub async fn get_session(
        &self,
        req: GetSessionRequest,
    ) -> Result<GetSessionResponse, MuxError> {
        let bytes = self.send_request(Request::GetSession(req)).await?;
        decode_response(&bytes)
    }

    pub async fn kill_session(
        &self,
        req: KillSessionRequest,
    ) -> Result<KillSessionResponse, MuxError> {
        let bytes = self.send_request(Request::KillSession(req)).await?;
        decode_response(&bytes)
    }

    pub async fn shutdown(&self) -> Result<ShutdownResponse, MuxError> {
        let bytes = self
            .send_request(Request::Shutdown(ShutdownRequest {}))
            .await?;
        decode_response(&bytes)
    }

    pub async fn stream_session_events(&self) -> Result<(), MuxError> {
        // v0.1: streaming unimplemented; skip the round-trip entirely.
        Err(MuxError::RpcError("streaming not implemented".into()))
    }

    // ── Health probe ──────────────────────────────────────────────────────────

    /// Poll health until the agent responds or `max_duration` expires.
    ///
    /// Returns `true` if the agent became healthy within the allotted time,
    /// `false` if the deadline was reached without a successful response.
    ///
    /// Each probe is bounded by the remaining budget so the total wall-clock
    /// time is at most `max_duration + 1 sleep interval` regardless of the
    /// per-request timeout.
    pub async fn poll_until_healthy(&self, max_duration: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + max_duration;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let probe = timeout(remaining, self.health());
            if matches!(probe.await, Ok(Ok(_))) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

// ── Response decode ───────────────────────────────────────────────────────────

fn decode_response<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MuxError> {
    let result: RpcResult<T> = serde_json::from_slice(bytes)
        .map_err(|e| MuxError::RpcError(format!("response parse error: {e}")))?;
    result
        .into_result()
        .map_err(|e| MuxError::AgentError(format!("{}: {}", e.error, e.message)))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::HealthResponse;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, UnixListener};

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn health_ok_frame() -> Vec<u8> {
        let body = codec::encode(&HealthResponse { ok: true }).unwrap();
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    /// Spawn a Unix socket echo server: reads one request, responds with health ok.
    async fn spawn_unix_echo_server(path: &std::path::Path) {
        let listener = UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = codec::read_message(&mut stream).await;
                let frame = health_ok_frame().await;
                let _ = stream.write_all(&frame).await;
            }
        });
    }

    /// Spawn a TCP echo server: reads one request, responds, closes. Returns port.
    async fn spawn_tcp_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = codec::read_message(&mut stream).await;
                let frame = health_ok_frame().await;
                let _ = stream.write_all(&frame).await;
            }
        });
        port
    }

    // ── 1. codec roundtrip via duplex ─────────────────────────────────────────

    #[tokio::test]
    async fn codec_roundtrip_via_duplex() {
        let (mut client_half, mut server_half) = tokio::io::duplex(4096);
        let body = b"hello, frame!";
        codec::write_message(&mut client_half, body).await.unwrap();
        drop(client_half);
        let received = codec::read_message(&mut server_half).await.unwrap().unwrap();
        assert_eq!(received, body);
    }

    // ── 2. health over streamlocal ────────────────────────────────────────────

    #[tokio::test]
    async fn send_request_streamlocal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("test.sock");
        spawn_unix_echo_server(&socket_path).await;
        let client = RpcClient::streamlocal(&socket_path);
        let resp = client.health().await.unwrap();
        assert!(resp.ok);
    }

    // ── 3. health over TCP ────────────────────────────────────────────────────

    #[tokio::test]
    async fn send_request_tcp_roundtrip() {
        let port = spawn_tcp_echo_server().await;
        let client = RpcClient::tcp("127.0.0.1", port);
        let resp = client.health().await.unwrap();
        assert!(resp.ok);
    }

    // ── 4. connection refused ─────────────────────────────────────────────────

    #[tokio::test]
    async fn connection_refused_unix() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("nonexistent.sock");
        let client = RpcClient::streamlocal(&socket_path);
        let err = client.health().await.unwrap_err();
        assert!(
            matches!(err, MuxError::ConnectionRefused(_)),
            "expected ConnectionRefused, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn connection_refused_tcp() {
        let client = RpcClient::tcp("127.0.0.1", 1);
        let err = client.health().await.unwrap_err();
        assert!(
            matches!(err, MuxError::ConnectionRefused(_) | MuxError::ConnectionTimeout(_)),
            "expected ConnectionRefused or ConnectionTimeout, got: {err:?}"
        );
    }

    // ── 5. poll_until_healthy returns true when up ────────────────────────────

    #[tokio::test]
    async fn poll_until_healthy_returns_true_when_up() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let listener = Arc::new(listener);
        let listener_clone = Arc::clone(&listener);
        tokio::spawn(async move {
            for _ in 0..5 {
                if let Ok((mut stream, _)) = listener_clone.accept().await {
                    let _ = codec::read_message(&mut stream).await;
                    let frame = health_ok_frame().await;
                    let _ = stream.write_all(&frame).await;
                }
            }
        });
        let client = RpcClient::tcp("127.0.0.1", port);
        let healthy = client.poll_until_healthy(Duration::from_secs(5)).await;
        assert!(healthy);
    }

    // ── 6. poll_until_healthy returns false when never up ────────────────────

    #[tokio::test]
    async fn poll_until_healthy_returns_false_when_down() {
        let client = RpcClient::tcp("127.0.0.1", 1);
        let healthy = client.poll_until_healthy(Duration::from_secs(2)).await;
        assert!(!healthy);
    }

    // ── 7. oversized response rejected by codec ───────────────────────────────

    #[tokio::test]
    async fn oversized_frame_rejected() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        let huge = (codec::MAX_MESSAGE_BYTES + 1).to_le_bytes();
        writer.write_all(&huge).await.unwrap();
        drop(writer);
        // codec::read_message returns Err on oversized length
        let result = codec::read_message(&mut reader).await;
        assert!(result.is_err(), "expected error for oversized frame");
    }

    // ── 8. server closes connection mid-request ───────────────────────────────

    #[tokio::test]
    async fn server_closed_connection_returns_rpc_error() {
        // Server accepts, reads request, then closes without responding.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = codec::read_message(&mut stream).await;
                drop(stream); // close without responding
            }
        });
        let client = RpcClient::tcp("127.0.0.1", port);
        let err = client.health().await.unwrap_err();
        assert!(
            matches!(err, MuxError::RpcError(_)),
            "expected RpcError for server-closed connection, got: {err:?}"
        );
    }

    // ── 9. error response decoded as AgentError ───────────────────────────────

    #[tokio::test]
    async fn error_response_decoded_as_agent_error() {
        use crate::schema::RpcError;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = codec::read_message(&mut stream).await;
                let err_body = codec::encode(&RpcError::not_found("session missing")).unwrap();
                let mut frame = (err_body.len() as u32).to_le_bytes().to_vec();
                frame.extend_from_slice(&err_body);
                let _ = stream.write_all(&frame).await;
            }
        });
        let client = RpcClient::tcp("127.0.0.1", port);
        let err = client.health().await.unwrap_err();
        assert!(
            matches!(err, MuxError::AgentError(_)),
            "expected AgentError, got: {err:?}"
        );
    }

    // ── 10. from_transport dispatches correctly ───────────────────────────────

    #[test]
    fn from_transport_streamlocal_sets_unix_endpoint() {
        let client = RpcClient::from_transport(
            TransportMode::Streamlocal, "/tmp/mux.sock", "127.0.0.1", 9000,
        );
        let display = client.endpoint_display();
        assert!(display.starts_with("unix:") && display.contains("mux.sock"));
    }

    #[test]
    fn from_transport_tcp_sets_tcp_endpoint() {
        let client = RpcClient::from_transport(
            TransportMode::Tcp, "/tmp/mux.sock", "127.0.0.1", 9001,
        );
        assert_eq!(client.endpoint_display(), "127.0.0.1:9001");
    }

    // ── 11. stream_session_events short-circuits ──────────────────────────────

    #[tokio::test]
    async fn stream_session_events_never_connects() {
        // Point to a nonexistent server — should fail immediately without connecting.
        let client = RpcClient::tcp("127.0.0.1", 1);
        let err = client.stream_session_events().await.unwrap_err();
        // Returns RpcError("streaming not implemented") without attempting connection.
        assert!(
            matches!(&err, MuxError::RpcError(msg) if msg.contains("not implemented")),
            "expected 'not implemented' RpcError, got: {err:?}"
        );
    }
}
