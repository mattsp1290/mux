use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use mux_core::{error::MuxError, types::TransportMode};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MUX_FORCE_TRANSPORT_ENV: &str = "MUX_FORCE_TRANSPORT";

/// Probe available transports for a host. Tries streamlocal first, falls back to TCP.
///
/// `streamlocal_path`: local end of the forwarded Unix domain socket
/// `loopback_port`: TCP port on 127.0.0.1 (direct-tcpip loopback)
///
/// Returns `TransportMode::Streamlocal` if the socket is connectable,
/// `TransportMode::Tcp` if TCP loopback works, or `ConnectionRefused` if neither works.
pub fn probe_transport(streamlocal_path: &Path, loopback_port: u16) -> Result<TransportMode, MuxError> {
    if probe_streamlocal(streamlocal_path) {
        return Ok(TransportMode::Streamlocal);
    }
    if probe_tcp_loopback(loopback_port) {
        return Ok(TransportMode::Tcp);
    }
    Err(MuxError::ConnectionRefused(format!(
        "streamlocal:{} and 127.0.0.1:{}",
        streamlocal_path.display(),
        loopback_port
    )))
}

/// Check if a Unix domain socket at `path` is connectable.
pub fn probe_streamlocal(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

/// Check if TCP 127.0.0.1:port is connectable within PROBE_TIMEOUT.
pub fn probe_tcp_loopback(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

/// Parse a MUX_FORCE_TRANSPORT value from an optional string.
/// `None` input (unset env var) → `Ok(None)`. Invalid string → `Err`.
pub fn parse_force_transport(val: Option<&str>) -> Result<Option<TransportMode>, MuxError> {
    match val {
        None => Ok(None),
        Some(s) => Ok(Some(s.parse::<TransportMode>()?)),
    }
}

/// Read and parse MUX_FORCE_TRANSPORT environment variable.
/// Returns `None` if the variable is not set.
/// Returns `Err(MuxError::InvalidForceTransport)` if set to an invalid value.
pub fn read_force_transport() -> Result<Option<TransportMode>, MuxError> {
    let raw = std::env::var(MUX_FORCE_TRANSPORT_ENV).ok();
    parse_force_transport(raw.as_deref())
}

/// Apply a MUX_FORCE_TRANSPORT override to the probed transport.
/// Used by `mux create` only.
pub fn select_transport(probed: TransportMode, forced: Option<TransportMode>) -> TransportMode {
    forced.unwrap_or(probed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use tempfile::TempDir;

    // ── probe_streamlocal ─────────────────────────────────────────────────────

    #[test]
    fn probe_streamlocal_missing_path_returns_false() {
        assert!(!probe_streamlocal(Path::new("/nonexistent/path.sock")));
    }

    #[test]
    fn probe_streamlocal_existing_socket_returns_true() {
        use std::os::unix::net::UnixListener;
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");
        let _listener = UnixListener::bind(&sock_path).unwrap();
        assert!(probe_streamlocal(&sock_path));
    }

    // ── probe_tcp_loopback ────────────────────────────────────────────────────

    #[test]
    fn probe_tcp_loopback_closed_port_returns_false() {
        // Port 1 is almost certainly not listening.
        assert!(!probe_tcp_loopback(1));
    }

    #[test]
    fn probe_tcp_loopback_open_port_returns_true() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(probe_tcp_loopback(port));
    }

    // ── probe_transport ───────────────────────────────────────────────────────

    #[test]
    fn probe_transport_prefers_streamlocal() {
        use std::os::unix::net::UnixListener;
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");
        let _listener = UnixListener::bind(&sock_path).unwrap();

        let tcp_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp_listener.local_addr().unwrap().port();

        let result = probe_transport(&sock_path, port).unwrap();
        assert_eq!(result, TransportMode::Streamlocal);
    }

    #[test]
    fn probe_transport_falls_back_to_tcp() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("no.sock"); // does not exist

        let tcp_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp_listener.local_addr().unwrap().port();

        let result = probe_transport(&sock_path, port).unwrap();
        assert_eq!(result, TransportMode::Tcp);
    }

    #[test]
    fn probe_transport_errors_when_both_fail() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("no.sock");
        let result = probe_transport(&sock_path, 1);
        assert!(matches!(result, Err(MuxError::ConnectionRefused(_))));
    }

    // ── parse_force_transport (pure, no env mutation) ─────────────────────────

    #[test]
    fn parse_force_transport_none_returns_none() {
        assert_eq!(parse_force_transport(None).unwrap(), None);
    }

    #[test]
    fn parse_force_transport_streamlocal() {
        assert_eq!(
            parse_force_transport(Some("streamlocal")).unwrap(),
            Some(TransportMode::Streamlocal)
        );
    }

    #[test]
    fn parse_force_transport_tcp() {
        assert_eq!(
            parse_force_transport(Some("tcp")).unwrap(),
            Some(TransportMode::Tcp)
        );
    }

    #[test]
    fn parse_force_transport_invalid_errors() {
        let result = parse_force_transport(Some("quic"));
        assert!(matches!(result, Err(MuxError::InvalidForceTransport(s)) if s == "quic"));
    }

    // ── select_transport ──────────────────────────────────────────────────────

    #[test]
    fn select_transport_forced_overrides_probed() {
        assert_eq!(
            select_transport(TransportMode::Streamlocal, Some(TransportMode::Tcp)),
            TransportMode::Tcp
        );
    }

    #[test]
    fn select_transport_none_uses_probed() {
        assert_eq!(
            select_transport(TransportMode::Tcp, None),
            TransportMode::Tcp
        );
    }
}
