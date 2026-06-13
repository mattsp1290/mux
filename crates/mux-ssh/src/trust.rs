//! SSH host-key TOFU trust logic and SSH agent key enumeration.
//!
//! Spec: docs/04-ssh-trust-and-transport.md

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use anyhow::Result;
use mux_state::fingerprint_repo;

// ── Wire protocol constants ───────────────────────────────────────────────────

const SSH2_AGENTC_REQUEST_IDENTITIES: u8 = 0x0b;
const SSH2_AGENT_IDENTITIES_ANSWER: u8 = 0x0c;
const SSH_AGENT_FAILURE: u8 = 0x05;
const MAX_AGENT_REPLY: usize = 256 * 1024;

// ── Algorithm preference order for attach pinning ────────────────────────────

const ALG_PREFERENCE: &[&str] = &[
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "rsa-sha2-512",
    "rsa-sha2-256",
];

// ── Public types ─────────────────────────────────────────────────────────────

/// Result of checking a host key against stored records.
#[derive(Debug, Clone, PartialEq)]
pub enum TrustCheckResult {
    /// Stored fingerprint matches — proceed silently.
    Trusted,
    /// No stored record for this algorithm — first contact.
    FirstContact { algorithm: String, fingerprint: String },
    /// Stored record differs — abort; never connect.
    Mismatch { algorithm: String, stored: String, received: String },
}

/// A public key offered by the SSH agent.
#[derive(Debug, Clone)]
pub struct AgentKey {
    pub algorithm: String,
    pub key_blob: Vec<u8>,
    pub comment: String,
}

// ── Public functions ──────────────────────────────────────────────────────────

/// Check a received server host-key fingerprint against stored records.
pub fn check_host_key(
    conn: &rusqlite::Connection,
    host_id: i64,
    algorithm: &str,
    received_fingerprint: &str,
) -> Result<TrustCheckResult> {
    match fingerprint_repo::get(conn, host_id, algorithm)? {
        None => Ok(TrustCheckResult::FirstContact {
            algorithm: algorithm.to_owned(),
            fingerprint: received_fingerprint.to_owned(),
        }),
        Some(stored) if stored.fingerprint == received_fingerprint => Ok(TrustCheckResult::Trusted),
        Some(stored) => Ok(TrustCheckResult::Mismatch {
            algorithm: algorithm.to_owned(),
            stored: stored.fingerprint,
            received: received_fingerprint.to_owned(),
        }),
    }
}

/// Persist a trust decision after user confirms first-contact.
pub fn trust_fingerprint(
    conn: &rusqlite::Connection,
    host_id: i64,
    algorithm: &str,
    fingerprint: &str,
) -> Result<()> {
    let trusted_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    fingerprint_repo::upsert(conn, host_id, algorithm, fingerprint, trusted_at)
}

/// Select the best stored fingerprint for `mux attach` (algorithm preference order).
///
/// Returns `None` if the host has no stored fingerprints.
pub fn preferred_fingerprint_for_attach(
    fingerprints: &[mux_state::model::KnownHostFingerprint],
) -> Option<&mux_state::model::KnownHostFingerprint> {
    if fingerprints.is_empty() {
        return None;
    }
    for preferred_alg in ALG_PREFERENCE {
        if let Some(fp) = fingerprints.iter().find(|fp| fp.algorithm == *preferred_alg) {
            return Some(fp);
        }
    }
    // No known algorithm matched — return the first entry as-is.
    fingerprints.first()
}

/// Enumerate public keys from the SSH agent at `SSH_AUTH_SOCK`.
///
/// Returns `Err(MuxError::SshAgentNotForwarded)` if `SSH_AUTH_SOCK` is not set
/// or the socket cannot be connected.
pub fn list_agent_keys() -> Result<Vec<AgentKey>, mux_core::error::MuxError> {
    let sock_path = std::env::var("SSH_AUTH_SOCK").unwrap_or_default();
    if sock_path.is_empty() {
        return Err(mux_core::error::MuxError::SshAgentNotForwarded);
    }

    let mut stream =
        UnixStream::connect(&sock_path).map_err(|_| mux_core::error::MuxError::SshAgentNotForwarded)?;

    // 5-second deadline prevents an unresponsive agent from hanging the CLI.
    let timeout = std::time::Duration::from_secs(5);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    // Send SSH2_AGENTC_REQUEST_IDENTITIES, length-prefixed.
    let mut req = Vec::with_capacity(5);
    req.extend_from_slice(&1u32.to_be_bytes()); // message length = 1
    req.push(SSH2_AGENTC_REQUEST_IDENTITIES);
    stream
        .write_all(&req)
        .map_err(|_e| mux_core::error::MuxError::SshAgentNotForwarded)?;

    // Read response: 4-byte length prefix, then body.
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .map_err(|_| mux_core::error::MuxError::SshAgentNotForwarded)?;
    let resp_len = u32::from_be_bytes(len_buf) as usize;

    if resp_len == 0 || resp_len > MAX_AGENT_REPLY {
        tracing::warn!("ssh agent reply length {resp_len} out of bounds (max {MAX_AGENT_REPLY})");
        return Err(mux_core::error::MuxError::SshAgentNotForwarded);
    }

    let mut body = vec![0u8; resp_len];
    stream
        .read_exact(&mut body)
        .map_err(|_| mux_core::error::MuxError::SshAgentNotForwarded)?;

    // Parse the response type.
    match body.first().copied() {
        Some(t) if t == SSH2_AGENT_IDENTITIES_ANSWER => {}
        Some(SSH_AGENT_FAILURE) => {
            tracing::warn!("ssh agent returned SSH_AGENT_FAILURE");
            return Err(mux_core::error::MuxError::SshAgentNotForwarded);
        }
        other => {
            tracing::warn!("unexpected SSH agent response type: {:?}", other);
            return Err(mux_core::error::MuxError::SshAgentNotForwarded);
        }
    }

    let mut pos = 1usize;
    let count = match read_u32_be(&body, &mut pos) {
        Some(n) => n as usize,
        None => return Ok(Vec::new()),
    };

    // Clamp capacity: real agents hold at most a handful of keys.
    let mut keys = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        let key_blob = match read_bytes(&body, &mut pos) {
            Some(b) => b,
            None => break,
        };
        let comment = match read_string(&body, &mut pos) {
            Some(s) => s,
            None => break,
        };

        // Extract algorithm name from the start of the key blob.
        let algorithm = {
            let mut kb_pos = 0usize;
            let alg = read_string(&key_blob, &mut kb_pos).unwrap_or_default();
            if alg.is_empty() {
                tracing::warn!("ssh agent key has unreadable algorithm name; key will be skipped by preference selection");
            }
            alg
        };

        keys.push(AgentKey {
            algorithm,
            key_blob,
            comment,
        });
    }

    Ok(keys)
}

// ── Wire format helpers ───────────────────────────────────────────────────────

fn read_u32_be(buf: &[u8], pos: &mut usize) -> Option<u32> {
    if *pos + 4 > buf.len() {
        return None;
    }
    let val = u32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]);
    *pos += 4;
    Some(val)
}

fn read_bytes(buf: &[u8], pos: &mut usize) -> Option<Vec<u8>> {
    let len = read_u32_be(buf, pos)? as usize;
    if *pos + len > buf.len() {
        return None;
    }
    let bytes = buf[*pos..*pos + len].to_vec();
    *pos += len;
    Some(bytes)
}

fn read_string(buf: &[u8], pos: &mut usize) -> Option<String> {
    let bytes = read_bytes(buf, pos)?;
    String::from_utf8(bytes).ok()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mux_state::store::Store;
    use tempfile::TempDir;

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store = Store::open(&db_path).unwrap();
        (dir, store)
    }

    fn insert_host(conn: &rusqlite::Connection) -> i64 {
        mux_state::host_repo::insert(conn, "test", "user", "127.0.0.1", 22, 1_000_000).unwrap()
    }

    // ── check_host_key ────────────────────────────────────────────────────────

    #[test]
    fn check_host_key_first_contact() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);

        let result = check_host_key(conn, host_id, "ssh-ed25519", "AAAA1234").unwrap();
        assert_eq!(
            result,
            TrustCheckResult::FirstContact {
                algorithm: "ssh-ed25519".into(),
                fingerprint: "AAAA1234".into(),
            }
        );
    }

    #[test]
    fn check_host_key_trusted() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        fingerprint_repo::upsert(conn, host_id, "ssh-ed25519", "AAAA1234", 1_000_000).unwrap();

        let result = check_host_key(conn, host_id, "ssh-ed25519", "AAAA1234").unwrap();
        assert_eq!(result, TrustCheckResult::Trusted);
    }

    #[test]
    fn check_host_key_mismatch() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        fingerprint_repo::upsert(conn, host_id, "ssh-ed25519", "AAAA1234", 1_000_000).unwrap();

        let result = check_host_key(conn, host_id, "ssh-ed25519", "BBBB5678").unwrap();
        assert_eq!(
            result,
            TrustCheckResult::Mismatch {
                algorithm: "ssh-ed25519".into(),
                stored: "AAAA1234".into(),
                received: "BBBB5678".into(),
            }
        );
    }

    // ── trust_fingerprint ─────────────────────────────────────────────────────

    #[test]
    fn trust_fingerprint_stores_and_updates() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);

        trust_fingerprint(conn, host_id, "ssh-ed25519", "AAAA1234").unwrap();
        let first = fingerprint_repo::get(conn, host_id, "ssh-ed25519")
            .unwrap()
            .expect("should exist after first trust");
        assert_eq!(first.fingerprint, "AAAA1234");

        trust_fingerprint(conn, host_id, "ssh-ed25519", "BBBB5678").unwrap();
        let second = fingerprint_repo::get(conn, host_id, "ssh-ed25519")
            .unwrap()
            .expect("should exist after second trust");
        assert_eq!(second.fingerprint, "BBBB5678", "second call should overwrite");
    }

    // ── preferred_fingerprint_for_attach ──────────────────────────────────────

    fn make_fp(algorithm: &str, fingerprint: &str) -> mux_state::model::KnownHostFingerprint {
        mux_state::model::KnownHostFingerprint {
            id: 1,
            host_id: 1,
            algorithm: algorithm.to_owned(),
            fingerprint: fingerprint.to_owned(),
            trusted_at: 1_000_000,
        }
    }

    #[test]
    fn preferred_fingerprint_ed25519_first() {
        let fps = vec![
            make_fp("rsa-sha2-256", "RSA256FP"),
            make_fp("ssh-ed25519", "ED25519FP"),
        ];
        let result = preferred_fingerprint_for_attach(&fps).expect("should return a fingerprint");
        assert_eq!(result.algorithm, "ssh-ed25519");
        assert_eq!(result.fingerprint, "ED25519FP");
    }

    #[test]
    fn preferred_fingerprint_falls_back_to_arbitrary() {
        let fps = vec![make_fp("x-unknown-algo", "UNKNOWNFP")];
        let result = preferred_fingerprint_for_attach(&fps).expect("should return a fingerprint");
        assert_eq!(result.algorithm, "x-unknown-algo");
        assert_eq!(result.fingerprint, "UNKNOWNFP");
    }

    #[test]
    fn preferred_fingerprint_empty_slice() {
        let fps: Vec<mux_state::model::KnownHostFingerprint> = vec![];
        assert!(preferred_fingerprint_for_attach(&fps).is_none());
    }

    // ── preferred_fingerprint ordering ───────────────────────────────────────

    #[test]
    fn preferred_fingerprint_ecdsa_before_rsa512() {
        let fps = vec![
            make_fp("rsa-sha2-512", "RSA512FP"),
            make_fp("ecdsa-sha2-nistp256", "ECDSAFP"),
        ];
        let result = preferred_fingerprint_for_attach(&fps).expect("should return a fingerprint");
        assert_eq!(result.algorithm, "ecdsa-sha2-nistp256");
    }

    #[test]
    fn preferred_fingerprint_rsa512_before_rsa256() {
        let fps = vec![
            make_fp("rsa-sha2-256", "RSA256FP"),
            make_fp("rsa-sha2-512", "RSA512FP"),
        ];
        let result = preferred_fingerprint_for_attach(&fps).expect("should return a fingerprint");
        assert_eq!(result.algorithm, "rsa-sha2-512");
    }

    #[test]
    fn preferred_fingerprint_full_order_ed25519_wins() {
        let fps = vec![
            make_fp("rsa-sha2-256", "RSA256FP"),
            make_fp("rsa-sha2-512", "RSA512FP"),
            make_fp("ecdsa-sha2-nistp256", "ECDSAFP"),
            make_fp("ssh-ed25519", "ED25519FP"),
        ];
        let result = preferred_fingerprint_for_attach(&fps).expect("should return a fingerprint");
        assert_eq!(result.algorithm, "ssh-ed25519");
    }

    // ── list_agent_keys wire protocol ────────────────────────────────────────

    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    /// Binds a Unix socket at `path` and spawns a background thread that accepts
    /// one connection, drains the request, and writes `response`. The socket is
    /// bound before this function returns.
    fn spawn_agent_responder(path: &std::path::Path, response: Vec<u8>) {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 64];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(&response);
            }
        });
    }

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut v = (body.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    fn agent_failure_frame() -> Vec<u8> {
        frame(&[SSH_AGENT_FAILURE])
    }

    /// A length prefix exceeding MAX_AGENT_REPLY with no body bytes following.
    fn agent_oversized_len_frame() -> Vec<u8> {
        ((MAX_AGENT_REPLY as u32) + 1).to_be_bytes().to_vec()
    }

    fn agent_one_ed25519_key_frame() -> Vec<u8> {
        let alg = b"ssh-ed25519";
        let key_data = [0u8; 32];

        // key blob: [4-byte len][alg bytes][4-byte len][key bytes]
        let mut blob = Vec::new();
        blob.extend_from_slice(&(alg.len() as u32).to_be_bytes());
        blob.extend_from_slice(alg);
        blob.extend_from_slice(&(key_data.len() as u32).to_be_bytes());
        blob.extend_from_slice(&key_data);

        let comment = b"test-key";

        let mut body = Vec::new();
        body.push(SSH2_AGENT_IDENTITIES_ANSWER);
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&(blob.len() as u32).to_be_bytes());
        body.extend_from_slice(&blob);
        body.extend_from_slice(&(comment.len() as u32).to_be_bytes());
        body.extend_from_slice(comment);

        frame(&body)
    }

    #[test]
    fn list_agent_keys_unconnectable_socket() {
        // Point SSH_AUTH_SOCK to a nonexistent path so the connect fails.
        std::env::set_var("SSH_AUTH_SOCK", "/nonexistent/path/agent.sock");
        let result = list_agent_keys();
        assert!(
            matches!(result, Err(mux_core::error::MuxError::SshAgentNotForwarded)),
            "expected SshAgentNotForwarded, got {result:?}"
        );
    }

    #[test]
    fn list_agent_keys_no_auth_sock_env_returns_not_forwarded() {
        std::env::remove_var("SSH_AUTH_SOCK");
        let result = list_agent_keys();
        assert!(
            matches!(result, Err(mux_core::error::MuxError::SshAgentNotForwarded)),
            "expected SshAgentNotForwarded when SSH_AUTH_SOCK is unset, got {result:?}"
        );
    }

    #[test]
    fn list_agent_keys_agent_returns_failure() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("agent-fail.sock");
        spawn_agent_responder(&sock, agent_failure_frame());
        std::env::set_var("SSH_AUTH_SOCK", sock.to_str().unwrap());
        let result = list_agent_keys();
        assert!(
            matches!(result, Err(mux_core::error::MuxError::SshAgentNotForwarded)),
            "expected SshAgentNotForwarded for SSH_AGENT_FAILURE, got {result:?}"
        );
    }

    #[test]
    fn list_agent_keys_oversized_reply_returns_not_forwarded() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("agent-oversized.sock");
        spawn_agent_responder(&sock, agent_oversized_len_frame());
        std::env::set_var("SSH_AUTH_SOCK", sock.to_str().unwrap());
        let result = list_agent_keys();
        assert!(
            matches!(result, Err(mux_core::error::MuxError::SshAgentNotForwarded)),
            "expected SshAgentNotForwarded for oversized reply, got {result:?}"
        );
    }

    #[test]
    fn list_agent_keys_valid_reply_returns_keys() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("agent-ok.sock");
        spawn_agent_responder(&sock, agent_one_ed25519_key_frame());
        std::env::set_var("SSH_AUTH_SOCK", sock.to_str().unwrap());
        let result = list_agent_keys().expect("should succeed with valid agent response");
        assert_eq!(result.len(), 1, "expected exactly one key");
        assert_eq!(result[0].algorithm, "ssh-ed25519");
        assert_eq!(result[0].comment, "test-key");
        assert!(!result[0].key_blob.is_empty());
    }
}
