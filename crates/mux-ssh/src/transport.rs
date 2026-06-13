use std::net::{TcpStream, ToSocketAddrs};
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
    Err(MuxError::ConnectionRefused(format!("127.0.0.1:{loopback_port}")))
}

/// Check if a Unix domain socket at `path` is connectable.
pub fn probe_streamlocal(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

/// Check if TCP 127.0.0.1:port is connectable within PROBE_TIMEOUT.
pub fn probe_tcp_loopback(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    match addr.to_socket_addrs() {
        Ok(mut addrs) => addrs
            .next()
            .map(|a| TcpStream::connect_timeout(&a, PROBE_TIMEOUT).is_ok())
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Read and parse MUX_FORCE_TRANSPORT environment variable.
/// Returns `None` if the variable is not set.
/// Returns `Err(MuxError::InvalidForceTransport)` if set to an invalid value.
pub fn read_force_transport() -> Result<Option<TransportMode>, MuxError> {
    match std::env::var(MUX_FORCE_TRANSPORT_ENV) {
        Err(_) => Ok(None),
        Ok(val) => {
            let mode = val.parse::<TransportMode>()?;
            Ok(Some(mode))
        }
    }
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

    // ── read_force_transport ──────────────────────────────────────────────────

    #[test]
    fn read_force_transport_unset_returns_none() {
        std::env::remove_var(MUX_FORCE_TRANSPORT_ENV);
        assert_eq!(read_force_transport().unwrap(), None);
    }

    #[test]
    fn read_force_transport_streamlocal() {
        std::env::set_var(MUX_FORCE_TRANSPORT_ENV, "streamlocal");
        assert_eq!(read_force_transport().unwrap(), Some(TransportMode::Streamlocal));
        std::env::remove_var(MUX_FORCE_TRANSPORT_ENV);
    }

    #[test]
    fn read_force_transport_tcp() {
        std::env::set_var(MUX_FORCE_TRANSPORT_ENV, "tcp");
        assert_eq!(read_force_transport().unwrap(), Some(TransportMode::Tcp));
        std::env::remove_var(MUX_FORCE_TRANSPORT_ENV);
    }

    #[test]
    fn read_force_transport_invalid_errors() {
        std::env::set_var(MUX_FORCE_TRANSPORT_ENV, "quic");
        let result = read_force_transport();
        assert!(matches!(result, Err(MuxError::InvalidForceTransport(s)) if s == "quic"));
        std::env::remove_var(MUX_FORCE_TRANSPORT_ENV);
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
