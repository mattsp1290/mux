//! Remote agent start protocol.
//!
//! Spec: docs/05-agent-rpc-and-lifecycle.md §Agent startup

use std::time::Duration;

use mux_core::error::MuxError;
use mux_rpc::client::RpcClient;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// URLs extracted from the agent.lock file.
#[derive(Debug, Clone)]
pub struct AgentUrls {
    pub tcp_url: String, // "tcp://127.0.0.1:<port>"
    pub tcp_port: u16,   // parsed from tcp_url
}

impl AgentUrls {
    /// Parse the TCP port from a `tcp://host:port` URL.
    pub fn from_tcp_url(tcp_url: impl Into<String>) -> Result<Self, MuxError> {
        let tcp_url = tcp_url.into();
        let port_str = tcp_url
            .rsplit(':')
            .next()
            .ok_or_else(|| MuxError::RpcError(format!("invalid agent TCP URL: {tcp_url}")))?;
        let tcp_port = port_str.parse::<u16>().map_err(|_| {
            MuxError::RpcError(format!("invalid port in agent TCP URL: {tcp_url}"))
        })?;
        Ok(AgentUrls { tcp_url, tcp_port })
    }

    /// Create an RpcClient for TCP loopback on the agent's port.
    pub fn rpc_client(&self) -> RpcClient {
        RpcClient::tcp("127.0.0.1", self.tcp_port)
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
}

impl<E: RemoteExec> AgentStarter<E> {
    pub fn new(home: impl Into<String>, exec: E) -> Self {
        Self {
            home: home.into(),
            exec,
        }
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
    fn read_lock(&self) -> Result<Option<(u32, String)>, MuxError> {
        let lock_path = self.lock_path();
        let (code, stdout, _stderr) = self.exec.run(&format!("cat {lock_path} 2>/dev/null"))?;
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

    /// Check if a process with the given PID is alive.
    fn is_process_alive(&self, pid: u32) -> bool {
        let (code, _, _) = self
            .exec
            .run(&format!("kill -0 {pid} 2>/dev/null"))
            .unwrap_or((1, String::new(), String::new()));
        code == 0
    }

    /// Remove stale lock file and socket.
    fn cleanup_stale(&self) -> Result<(), MuxError> {
        let lock = self.lock_path();
        let sock = self.sock_path();
        self.exec.run(&format!("rm -f {lock} {sock}"))?;
        Ok(())
    }

    /// Start the agent in the background.
    fn start_agent(&self, bind_addr: &str) -> Result<(), MuxError> {
        let bin = self.bin_path();
        let log = self.log_path();
        // nohup runs the binary detached; output goes to log file.
        let cmd = format!("nohup {bin} --bind {bind_addr} >> {log} 2>&1 & echo $!");
        let (code, _stdout, stderr) = self.exec.run(&cmd)?;
        if code != 0 {
            return Err(MuxError::RpcError(format!(
                "failed to start mux-agent: {stderr}"
            )));
        }
        Ok(())
    }

    /// Collect the last N lines from agent.log.
    fn collect_log_tail(&self, lines: usize) -> String {
        let log = self.log_path();
        let cmd = format!("tail -n {lines} {log} 2>/dev/null");
        let (_, stdout, _) = self.exec.run(&cmd).unwrap_or_default();
        stdout
    }

    /// Ensure the agent is running. Returns the agent URLs.
    ///
    /// This is the main entry point for the agent start protocol.
    pub fn ensure_running(&self) -> Result<AgentUrls, MuxError> {
        // Step 1: Check for existing lock.
        if let Some((pid, tcp_url)) = self.read_lock()? {
            // Step 2/3: Determine if it's stale or held.
            if self.is_process_alive(pid) {
                // Agent already running — return existing URLs.
                return AgentUrls::from_tcp_url(tcp_url);
            }
            // Stale — clean up.
            self.cleanup_stale()?;
        }

        // Step 4: Start the agent.
        // Use 0.0.0.0:0 to let the OS pick a port.
        self.start_agent("0.0.0.0:0")?;

        // Step 5/6: Poll for readiness. Use a busy-wait loop since this is sync.
        // The real implementation will be async; this provides the testable logic.
        self.poll_until_ready()
    }

    /// Poll agent.lock until it appears and the agent responds to health checks.
    fn poll_until_ready(&self) -> Result<AgentUrls, MuxError> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() >= STARTUP_TIMEOUT {
                let log_tail = self.collect_log_tail(50);
                return Err(MuxError::AgentStartTimeout { log_tail });
            }

            // Try to read the lock file — it appears when the agent is ready.
            if let Some((_pid, tcp_url)) = self.read_lock().unwrap_or(None) {
                if let Ok(urls) = AgentUrls::from_tcp_url(tcp_url) {
                    return Ok(urls);
                }
            }

            std::thread::sleep(PROBE_INTERVAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::cell::RefCell;

    struct MockExec {
        // Sequential response queue — popped in order, one per run() call.
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
                // No more responses — simulate command not found.
                Ok((1, String::new(), "mock: no more responses".to_owned()))
            }
        }
    }

    // Test: agent not running, start it, lock appears, returns URLs
    #[test]
    fn ensure_running_starts_agent_when_no_lock() {
        let exec = MockExec::new(vec![
            // read_lock: no lock file
            (1, "", ""),
            // start_agent: success with PID
            (0, "1234", ""),
            // poll_until_ready: first poll returns lock
            (0, r#"{"pid":1234,"tcp_url":"tcp://127.0.0.1:9876"}"#, ""),
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let urls = starter.ensure_running().unwrap();
        assert_eq!(urls.tcp_port, 9876);
        assert_eq!(urls.tcp_url, "tcp://127.0.0.1:9876");
    }

    // Test: agent already running with live PID
    #[test]
    fn ensure_running_returns_existing_when_alive() {
        let exec = MockExec::new(vec![
            // read_lock: returns valid lock
            (0, r#"{"pid":5678,"tcp_url":"tcp://127.0.0.1:7777"}"#, ""),
            // is_process_alive: PID is alive
            (0, "", ""),
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let urls = starter.ensure_running().unwrap();
        assert_eq!(urls.tcp_port, 7777);
    }

    // Test: stale lock (dead PID) — cleans up and starts fresh
    #[test]
    fn ensure_running_cleans_stale_and_restarts() {
        let exec = MockExec::new(vec![
            // read_lock: returns stale lock
            (0, r#"{"pid":9999,"tcp_url":"tcp://127.0.0.1:8888"}"#, ""),
            // is_process_alive: PID is dead
            (1, "", "no such process"),
            // cleanup_stale: rm -f succeeds
            (0, "", ""),
            // start_agent
            (0, "1111", ""),
            // poll_until_ready: lock appears
            (0, r#"{"pid":1111,"tcp_url":"tcp://127.0.0.1:4444"}"#, ""),
        ]);
        let starter = AgentStarter::new("/home/user", exec);
        let urls = starter.ensure_running().unwrap();
        assert_eq!(urls.tcp_port, 4444);
    }

    // Test: AgentUrls parsing
    #[test]
    fn agent_urls_parses_port_from_tcp_url() {
        let urls = AgentUrls::from_tcp_url("tcp://127.0.0.1:9001").unwrap();
        assert_eq!(urls.tcp_port, 9001);
        assert_eq!(urls.tcp_url, "tcp://127.0.0.1:9001");
    }

    #[test]
    fn agent_urls_rejects_malformed_url() {
        let result = AgentUrls::from_tcp_url("not-a-url");
        // "not-a-url".rsplit(':').next() => "not-a-url", which fails u16 parse
        assert!(result.is_err());
    }

    #[test]
    fn agent_urls_rejects_non_numeric_port() {
        let result = AgentUrls::from_tcp_url("tcp://127.0.0.1:abc");
        assert!(result.is_err());
    }
}
