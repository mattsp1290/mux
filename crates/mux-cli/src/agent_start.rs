//! Remote agent start protocol.
//!
//! Spec: docs/05-agent-rpc-and-lifecycle.md §Agent startup

use std::time::Duration;

use mux_core::error::{truncate_stderr, MuxError};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// Single-quote a string for safe use as one shell word.
///
/// Wraps in single quotes and escapes embedded single quotes via the `'\''` idiom,
/// preventing word-splitting and injection when paths are interpolated into shell commands.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// URLs extracted from the agent.lock file.
#[derive(Debug, Clone)]
pub struct AgentUrls {
    tcp_url: String, // "tcp://127.0.0.1:<port>"
    tcp_port: u16,   // non-zero, parsed from tcp_url
}

impl AgentUrls {
    /// Parse from a `tcp://host:port` URL. Rejects missing scheme and port 0.
    pub fn from_tcp_url(tcp_url: impl Into<String>) -> Result<Self, MuxError> {
        let tcp_url = tcp_url.into();
        let rest = tcp_url
            .strip_prefix("tcp://")
            .ok_or_else(|| MuxError::RpcError(format!("agent TCP URL missing tcp:// scheme: {tcp_url}")))?;
        let port_str = rest
            .rsplit(':')
            .next()
            .ok_or_else(|| MuxError::RpcError(format!("invalid agent TCP URL: {tcp_url}")))?;
        let tcp_port = port_str
            .parse::<u16>()
            .ok()
            .filter(|&p| p != 0)
            .ok_or_else(|| MuxError::RpcError(format!("invalid port in agent TCP URL: {tcp_url}")))?;
        Ok(AgentUrls { tcp_url, tcp_port })
    }

    pub fn tcp_url(&self) -> &str {
        &self.tcp_url
    }

    pub fn tcp_port(&self) -> u16 {
        self.tcp_port
    }
}

/// Abstract interface for executing commands on a remote host.
///
/// Implementations: real SSH (future), mock for tests.
pub trait RemoteExec {
    /// Run a command and return (exit_code, stdout, stderr).
    fn run(&self, cmd: &str) -> Result<(i32, String, String), MuxError>;
}

/// The agent start protocol state machine.
pub struct AgentStarter<E: RemoteExec> {
    home: String,
    exec: E,
    startup_timeout: Duration,
    probe_interval: Duration,
}

impl<E: RemoteExec> AgentStarter<E> {
    pub fn new(home: impl Into<String>, exec: E) -> Self {
        Self {
            home: home.into(),
            exec,
            startup_timeout: STARTUP_TIMEOUT,
            probe_interval: PROBE_INTERVAL,
        }
    }

    /// Override timeouts — primarily for testing with short-lived agents.
    pub fn with_timeouts(mut self, startup_timeout: Duration, probe_interval: Duration) -> Self {
        self.startup_timeout = startup_timeout;
        self.probe_interval = probe_interval;
        self
    }

    fn lock_path(&self) -> String {
        format!("{}/.mux/agent.lock", self.home)
    }

    fn sock_path(&self) -> String {
        format!("{}/.mux/agent.sock", self.home)
    }

    fn log_path(&self) -> String {
        format!("{}/.mux/agent.log", self.home)
    }

    fn bin_path(&self) -> String {
        format!("{}/.mux/bin/mux-agent", self.home)
    }

    /// Read and parse agent.lock if it exists.
    ///
    /// Returns `Ok(None)` when the file is absent or empty (agent not yet ready).
    /// Returns `Err` when the file is present but malformed (corrupt JSON, missing fields).
    fn read_lock(&self) -> Result<Option<(u32, String)>, MuxError> {
        let (code, stdout, _stderr) = self
            .exec
            .run(&format!("cat {} 2>/dev/null", sh_quote(&self.lock_path())))?;
        if code != 0 || stdout.trim().is_empty() {
            return Ok(None);
        }
        let json: serde_json::Value = serde_json::from_str(stdout.trim())
            .map_err(|e| MuxError::RpcError(format!("invalid agent.lock JSON: {e}")))?;
        let pid = json["pid"]
            .as_u64()
            .ok_or_else(|| MuxError::RpcError("agent.lock missing pid".into()))?
            as u32;
        let tcp_url = json["tcp_url"]
            .as_str()
            .ok_or_else(|| MuxError::RpcError("agent.lock missing tcp_url".into()))?
            .to_owned();
        Ok(Some((pid, tcp_url)))
    }

    /// Check if a process with the given PID is alive on the remote host.
    fn is_process_alive(&self, pid: u32) -> bool {
        let (code, _, _) = self
            .exec
            .run(&format!("kill -0 {pid} 2>/dev/null"))
            .unwrap_or((1, String::new(), String::new()));
        code == 0
    }

    /// Remove stale lock file and socket.
    fn cleanup_stale(&self) -> Result<(), MuxError> {
        self.exec.run(&format!(
            "rm -f {} {}",
            sh_quote(&self.lock_path()),
            sh_quote(&self.sock_path()),
        ))?;
        Ok(())
    }

    /// Start the agent in the background. Returns the spawned PID if parseable.
    fn start_agent(&self, bind_addr: &str) -> Result<Option<u32>, MuxError> {
        let cmd = format!(
            "nohup {} --bind {} >> {} 2>&1 & echo $!",
            sh_quote(&self.bin_path()),
            sh_quote(bind_addr),
            sh_quote(&self.log_path()),
        );
        let (code, stdout, stderr) = self.exec.run(&cmd)?;
        if code != 0 {
            return Err(MuxError::RpcError(format!(
                "failed to start mux-agent: {stderr}"
            )));
        }
        Ok(stdout.trim().parse::<u32>().ok())
    }

    /// Collect the last N lines from agent.log, byte-capped via `truncate_stderr`.
    fn collect_log_tail(&self, lines: usize) -> String {
        let cmd = format!("tail -n {lines} {} 2>/dev/null", sh_quote(&self.log_path()));
        let (_, stdout, _) = self.exec.run(&cmd).unwrap_or_default();
        truncate_stderr(&stdout)
    }

    /// Connect to an existing agent without starting one.
    ///
    /// Returns `Ok(Some(urls))` if a live agent is found, `Ok(None)` if not running.
    /// Used by operations (e.g. `mux kill`) that must not start a new agent as a side effect.
    pub fn probe_existing(&self) -> Result<Option<AgentUrls>, MuxError> {
        match self.read_lock()? {
            None => Ok(None),
            Some((pid, tcp_url)) => {
                if self.is_process_alive(pid) {
                    Ok(Some(AgentUrls::from_tcp_url(tcp_url)?))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Ensure the agent is running. Returns the agent URLs.
    ///
    /// This is the main entry point for the agent start protocol.
    pub fn ensure_running(&self) -> Result<AgentUrls, MuxError> {
        // Step 1: Check for existing lock.
        if let Some((pid, tcp_url)) = self.read_lock()? {
            if self.is_process_alive(pid) {
                return AgentUrls::from_tcp_url(tcp_url);
            }
            self.cleanup_stale()?;
        }

        // Step 4: Start the agent. Use 0.0.0.0:0 to let the OS pick a port.
        self.start_agent("0.0.0.0:0")?;

        self.poll_until_ready()
    }

    /// Poll agent.lock until the agent writes it (lock-file-based readiness).
    ///
    /// - `Ok(None)` from `read_lock`: lock not yet present — keep polling.
    /// - `Err` from `read_lock`: lock present but corrupt — fail immediately.
    /// - `Ok(Some(...))` with a bad URL: fail immediately (agent wrote an invalid URL).
    fn poll_until_ready(&self) -> Result<AgentUrls, MuxError> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() >= self.startup_timeout {
                let log_tail = self.collect_log_tail(50);
                return Err(MuxError::AgentStartTimeout { log_tail });
            }

            match self.read_lock() {
                Ok(Some((_pid, tcp_url))) => return AgentUrls::from_tcp_url(tcp_url),
                Ok(None) => {}
                Err(e) => return Err(e),
            }

            std::thread::sleep(self.probe_interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct MockExec {
        responses: RefCell<VecDeque<(i32, String, String)>>,
    }

    impl MockExec {
        fn new(responses: Vec<(i32, &str, &str)>) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(code, out, err)| (code, out.to_owned(), err.to_owned()))
                        .collect(),
                ),
            }
        }
    }

    impl RemoteExec for MockExec {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
            let mut responses = self.responses.borrow_mut();
            if let Some((code, out, err)) = responses.pop_front() {
                Ok((code, out, err))
            } else {
                Ok((1, String::new(), "mock: no more responses".to_owned()))
            }
        }
    }

    fn short_timeout_starter(home: &str, exec: MockExec) -> AgentStarter<MockExec> {
        AgentStarter::new(home, exec)
            .with_timeouts(Duration::from_millis(50), Duration::from_millis(5))
    }

    // ── sh_quote ───────────────────────────────────────────────────────────────

    #[test]
    fn sh_quote_wraps_plain_path() {
        assert_eq!(sh_quote("/home/user/.mux"), "'/home/user/.mux'");
    }

    #[test]
    fn sh_quote_escapes_single_quote() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn sh_quote_handles_spaces() {
        assert_eq!(sh_quote("/home/my user/.mux"), "'/home/my user/.mux'");
    }

    // ── AgentUrls parsing ──────────────────────────────────────────────────────

    #[test]
    fn agent_urls_parses_port_from_tcp_url() {
        let urls = AgentUrls::from_tcp_url("tcp://127.0.0.1:9001").unwrap();
        assert_eq!(urls.tcp_port(), 9001);
        assert_eq!(urls.tcp_url(), "tcp://127.0.0.1:9001");
    }

    #[test]
    fn agent_urls_rejects_missing_scheme() {
        assert!(AgentUrls::from_tcp_url("127.0.0.1:9001").is_err());
    }

    #[test]
    fn agent_urls_rejects_malformed_url() {
        assert!(AgentUrls::from_tcp_url("not-a-url").is_err());
    }

    #[test]
    fn agent_urls_rejects_non_numeric_port() {
        assert!(AgentUrls::from_tcp_url("tcp://127.0.0.1:abc").is_err());
    }

    #[test]
    fn agent_urls_rejects_port_zero() {
        assert!(AgentUrls::from_tcp_url("tcp://127.0.0.1:0").is_err());
    }

    // ── read_lock ──────────────────────────────────────────────────────────────

    #[test]
    fn read_lock_returns_none_when_file_missing() {
        let exec = MockExec::new(vec![(1, "", "")]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.read_lock(), Ok(None)));
    }

    #[test]
    fn read_lock_returns_none_when_output_is_whitespace() {
        // Agent wrote lock file but with only whitespace — treated as not-yet-ready.
        let exec = MockExec::new(vec![(0, "   \n\t  ", "")]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.read_lock(), Ok(None)));
    }

    #[test]
    fn read_lock_returns_err_on_invalid_json() {
        let exec = MockExec::new(vec![(0, "not json", "")]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.read_lock(), Err(MuxError::RpcError(_))));
    }

    #[test]
    fn read_lock_returns_err_on_missing_pid() {
        let exec = MockExec::new(vec![(0, r#"{"tcp_url":"tcp://127.0.0.1:5000"}"#, "")]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.read_lock(), Err(MuxError::RpcError(_))));
    }

    #[test]
    fn read_lock_returns_err_on_missing_tcp_url() {
        let exec = MockExec::new(vec![(0, r#"{"pid":1234}"#, "")]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.read_lock(), Err(MuxError::RpcError(_))));
    }

    // ── probe_existing ─────────────────────────────────────────────────────────

    #[test]
    fn probe_existing_returns_none_when_no_lock() {
        let exec = MockExec::new(vec![(1, "", "")]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.probe_existing(), Ok(None)));
    }

    #[test]
    fn probe_existing_returns_none_when_process_dead() {
        let exec = MockExec::new(vec![
            (0, r#"{"pid":9999,"tcp_url":"tcp://127.0.0.1:6000"}"#, ""), // read_lock: stale
            (1, "", "no such process"),                                    // kill -0: dead
        ]);
        let starter = AgentStarter::new("/home/u", exec);
        assert!(matches!(starter.probe_existing(), Ok(None)));
    }

    #[test]
    fn probe_existing_returns_urls_when_alive() {
        let exec = MockExec::new(vec![
            (0, r#"{"pid":5678,"tcp_url":"tcp://127.0.0.1:7777"}"#, ""), // read_lock
            (0, "", ""),                                                   // kill -0: alive
        ]);
        let starter = AgentStarter::new("/home/u", exec);
        let urls = starter.probe_existing().unwrap().expect("expected Some(urls)");
        assert_eq!(urls.tcp_port(), 7777);
    }

    // ── ensure_running happy paths ─────────────────────────────────────────────

    #[test]
    fn ensure_running_starts_agent_when_no_lock() {
        let exec = MockExec::new(vec![
            (1, "", ""),
            (0, "1234", ""),
            (0, r#"{"pid":1234,"tcp_url":"tcp://127.0.0.1:9876"}"#, ""),
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let urls = starter.ensure_running().unwrap();
        assert_eq!(urls.tcp_port(), 9876);
        assert_eq!(urls.tcp_url(), "tcp://127.0.0.1:9876");
    }

    /// Held lock: agent is already running — return existing URLs without restarting.
    ///
    /// Only 2 mock responses are provided (read_lock + kill -0). If ensure_running
    /// incorrectly called start_agent, the mock would return the default error response
    /// (exit 1, "mock: no more responses") and the test would fail.
    #[test]
    fn ensure_running_held_lock_returns_existing_without_restart() {
        let exec = MockExec::new(vec![
            (0, r#"{"pid":5678,"tcp_url":"tcp://127.0.0.1:7777"}"#, ""), // read_lock: lock held
            (0, "", ""),                                                   // kill -0: alive
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let urls = starter.ensure_running().unwrap();
        assert_eq!(urls.tcp_port(), 7777);
    }

    #[test]
    fn ensure_running_cleans_stale_and_restarts() {
        let exec = MockExec::new(vec![
            (0, r#"{"pid":9999,"tcp_url":"tcp://127.0.0.1:8888"}"#, ""), // read_lock: stale
            (1, "", "no such process"),                                    // kill -0: dead
            (0, "", ""),                                                   // cleanup_stale
            (0, "1111", ""),                                               // start_agent
            (0, r#"{"pid":1111,"tcp_url":"tcp://127.0.0.1:4444"}"#, ""), // poll: ready
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let urls = starter.ensure_running().unwrap();
        assert_eq!(urls.tcp_port(), 4444);
    }

    #[test]
    fn ensure_running_start_agent_failure_propagates() {
        let exec = MockExec::new(vec![
            (1, "", ""),                        // read_lock: no lock
            (1, "", "binary not found"),        // start_agent fails
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let err = starter.ensure_running().unwrap_err();
        assert!(
            matches!(err, MuxError::RpcError(_)),
            "expected RpcError on start failure, got: {err:?}"
        );
    }

    // ── timeout and error paths ────────────────────────────────────────────────

    #[test]
    fn ensure_running_times_out() {
        // Queue only has initial read_lock + start_agent; all further calls
        // (poll read_lock iterations + collect_log_tail) get the MockExec default
        // (1, "", "") → Ok(None), so the loop spins until the 50ms timeout fires.
        let exec = MockExec::new(vec![
            (1, "", ""),     // initial read_lock in ensure_running: no lock
            (0, "9999", ""), // start_agent
        ]);
        let starter = short_timeout_starter("/home/user", exec);
        let err = starter.ensure_running().unwrap_err();
        assert!(
            matches!(err, MuxError::AgentStartTimeout { .. }),
            "expected AgentStartTimeout, got: {err:?}"
        );
    }

    #[test]
    fn ensure_running_timeout_includes_log_tail_in_error() {
        // Both poll reads and the collect_log_tail call hit the mock default (1,"","").
        // The variant itself proves the 50-line tail path was reached; content assertions
        // are deferred to integration tests where command dispatch is observable.
        let exec = MockExec::new(vec![
            (1, "", ""),     // read_lock: no lock
            (0, "9999", ""), // start_agent
        ]);
        let starter = short_timeout_starter("/home/user", exec);
        match starter.ensure_running().unwrap_err() {
            MuxError::AgentStartTimeout { log_tail } => {
                // truncate_stderr caps at MAX_STDERR_BYTES (2048); empty is also valid.
                assert!(
                    log_tail.len() <= mux_core::error::MAX_STDERR_BYTES,
                    "log_tail exceeded MAX_STDERR_BYTES: {} bytes",
                    log_tail.len()
                );
            }
            other => panic!("expected AgentStartTimeout, got: {other:?}"),
        }
    }

    #[test]
    fn poll_until_ready_fails_fast_on_corrupt_lock() {
        let exec = MockExec::new(vec![
            (1, "", ""),                       // initial read_lock: absent
            (0, "9999", ""),                   // start_agent
            (0, "not valid json at all", ""),  // poll: lock present but corrupt
        ]);
        let starter = short_timeout_starter("/home/user", exec);
        let err = starter.ensure_running().unwrap_err();
        assert!(
            matches!(err, MuxError::RpcError(_)),
            "expected RpcError for corrupt lock, got: {err:?}"
        );
    }

    // ── unimplemented protocol features (stubs for future iterations) ──────────
    //
    // The following scenarios are defined in docs/05-agent-rpc-and-lifecycle.md
    // but not yet implemented in agent_start.rs. They are marked #[ignore] so
    // they compile and document the acceptance criteria without failing CI.
    // Implement in the iterations that wire up each feature.

    /// Concurrent-start safety: a second client that races to start the agent should
    /// detect the in-progress start via O_CREAT|O_EXCL lock atomicity and wait,
    /// then return the existing agent URLs once the lock appears.
    ///
    /// Spec: docs/05-agent-rpc-and-lifecycle.md §Agent startup step 7.
    #[test]
    #[ignore = "O_CREAT|O_EXCL concurrent-start not yet implemented"]
    fn concurrent_start_second_client_waits_for_first() {
        todo!("implement when atomic lock creation is wired")
    }

    /// Streamlocal start: agent writes both a TCP URL and a Unix-socket URL in
    /// agent.lock; ensure_running returns AgentUrls with a valid sock_url so
    /// the client can connect via the Unix socket (lower latency).
    ///
    /// Spec: docs/05-agent-rpc-and-lifecycle.md §Agent listen URLs.
    #[test]
    #[ignore = "streamlocal transport not yet implemented in agent_start.rs"]
    fn ensure_running_streamlocal_transport_available() {
        todo!("implement when streamlocal URL is wired into AgentUrls and read_lock")
    }

    /// TCP fallback: when MUX_FORCE_TRANSPORT=tcp is set (or when the Unix socket
    /// is unavailable), ensure_running connects via TCP even if a sock_url is present.
    ///
    /// Spec: docs/05-agent-rpc-and-lifecycle.md §Agent listen URLs.
    #[test]
    #[ignore = "MUX_FORCE_TRANSPORT not yet wired into ensure_running"]
    fn ensure_running_tcp_fallback_when_force_transport_is_tcp() {
        todo!("implement when MUX_FORCE_TRANSPORT env var is read and validated in ensure_running")
    }

    /// Invalid MUX_FORCE_TRANSPORT value → InvalidForceTransport error before SSH.
    ///
    /// TransportMode::from_str already validates the value (tested in mux-core);
    /// this stub ensures the agent start flow surfaces the error at the right layer.
    #[test]
    #[ignore = "MUX_FORCE_TRANSPORT not yet wired into ensure_running"]
    fn ensure_running_invalid_force_transport_rejected() {
        todo!("implement when MUX_FORCE_TRANSPORT env var is read in ensure_running")
    }
}
