//! RPC client over SSH-forwarded streamlocal and TCP fallback channels.
//!
//! Wire format: `[u32 LE length][UTF-8 JSON body]`

use std::path::PathBuf;
use std::time::Duration;

use mux_core::{error::MuxError, types::TransportMode};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use tokio::time::timeout;

use crate::schema::{
    CreateSessionRequest, CreateSessionResponse, GetSessionRequest, GetSessionResponse,
    HealthRequest, HealthResponse, KillSessionRequest, KillSessionResponse, ListSessionsRequest,
    ListSessionsResponse, Request, RpcResult, ShutdownRequest, ShutdownResponse,
    StreamSessionEventsRequest,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FRAME_LEN: usize = 4 * 1024 * 1024;

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
        let json = serde_json::to_vec(&req)
            .map_err(|e| MuxError::RpcError(e.to_string()))?;
        let frame_len = u32::try_from(json.len())
            .map_err(|_| MuxError::RpcError("request too large to frame".into()))?;

        timeout(REQUEST_TIMEOUT, async {
            match &self.endpoint {
                RpcEndpoint::Streamlocal(path) => {
                    let mut stream = UnixStream::connect(path).await.map_err(|_| {
                        MuxError::ConnectionRefused(path.display().to_string())
                    })?;
                    write_frame(&mut stream, frame_len, &json).await?;
                    read_frame(&mut stream).await
                }
                RpcEndpoint::Tcp { host, port } => {
                    let addr = format!("{host}:{port}");
                    let mut stream = TcpStream::connect(&addr)
                        .await
                        .map_err(|_| MuxError::ConnectionRefused(addr.clone()))?;
                    write_frame(&mut stream, frame_len, &json).await?;
                    read_frame(&mut stream).await
                }
            }
        })
        .await
        .map_err(|_| MuxError::ConnectionTimeout(self.endpoint_display()))?
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
        // Streaming is not implemented in v0.1.
        let _bytes = self
            .send_request(Request::StreamSessionEvents(StreamSessionEventsRequest {}))
            .await?;
        Err(MuxError::RpcError("streaming not implemented".into()))
    }

    // ── Health probe ──────────────────────────────────────────────────────────

    /// Poll health until the agent responds or `max_duration` expires.
    ///
    /// Returns `true` if the agent became healthy within the allotted time,
    /// `false` if the deadline was reached without a successful response.
    pub async fn poll_until_healthy(&self, max_duration: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + max_duration;
        loop {
            if self.health().await.is_ok() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

// ── Frame helpers ─────────────────────────────────────────────────────────────

async fn write_frame<W: AsyncWriteExt + Unpin>(
    stream: &mut W,
    len: u32,
    body: &[u8],
) -> Result<(), MuxError> {
    stream
        .write_all(&len.to_le_bytes())
        .await
        .map_err(|e| MuxError::RpcError(format!("write length prefix: {e}")))?;
    stream
        .write_all(body)
        .await
        .map_err(|e| MuxError::RpcError(format!("write body: {e}")))?;
    Ok(())
}

async fn read_frame<R: AsyncReadExt + Unpin>(stream: &mut R) -> Result<Vec<u8>, MuxError> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| MuxError::RpcError(format!("read length prefix: {e}")))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(MuxError::RpcError(format!(
            "invalid frame length: {len} (max {MAX_FRAME_LEN})"
        )));
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| MuxError::RpcError(format!("read body: {e}")))?;
    Ok(body)
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
    // tokio AsyncWriteExt and AsyncReadExt are used via the write_all/read_exact
    // methods on streams returned by the listener helpers — no explicit import needed.
    use tokio::net::{TcpListener, UnixListener};

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a framed `HealthResponse { ok: true }` payload.
    fn health_ok_frame() -> Vec<u8> {
        let body = serde_json::to_vec(&HealthResponse { ok: true }).unwrap();
        let len = (body.len() as u32).to_le_bytes();
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&len);
        frame.extend_from_slice(&body);
        frame
    }

    /// Spawn a Unix socket echo server that reads one request frame, then
    /// responds with a `HealthResponse { ok: true }` frame, then closes.
    async fn spawn_unix_echo_server(path: &std::path::Path) {
        let listener = UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                // Consume the request frame (length + body).
                let mut len_buf = [0u8; 4];
                let _ = stream.read_exact(&mut len_buf).await;
                let body_len = u32::from_le_bytes(len_buf) as usize;
                let mut _body = vec![0u8; body_len];
                let _ = stream.read_exact(&mut _body).await;

                // Reply with health ok.
                let _ = stream.write_all(&health_ok_frame()).await;
            }
        });
    }

    /// Spawn a TCP echo server that reads one request frame, responds, then closes.
    /// Returns the actual bound port.
    async fn spawn_tcp_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut len_buf = [0u8; 4];
                let _ = stream.read_exact(&mut len_buf).await;
                let body_len = u32::from_le_bytes(len_buf) as usize;
                let mut _body = vec![0u8; body_len];
                let _ = stream.read_exact(&mut _body).await;

                let _ = stream.write_all(&health_ok_frame()).await;
            }
        });
        port
    }

    // ── 1. write_frame / read_frame roundtrip ─────────────────────────────────

    #[tokio::test]
    async fn frame_roundtrip_via_duplex() {
        let (mut client_half, mut server_half) = tokio::io::duplex(4096);

        let body = b"hello, frame!";
        let len = body.len() as u32;
        write_frame(&mut client_half, len, body).await.unwrap();

        // Drop client half so EOF signals are sent after writing.
        drop(client_half);

        let received = read_frame(&mut server_half).await.unwrap();
        assert_eq!(received, body);
    }

    // ── 2. send_request via streamlocal echo server ───────────────────────────

    #[tokio::test]
    async fn send_request_streamlocal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("test.sock");

        spawn_unix_echo_server(&socket_path).await;
        // Give the server a tick to bind.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let client = RpcClient::streamlocal(&socket_path);
        let resp = client.health().await.unwrap();
        assert!(resp.ok);
    }

    // ── 3. send_request via TCP echo server ───────────────────────────────────

    #[tokio::test]
    async fn send_request_tcp_roundtrip() {
        let port = spawn_tcp_echo_server().await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        let client = RpcClient::tcp("127.0.0.1", port);
        let resp = client.health().await.unwrap();
        assert!(resp.ok);
    }

    // ── 4. Connection refused → MuxError::ConnectionRefused ──────────────────

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
        // Port 1 is privileged and always refused on Linux.
        let client = RpcClient::tcp("127.0.0.1", 1);
        let err = client.health().await.unwrap_err();
        // The timeout may fire before refused depending on the OS, accept either.
        assert!(
            matches!(
                err,
                MuxError::ConnectionRefused(_) | MuxError::ConnectionTimeout(_)
            ),
            "expected ConnectionRefused or ConnectionTimeout, got: {err:?}"
        );
    }

    // ── 5. poll_until_healthy returns true when server is up ──────────────────

    #[tokio::test]
    async fn poll_until_healthy_returns_true_when_up() {
        // The TCP echo server only handles one connection. For polling we need a
        // server that stays up for multiple probes; spawn one that loops.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let listener = Arc::new(listener);
        let listener_clone = Arc::clone(&listener);
        tokio::spawn(async move {
            // Accept a fixed number of connections so the test can't stall.
            for _ in 0..5 {
                if let Ok((mut stream, _)) = listener_clone.accept().await {
                    let mut len_buf = [0u8; 4];
                    let _ = stream.read_exact(&mut len_buf).await;
                    let body_len = u32::from_le_bytes(len_buf) as usize;
                    let mut _body = vec![0u8; body_len];
                    let _ = stream.read_exact(&mut _body).await;
                    let _ = stream.write_all(&health_ok_frame()).await;
                }
            }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        let client = RpcClient::tcp("127.0.0.1", port);
        let healthy = client.poll_until_healthy(Duration::from_secs(5)).await;
        assert!(healthy);
    }

    // ── 6. poll_until_healthy returns false when server never responds ─────────

    #[tokio::test]
    async fn poll_until_healthy_returns_false_when_down() {
        // Use a port that will always refuse connections.
        let client = RpcClient::tcp("127.0.0.1", 1);
        // Use a short max_duration so the test finishes quickly.
        let healthy = client.poll_until_healthy(Duration::from_secs(2)).await;
        assert!(!healthy);
    }

    // ── 7. Frame too large rejected ───────────────────────────────────────────

    #[tokio::test]
    async fn read_frame_rejects_too_large() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        // Write a length > MAX_FRAME_LEN.
        let huge = (MAX_FRAME_LEN + 1) as u32;
        writer.write_all(&huge.to_le_bytes()).await.unwrap();
        drop(writer);

        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(
            matches!(err, MuxError::RpcError(_)),
            "expected RpcError for oversized frame, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn read_frame_rejects_zero_length() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_all(&0u32.to_le_bytes()).await.unwrap();
        drop(writer);

        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(
            matches!(err, MuxError::RpcError(_)),
            "expected RpcError for zero-length frame, got: {err:?}"
        );
    }

    // ── 8. from_transport dispatches correctly ────────────────────────────────

    #[test]
    fn from_transport_streamlocal_sets_unix_endpoint() {
        let client = RpcClient::from_transport(
            TransportMode::Streamlocal,
            "/tmp/mux.sock",
            "127.0.0.1",
            9000,
        );
        let display = client.endpoint_display();
        assert!(
            display.starts_with("unix:"),
            "expected unix: prefix, got: {display}"
        );
        assert!(display.contains("mux.sock"));
    }

    #[test]
    fn from_transport_tcp_sets_tcp_endpoint() {
        let client = RpcClient::from_transport(
            TransportMode::Tcp,
            "/tmp/mux.sock",
            "127.0.0.1",
            9001,
        );
        let display = client.endpoint_display();
        assert_eq!(display, "127.0.0.1:9001");
    }
}
