//! Spec: docs/01 §mux status, docs/07 §Status flow

use anyhow::Result;
use rusqlite::Connection;

use mux_core::error::MuxError;
use mux_rpc::client::RpcClient;
use mux_rpc::schema::{GetSessionRequest, SessionStatusValue};
use mux_state::host_repo;
use mux_state::model::Session;

use crate::agent_start::{AgentStarter, RemoteExec};
use crate::kill::resolve_session;

// ── Internal types ────────────────────────────────────────────────────────────

enum LiveResult {
    Live(mux_rpc::schema::GetSessionResponse),
    AgentNotFound,
    NoAgent,
    ProbeError(String),
    HostNotProbed,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Execution context for `mux status`.
pub struct StatusContext<'a, E: RemoteExec> {
    pub conn: &'a Connection,
    /// Shell executor on the session's remote host (used to probe agent lock).
    pub ssh: E,
    /// UUID or shortname of the session.
    pub selector: String,
}

/// Show session status.
///
/// Implements the status flow from docs/07:
/// 1. Resolve selector (UUID first; UUID format not found → hard error).
/// 2. Load session and host from SQLite.
/// 3. Attempt `GetSession` RPC via the running agent.
///    - Success: display live data.
///    - Agent not running or host unreachable: display local SQLite data, note it.
///    - Other RPC error: surface it.
/// 4. No mutation of session status.
///
/// No TOFU host-key check — status is a read-only, best-effort refresh.
pub async fn run_status<E: RemoteExec>(ctx: StatusContext<'_, E>) -> Result<()> {
    // Step 1 — resolve selector
    let session = resolve_session(ctx.conn, &ctx.selector)?;

    // Step 2 — load host
    let host = host_repo::get_by_id(ctx.conn, session.host_id)?
        .ok_or_else(|| anyhow::anyhow!("mux: host record missing for session '{}'", ctx.selector))?;

    // Step 3 — attempt live GetSession RPC (no TOFU; read-only probe)
    //
    // host.home must have been set by `mux host test`; without it we cannot
    // know where agent.lock lives, so skip the live probe and fall back to
    // local data with a note.
    let live_result = if let Some(home) = host.home.as_deref() {
        let starter = AgentStarter::new(home, ctx.ssh);
        match starter.probe_existing() {
            Ok(Some(agent_urls)) => {
                let rpc = RpcClient::tcp("127.0.0.1", agent_urls.tcp_port());
                match rpc.get_session(GetSessionRequest { uuid: session.uuid.clone() }).await {
                    Ok(resp) => LiveResult::Live(resp),
                    Err(MuxError::AgentError(ref msg)) if msg.starts_with("not_found") => {
                        // Agent is running but does not own this session — possible drift.
                        LiveResult::AgentNotFound
                    }
                    Err(e) => return Err(anyhow::anyhow!("{e}")),
                }
            }
            Ok(None) => LiveResult::NoAgent,
            Err(e) => {
                // Probe failed (SSH error, corrupt lock, etc.) — show reason in note.
                LiveResult::ProbeError(e.to_string())
            }
        }
    } else {
        LiveResult::HostNotProbed
    };

    // Step 4 — display
    match live_result {
        LiveResult::Live(resp) => {
            print_session_live(&session, &host.alias, status_to_str(&resp.status), &resp.tmux_name);
        }
        LiveResult::AgentNotFound => {
            print_session_local(
                &session,
                &host.alias,
                "agent reachable but has no record of this session (possibly orphaned)",
            );
        }
        LiveResult::NoAgent => {
            print_session_local(&session, &host.alias, "agent not running");
        }
        LiveResult::ProbeError(ref reason) => {
            print_session_local(
                &session,
                &host.alias,
                &format!("could not probe agent: {reason}"),
            );
        }
        LiveResult::HostNotProbed => {
            print_session_local(&session, &host.alias, "host not yet probed (run 'mux host test')");
        }
    }

    Ok(())
}

fn status_to_str(s: &SessionStatusValue) -> &'static str {
    match s {
        SessionStatusValue::Active => "active",
        SessionStatusValue::Dead => "dead",
        SessionStatusValue::Unreachable => "unreachable",
        SessionStatusValue::Orphaned => "orphaned",
    }
}

fn print_session_live(session: &Session, host_alias: &str, live_status: &str, live_tmux: &str) {
    println!("uuid:      {}", session.uuid);
    println!("shortname: {}", session.shortname);
    println!("host:      {}", host_alias);
    println!("status:    {}", live_status);
    println!("tmux:      {}", live_tmux);
    if let Some(ref workdir) = session.workdir {
        println!("workdir:   {}", workdir);
    }
    println!("branch:    {}", session.branch);
    println!("repo:      {}", session.repo_slug);
}

fn print_session_local(session: &Session, host_alias: &str, note: &str) {
    println!("uuid:      {}", session.uuid);
    println!("shortname: {}", session.shortname);
    println!("host:      {}", host_alias);
    println!("status:    {} (local)", session.status);
    if let Some(ref tmux) = session.tmux_name {
        println!("tmux:      {}", tmux);
    }
    if let Some(ref workdir) = session.workdir {
        println!("workdir:   {}", workdir);
    }
    println!("branch:    {}", session.branch);
    println!("repo:      {}", session.repo_slug);
    println!("note:      {}", note);
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Behavioral contract tests live in crates/mux-cli/tests/status.rs (integration
// tests that exercise the public API). Only unit tests requiring private access
// to `status_to_str` are kept here.

#[cfg(test)]
mod tests {
    use super::*;

    // ── status_to_str unit test (private fn; cannot be in integration tests) ──

    #[test]
    fn status_to_str_all_variants() {
        use mux_rpc::schema::SessionStatusValue;
        assert_eq!(status_to_str(&SessionStatusValue::Active), "active");
        assert_eq!(status_to_str(&SessionStatusValue::Dead), "dead");
        assert_eq!(status_to_str(&SessionStatusValue::Unreachable), "unreachable");
        assert_eq!(status_to_str(&SessionStatusValue::Orphaned), "orphaned");
    }
}

