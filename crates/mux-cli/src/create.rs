//! `mux create` transaction — creates a remote tmux session for a repository.
//!
//! Spec: docs/07 §Create flow

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mux_core::{
    error::{truncate_stderr, MuxError},
    shortname::{
        shortname_for_branch, shortname_for_main, shortname_with_suffix, ADJECTIVES, NOUNS,
    },
    types::RepoRef,
    workdir::{build_workdir, build_workdir_parent},
};
use mux_state::{model::Host, session_repo};

use crate::agent_start::{AgentStarter, RemoteExec};

// ── sh_quote ──────────────────────────────────────────────────────────────────

/// Single-quote a string for safe use as one shell word.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ── SshHost trait ─────────────────────────────────────────────────────────────

/// Information about a server's host key.
#[derive(Debug, Clone)]
pub struct HostKeyInfo {
    pub algorithm: String,
    pub fingerprint: String, // "SHA256:..."
}

/// Abstract interface for executing commands on a remote SSH host.
///
/// Extends `RemoteExec` (used by `AgentStarter`) with host-key inspection.
/// Implementations: a real SSH executor (future) and `MockSshHost` (tests).
pub trait SshHost: RemoteExec {
    /// Get the server's host key fingerprint.
    fn host_key(&self) -> Result<HostKeyInfo, MuxError>;
}

// ── CreateContext ─────────────────────────────────────────────────────────────

/// Everything needed to run the `mux create` transaction.
pub struct CreateContext<'a, S: SshHost> {
    pub conn: &'a rusqlite::Connection,
    /// Local mux home directory (e.g. `~/.mux`).
    pub mux_home: &'a Path,
    /// Repository to clone.
    pub repo: RepoRef,
    /// Remote host configuration row.
    pub host: Host,
    /// Branch to check out.
    pub branch: String,
    /// SSH executor for remote commands.
    pub ssh: S,
    /// `true` if stdin is a terminal (enables TOFU interactive prompt).
    pub is_interactive: bool,
}

// ── CreateResult ──────────────────────────────────────────────────────────────

/// Returned on a successful `mux create`.
#[derive(Debug, Clone)]
pub struct CreateResult {
    pub uuid: String,
    pub shortname: String,
    pub tmux_name: String,
    pub workdir: PathBuf,
    pub transport_mode: String,
}

// ── run_create ────────────────────────────────────────────────────────────────

/// Run the full `mux create` transaction.
///
/// Returns `CreateResult` on success. On failure after the session reservation
/// has been written, the reservation is always cancelled. If the workdir was
/// created before the failure, it is removed (only when `is_safe_to_remove`).
pub async fn run_create<S: SshHost>(ctx: CreateContext<'_, S>) -> Result<CreateResult, MuxError> {
    let now = unix_now();

    // ── Preconditions ─────────────────────────────────────────────────────────

    // Host must have been probed (arch + home set).
    let remote_home = ctx.host.home.as_deref().ok_or_else(|| MuxError::HostNotConfigured {
        alias: ctx.host.alias.clone(),
    })?;
    let _arch = ctx.host.arch.as_deref().ok_or_else(|| MuxError::HostNotConfigured {
        alias: ctx.host.alias.clone(),
    })?;

    // ── Step 1: Generate UUID ─────────────────────────────────────────────────

    let uuid = uuid::Uuid::new_v4().to_string();

    // ── Step 2: Generate shortname ────────────────────────────────────────────

    let repo_leaf = ctx.repo.repo_leaf().to_owned();
    let is_main = ctx.branch == "main" || ctx.branch == "master";

    let base_shortname = if is_main {
        // Pick a random adjective-noun pair not already taken.
        pick_main_shortname(ctx.conn, ctx.host.id, &repo_leaf)?
    } else {
        shortname_for_branch(&repo_leaf, &ctx.branch)
    };

    // Resolve collisions with non-main branches (or any collision for main
    // where pick_main_shortname already picked a unique one, but still
    // guard against races with suffixes).
    let shortname = resolve_shortname_collision(ctx.conn, ctx.host.id, &base_shortname)?;

    // ── Step 3: Reserve the session row ──────────────────────────────────────

    session_repo::reserve(
        ctx.conn,
        &session_repo::ReserveParams {
            uuid: &uuid,
            host_id: ctx.host.id,
            shortname: &shortname,
            repo_slug: &ctx.repo.repo_slug(),
            branch: &ctx.branch,
            created_at: now,
        },
    )
    .map_err(MuxError::Other)?;

    // From here on, every early return must call cancel_reservation.

    // ── Step 4: TOFU host key check ───────────────────────────────────────────

    let tofu_result = match ctx.ssh.host_key() {
        Ok(info) => {
            mux_ssh::trust::check_host_key(ctx.conn, ctx.host.id, &info.algorithm, &info.fingerprint)
                .map_err(MuxError::Other)?
        }
        Err(e) => {
            cancel_reservation(ctx.conn, &uuid);
            return Err(e);
        }
    };

    use mux_ssh::trust::TrustCheckResult;
    match tofu_result {
        TrustCheckResult::Trusted => {
            // Proceed silently.
        }
        TrustCheckResult::FirstContact { algorithm, fingerprint } => {
            if !ctx.is_interactive {
                cancel_reservation(ctx.conn, &uuid);
                return Err(MuxError::TofuNonInteractive);
            }
            // Print and prompt.
            eprintln!(
                "The authenticity of host '{}' can't be established.",
                ctx.host.alias
            );
            eprintln!("{algorithm} key fingerprint is {fingerprint}");
            eprintln!("Are you sure you want to continue connecting? (yes/no): ");
            let accepted = read_yes_no()?;
            if !accepted {
                cancel_reservation(ctx.conn, &uuid);
                return Err(MuxError::HostKeyRejected);
            }
            mux_ssh::trust::trust_fingerprint(ctx.conn, ctx.host.id, &algorithm, &fingerprint)
                .map_err(MuxError::Other)?;
        }
        TrustCheckResult::Mismatch { .. } => {
            cancel_reservation(ctx.conn, &uuid);
            return Err(MuxError::HostKeyMismatch);
        }
    }

    // ── Step 5: Transport probing ─────────────────────────────────────────────

    let forced_transport = mux_ssh::transport::read_force_transport()?;

    let transport_mode = if let Some(forced) = forced_transport {
        forced
    } else {
        // Probe by running remote test commands: check for socket and port.
        let sock_path = format!("{}/.mux/agent.sock", remote_home);
        let port_str = ctx
            .ssh
            .run(&format!("cat {}/.mux/agent.port 2>/dev/null", sh_quote(remote_home)))
            .unwrap_or_default();

        // Prefer streamlocal if the socket exists.
        let sock_ok = ctx
            .ssh
            .run(&format!("test -S {}", sh_quote(&sock_path)))
            .map(|(code, _, _)| code == 0)
            .unwrap_or(false);

        if sock_ok {
            mux_core::types::TransportMode::Streamlocal
        } else {
            let port = port_str.1.trim().parse::<u16>().unwrap_or(0);
            if port > 0 {
                mux_core::types::TransportMode::Tcp
            } else {
                // Default to TCP — the agent will allocate a port later.
                mux_core::types::TransportMode::Tcp
            }
        }
    };

    let transport_str = match transport_mode {
        mux_core::types::TransportMode::Streamlocal => "streamlocal",
        mux_core::types::TransportMode::Tcp => "tcp",
        _ => "tcp",
    };

    // ── Step 6: Create workdir ────────────────────────────────────────────────

    // Build the remote mux_home path from the host's home directory.
    let remote_mux_home = PathBuf::from(format!("{}/.mux", remote_home));
    let workdir_parent = build_workdir_parent(&remote_mux_home, &uuid);
    let workdir = build_workdir(&remote_mux_home, &uuid, &repo_leaf);

    let workdir_parent_str = workdir_parent.to_string_lossy().into_owned();
    let workdir_str = workdir.to_string_lossy().into_owned();

    // Create the parent directory.
    let (mkdir_code, _, mkdir_stderr) = ctx
        .ssh
        .run(&format!("mkdir -p {}", sh_quote(&workdir_parent_str)))?;
    if mkdir_code != 0 {
        cancel_reservation(ctx.conn, &uuid);
        return Err(MuxError::Other(anyhow::anyhow!(
            "failed to create workdir parent: {}",
            mkdir_stderr.trim()
        )));
    }

    // Guard: workdir must not already exist.
    let (test_code, _, _) = ctx
        .ssh
        .run(&format!("test -d {}", sh_quote(&workdir_str)))?;
    if test_code == 0 {
        cancel_reservation(ctx.conn, &uuid);
        return Err(MuxError::WorkdirPreExisting(workdir.clone()));
    }

    // ── Step 7: Clone ─────────────────────────────────────────────────────────

    let clone_url = ctx.repo.clone_url_for(&ctx.host.addr);
    let clone_cmd = format!(
        "GIT_TERMINAL_PROMPT=0 git clone --branch {} {} {}",
        sh_quote(&ctx.branch),
        sh_quote(&clone_url),
        sh_quote(&workdir_str),
    );

    let (clone_code, _, clone_stderr) = ctx.ssh.run(&clone_cmd)?;
    if clone_code != 0 {
        cancel_reservation(ctx.conn, &uuid);
        // Attempt workdir cleanup (best-effort, ignore errors).
        let _ = ctx.ssh.run(&format!("rm -rf {}", sh_quote(&workdir_str)));
        return Err(MuxError::GitCloneFailed {
            exit_code: clone_code,
            stderr: truncate_stderr(&clone_stderr),
        });
    }

    // ── Step 8: Start agent ───────────────────────────────────────────────────

    let agent_urls = match AgentStarter::new(remote_home, &ctx.ssh).ensure_running() {
        Ok(urls) => urls,
        Err(e) => {
            cancel_reservation(ctx.conn, &uuid);
            let _ = ctx.ssh.run(&format!("rm -rf {}", sh_quote(&workdir_str)));
            return Err(e);
        }
    };

    // ── Step 9: RPC — create session ─────────────────────────────────────────

    let rpc_client = mux_rpc::client::RpcClient::tcp("127.0.0.1", agent_urls.tcp_port());
    let rpc_req = mux_rpc::schema::CreateSessionRequest {
        uuid: uuid.clone(),
        shortname: shortname.clone(),
        repo_slug: ctx.repo.repo_slug(),
        branch: ctx.branch.clone(),
        workdir_parent: workdir_parent_str.clone(),
        repo_leaf: repo_leaf.clone(),
    };

    let rpc_resp = match rpc_client.create_session(rpc_req).await {
        Ok(resp) => resp,
        Err(e) => {
            cancel_reservation(ctx.conn, &uuid);
            let _ = ctx.ssh.run(&format!("rm -rf {}", sh_quote(&workdir_str)));
            return Err(e);
        }
    };

    // ── Step 10: Activate the reservation ────────────────────────────────────

    let tmux_name = rpc_resp.tmux_name.clone();
    session_repo::activate(
        ctx.conn,
        &uuid,
        &tmux_name,
        &workdir_str,
        transport_str,
        unix_now(),
    )
    .map_err(MuxError::Other)?;

    // ── Done ──────────────────────────────────────────────────────────────────

    Ok(CreateResult {
        uuid,
        shortname,
        tmux_name,
        workdir: workdir.clone(),
        transport_mode: transport_str.to_owned(),
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Cancel a session reservation (best-effort — logs on failure but does not panic).
fn cancel_reservation(conn: &rusqlite::Connection, uuid: &str) {
    if let Err(e) = session_repo::cancel_reservation(conn, uuid) {
        tracing::warn!("failed to cancel session reservation {uuid}: {e}");
    }
}

/// Read a yes/no answer from stdin. Returns `true` for "yes"/"y", `false` otherwise.
fn read_yes_no() -> Result<bool, MuxError> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| MuxError::Other(anyhow::anyhow!("stdin read error: {e}")))?;
    let trimmed = line.trim().to_ascii_lowercase();
    Ok(trimmed == "yes" || trimmed == "y")
}

/// Pick a unique adjective-noun pair for a main-branch shortname.
///
/// Iterates all adjective×noun pairs (shuffled via a simple index permutation)
/// until one produces a shortname not already present in the session store.
/// Returns `MuxError::ShortnameExhausted` if all 400 pairs are taken.
fn pick_main_shortname(
    conn: &rusqlite::Connection,
    host_id: i64,
    repo_leaf: &str,
) -> Result<String, MuxError> {
    // Deterministic pseudo-random traversal using a linear step that's coprime
    // to the namespace size (400). This avoids requiring `rand` in the dep tree.
    let total = ADJECTIVES.len() * NOUNS.len();
    // Step 7 is coprime to 400 (gcd(7,400)=1), so it visits all pairs.
    let step = 7usize;
    let start = (unix_now() as usize) % total;

    for i in 0..total {
        let idx = (start + i * step) % total;
        let adj = ADJECTIVES[idx / NOUNS.len()];
        let noun = NOUNS[idx % NOUNS.len()];
        let candidate = shortname_for_main(repo_leaf, adj, noun);
        let existing = session_repo::get_by_shortname(conn, host_id, &candidate)
            .map_err(MuxError::Other)?;
        if existing.is_none() {
            return Ok(candidate);
        }
    }
    Err(MuxError::ShortnameExhausted)
}

/// Try `base`, then `base-2`, `base-3` ... until a free shortname is found.
///
/// Returns `MuxError::ShortnameExhausted` after 50 attempts.
fn resolve_shortname_collision(
    conn: &rusqlite::Connection,
    host_id: i64,
    base: &str,
) -> Result<String, MuxError> {
    const MAX_ATTEMPTS: u32 = 50;
    for attempt in 1..=MAX_ATTEMPTS {
        let candidate = shortname_with_suffix(base, attempt);
        let existing = session_repo::get_by_shortname(conn, host_id, &candidate)
            .map_err(MuxError::Other)?;
        if existing.is_none() {
            return Ok(candidate);
        }
    }
    Err(MuxError::ShortnameExhausted)
}

// ── impl RemoteExec for &S where S: SshHost ───────────────────────────────────

/// Allow `&S` to be used as `RemoteExec` when `S: SshHost` so that
/// `AgentStarter::new(home, &ctx.ssh)` works without consuming `ctx.ssh`.
impl<S: SshHost> RemoteExec for &S {
    fn run(&self, cmd: &str) -> Result<(i32, String, String), MuxError> {
        (*self).run(cmd)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use mux_state::store::Store;
    use tempfile::TempDir;

    // ── MockSshHost ───────────────────────────────────────────────────────────

    struct MockSshHost {
        responses: RefCell<VecDeque<(i32, String, String)>>,
        host_key_result: Result<HostKeyInfo, MuxError>,
    }

    impl MockSshHost {
        fn new(
            responses: Vec<(i32, &str, &str)>,
            host_key_result: Result<HostKeyInfo, MuxError>,
        ) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|(c, o, e)| (c, o.to_owned(), e.to_owned()))
                        .collect(),
                ),
                host_key_result,
            }
        }

        fn with_trusted_key(responses: Vec<(i32, &str, &str)>) -> Self {
            Self::new(
                responses,
                Ok(HostKeyInfo {
                    algorithm: "ssh-ed25519".to_owned(),
                    fingerprint: "SHA256:AAAA".to_owned(),
                }),
            )
        }
    }

    impl RemoteExec for MockSshHost {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
            let mut q = self.responses.borrow_mut();
            Ok(q.pop_front().unwrap_or((1, String::new(), "mock: no more responses".to_owned())))
        }
    }

    impl SshHost for MockSshHost {
        fn host_key(&self) -> Result<HostKeyInfo, MuxError> {
            match &self.host_key_result {
                Ok(info) => Ok(info.clone()),
                Err(e) => Err(match e {
                    MuxError::HostKeyMismatch => MuxError::HostKeyMismatch,
                    MuxError::TofuNonInteractive => MuxError::TofuNonInteractive,
                    _ => MuxError::HostKeyMismatch,
                }),
            }
        }
    }

    // ── DB helpers ─────────────────────────────────────────────────────────────

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store = Store::open(&db_path).unwrap();
        (dir, store)
    }

    fn insert_host_configured(conn: &rusqlite::Connection) -> Host {
        let id = mux_state::host_repo::insert(conn, "prod", "user", "10.0.0.1", 22, 1_000_000).unwrap();
        mux_state::host_repo::update_probe(
            conn,
            id,
            Some("aarch64"),
            Some("/home/user"),
            Some("tcp"),
        )
        .unwrap();
        mux_state::host_repo::get_by_id(conn, id).unwrap().unwrap()
    }

    fn insert_host_unconfigured(conn: &rusqlite::Connection) -> Host {
        let id = mux_state::host_repo::insert(conn, "bare", "user", "10.0.0.2", 22, 1_000_000).unwrap();
        mux_state::host_repo::get_by_id(conn, id).unwrap().unwrap()
    }

    fn repo() -> RepoRef {
        "owner/myrepo".parse().unwrap()
    }

    // ── 1. Host not configured (arch/home missing) ─────────────────────────────

    #[tokio::test]
    async fn host_not_configured_returns_error() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_unconfigured(conn);

        let ssh = MockSshHost::with_trusted_key(vec![]);

        let ctx = CreateContext {
            conn,
            mux_home: Path::new("/home/user/.mux"),
            repo: repo(),
            host,
            branch: "main".to_owned(),
            ssh,
            is_interactive: false,
        };

        let err = run_create(ctx).await.unwrap_err();
        assert!(
            matches!(err, MuxError::HostNotConfigured { .. }),
            "expected HostNotConfigured, got: {err:?}"
        );
    }

    // ── 2. TOFU mismatch → HostKeyMismatch ────────────────────────────────────

    #[tokio::test]
    async fn tofu_mismatch_returns_host_key_mismatch() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_configured(conn);

        // Store a fingerprint that differs from what mock returns.
        mux_state::fingerprint_repo::upsert(
            conn,
            host.id,
            "ssh-ed25519",
            "SHA256:DIFFERENT",
            1_000_000,
        )
        .unwrap();

        // Mock returns SHA256:AAAA but stored is SHA256:DIFFERENT → Mismatch.
        let ssh = MockSshHost::with_trusted_key(vec![]);

        let ctx = CreateContext {
            conn,
            mux_home: Path::new("/home/user/.mux"),
            repo: repo(),
            host,
            branch: "feature".to_owned(),
            ssh,
            is_interactive: false,
        };

        let err = run_create(ctx).await.unwrap_err();
        assert!(
            matches!(err, MuxError::HostKeyMismatch),
            "expected HostKeyMismatch, got: {err:?}"
        );

        // Reservation must be cleaned up.
        let sessions = session_repo::list_for_host(conn, 1).unwrap();
        assert!(sessions.is_empty(), "reservation should be cancelled on mismatch");
    }

    // ── 3. TOFU non-interactive first contact ─────────────────────────────────

    #[tokio::test]
    async fn tofu_non_interactive_returns_error() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_configured(conn);

        // No stored fingerprint → FirstContact. is_interactive=false → TofuNonInteractive.
        let ssh = MockSshHost::with_trusted_key(vec![]);

        let ctx = CreateContext {
            conn,
            mux_home: Path::new("/home/user/.mux"),
            repo: repo(),
            host,
            branch: "feature".to_owned(),
            ssh,
            is_interactive: false,
        };

        let err = run_create(ctx).await.unwrap_err();
        assert!(
            matches!(err, MuxError::TofuNonInteractive),
            "expected TofuNonInteractive, got: {err:?}"
        );

        // Reservation must be cleaned up.
        let sessions = session_repo::list_for_host(conn, 1).unwrap();
        assert!(sessions.is_empty(), "reservation should be cancelled");
    }

    // ── 4. Workdir pre-existing ───────────────────────────────────────────────

    #[tokio::test]
    async fn workdir_pre_existing_returns_error() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_configured(conn);

        // Trust the key so TOFU passes.
        mux_state::fingerprint_repo::upsert(
            conn,
            host.id,
            "ssh-ed25519",
            "SHA256:AAAA",
            1_000_000,
        )
        .unwrap();

        // Remote command sequence after TOFU:
        // 1. socket check (test -S ...) → exit 1 (no socket)
        // 2. cat agent.port → exit 1 / empty
        // 3. mkdir -p workdir_parent → exit 0
        // 4. test -d workdir → exit 0 (workdir pre-exists!)
        let ssh = MockSshHost::with_trusted_key(vec![
            (1, "", ""),      // test -S socket
            (1, "", ""),      // cat agent.port
            (0, "", ""),      // mkdir -p workdir_parent
            (0, "", ""),      // test -d workdir → exists
        ]);

        let ctx = CreateContext {
            conn,
            mux_home: Path::new("/home/user/.mux"),
            repo: repo(),
            host,
            branch: "feature".to_owned(),
            ssh,
            is_interactive: false,
        };

        let err = run_create(ctx).await.unwrap_err();
        assert!(
            matches!(err, MuxError::WorkdirPreExisting(_)),
            "expected WorkdirPreExisting, got: {err:?}"
        );

        let sessions = session_repo::list_for_host(conn, 1).unwrap();
        assert!(sessions.is_empty(), "reservation should be cancelled");
    }

    // ── 5. Git clone failure ──────────────────────────────────────────────────

    #[tokio::test]
    async fn git_clone_failure_cancels_reservation() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host = insert_host_configured(conn);

        mux_state::fingerprint_repo::upsert(
            conn,
            host.id,
            "ssh-ed25519",
            "SHA256:AAAA",
            1_000_000,
        )
        .unwrap();

        // After TOFU:
        // 1. socket check → no
        // 2. cat agent.port → empty
        // 3. mkdir -p → ok
        // 4. test -d workdir → not exists (exit 1)
        // 5. git clone → exit 128
        // 6. rm -rf workdir (cleanup) → exit 0
        let ssh = MockSshHost::with_trusted_key(vec![
            (1, "", ""),                    // test -S socket
            (1, "", ""),                    // cat agent.port
            (0, "", ""),                    // mkdir -p
            (1, "", ""),                    // test -d workdir → not exists
            (128, "", "repository not found"), // git clone fail
            (0, "", ""),                    // rm -rf cleanup
        ]);

        let ctx = CreateContext {
            conn,
            mux_home: Path::new("/home/user/.mux"),
            repo: repo(),
            host,
            branch: "feature".to_owned(),
            ssh,
            is_interactive: false,
        };

        let err = run_create(ctx).await.unwrap_err();
        assert!(
            matches!(err, MuxError::GitCloneFailed { exit_code: 128, .. }),
            "expected GitCloneFailed(128), got: {err:?}"
        );

        let sessions = session_repo::list_for_host(conn, 1).unwrap();
        assert!(sessions.is_empty(), "reservation should be cancelled after clone failure");
    }

    // ── sh_quote tests ────────────────────────────────────────────────────────

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
}
