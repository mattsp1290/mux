//! Spec: docs/01 §mux list, docs/07 §List flow

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;

#[cfg(test)]
use mux_core::error::MuxError;
use mux_rpc::client::RpcClient;
use mux_rpc::schema::{SessionInfo, SessionStatusValue};
use mux_state::model::Host;
use mux_state::session_repo::ImportParams;
use mux_state::{host_repo, session_repo};

use crate::agent_start::{AgentStarter, RemoteExec};

// ── Public API ────────────────────────────────────────────────────────────────

/// Execution context for `mux list`.
pub struct ListContext<'a, F> {
    pub conn: &'a Connection,
    /// Factory: given a `&Host`, return a `RemoteExec` for that host's SSH channel.
    /// Called once per host; must not perform TOFU (read-only probe).
    pub make_exec: F,
    pub plain: bool,
}

/// List sessions with per-host agent reconciliation.
///
/// Implements the list flow from docs/07:
/// 1. Load all non-dead sessions from SQLite, grouped by host.
/// 2. For each host: SSH health probe (no TOFU); if reachable call ListSessions.
/// 3. Reconcile per-session state mutations.
/// 4. Display: grouped by host, sorted by created_at ascending.
pub async fn run_list<F, E>(ctx: ListContext<'_, F>) -> Result<()>
where
    F: Fn(&Host) -> E,
    E: RemoteExec,
{
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let hosts = host_repo::list(ctx.conn)?;

    for host in &hosts {
        reconcile_host(ctx.conn, host, &ctx.make_exec, now).await?;
    }

    display_all(ctx.conn, &hosts, ctx.plain)
}

// ── Reconciliation ────────────────────────────────────────────────────────────

async fn reconcile_host<F, E>(
    conn: &Connection,
    host: &Host,
    make_exec: &F,
    now: i64,
) -> Result<()>
where
    F: Fn(&Host) -> E,
    E: RemoteExec,
{
    // Load ALL rows for this host (list_for_host filters tmux_name IS NOT NULL).
    // Two views are derived:
    //   - `all_uuids`: UUID set including dead rows — used as the import guard so a
    //     dead session's UUID is never re-imported (plain INSERT would hit UNIQUE).
    //   - `sessions`: non-dead rows only — used for the reconciliation state machine.
    let all_sessions = session_repo::list_for_host(conn, host.id)?;
    let all_uuids: HashSet<String> = all_sessions.iter().map(|s| s.uuid.clone()).collect();
    let sessions: Vec<_> = all_sessions
        .into_iter()
        .filter(|s| s.status != "dead")
        .collect();

    // Always probe the agent — even if DB has no sessions, the agent might have
    // live sessions to import (docs/07 rule 1: import unknown live sessions).
    let live_sessions = probe_agent(host, make_exec).await;

    // TODO (needs-manual): wrap per-host writes in BEGIN/COMMIT so a mid-host write
    // failure leaves no partial reconciliation state. Currently a write error aborts
    // all remaining hosts without rollback. Either transactional or best-effort
    // (log + continue to next host) would be an improvement.
    match live_sessions {
        None => {
            // Host unreachable: mark all `active` sessions as `unreachable`; leave others.
            for s in &sessions {
                if s.status == "active" {
                    session_repo::set_status(conn, &s.uuid, "unreachable", now)?;
                }
            }
        }
        Some(live) => {
            apply_reconciliation(conn, host, &sessions, &all_uuids, live, now)?;
        }
    }

    Ok(())
}

/// Returns the mux-prefixed agent sessions, or None if the agent is unreachable.
async fn probe_agent<F, E>(host: &Host, make_exec: &F) -> Option<Vec<SessionInfo>>
where
    F: Fn(&Host) -> E,
    E: RemoteExec,
{
    let home = host.home.as_deref()?;
    let exec = make_exec(host);
    let starter = AgentStarter::new(home, exec);

    let urls = match starter.probe_existing() {
        Ok(Some(u)) => u,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("mux: agent probe error on host '{}': {e}", host.alias);
            return None;
        }
    };

    let rpc = RpcClient::tcp("127.0.0.1", urls.tcp_port());
    let resp = match rpc.list_sessions().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("mux: ListSessions RPC failed on host '{}': {e}", host.alias);
            return None;
        }
    };

    // Filter to mux-prefixed sessions; these are the only ones we manage.
    let mux_sessions = resp
        .sessions
        .into_iter()
        .filter(|s| s.tmux_name.starts_with("mux-"))
        .collect();
    Some(mux_sessions)
}

fn apply_reconciliation(
    conn: &Connection,
    host: &Host,
    sessions: &[mux_state::model::Session],
    all_uuids: &HashSet<String>,
    live: Vec<SessionInfo>,
    now: i64,
) -> Result<()> {
    // Index agent sessions by UUID for fast lookup.
    let agent_by_uuid: HashMap<&str, &SessionInfo> =
        live.iter().map(|s| (s.uuid.as_str(), s)).collect();

    // `all_uuids` covers ALL DB rows for this host (including dead) so importing a
    // dead session UUID (which would fail the UNIQUE constraint) is prevented.

    // ── Reconcile DB sessions ─────────────────────────────────────────────────

    for s in sessions {
        // docs/07: dead or orphaned — skip; do not resurface.
        if s.status == "dead" || s.status == "orphaned" {
            continue;
        }

        if let Some(info) = agent_by_uuid.get(s.uuid.as_str()) {
            // UUID found in agent list → sync status (also handles resurrection).
            // NOTE (needs-manual): docs/07 specifies sync-to-active and unreachable→active
            // resurrection; it does not explicitly ask list to drive rows to terminal `dead`
            // or `orphaned` based on an agent report. Currently we sync unconditionally; if
            // an agent transiently reports `dead`, the row becomes permanently invisible.
            // Decide whether to restrict sync to non-terminal transitions.
            let new_status = agent_status_str(&info.status);
            if s.status != new_status {
                session_repo::set_status(conn, &s.uuid, new_status, now)?;
            }
        } else {
            // UUID not in agent list.
            let has_mux_prefix = s
                .tmux_name
                .as_deref()
                .map(|t| t.starts_with("mux-"))
                .unwrap_or(false);

            if has_mux_prefix {
                // docs/07: mux- session the agent no longer recognises → orphaned.
                // NOTE (needs-manual): this is a one-way door — orphaned rows are never
                // resurfaced (see skip above). If transient agent absence is possible,
                // consider `unreachable` (recoverable) here instead; confirm with spec owner.
                session_repo::set_status(conn, &s.uuid, "orphaned", now)?;
            } else if s.status == "active" {
                // Non-mux session missing from agent; mark unreachable.
                session_repo::set_status(conn, &s.uuid, "unreachable", now)?;
            }
        }
    }

    // ── Import unknown live sessions ──────────────────────────────────────────

    for info in &live {
        if all_uuids.contains(&info.uuid) {
            continue; // in DB (including dead rows — do not resurface)
        }

        // docs/07: live in agent with mux- prefix, UUID not in DB → import as active.
        let shortname = info
            .tmux_name
            .strip_prefix("mux-")
            .filter(|s| !s.is_empty())
            .unwrap_or(&info.shortname);

        // TODO: imported sessions have empty repo_slug/branch; the kill flow will
        // need to handle these gracefully when checking session ownership (docs/07 §Kill).
        session_repo::import_session(
            conn,
            &ImportParams {
                uuid: &info.uuid,
                host_id: host.id,
                shortname,
                tmux_name: Some(&info.tmux_name),
                repo_slug: "",
                branch: "",
                workdir: Some(&info.workdir),
                transport_mode: host.transport.as_deref(),
                created_at: now,
                updated_at: now,
            },
        )?;
    }

    Ok(())
}

fn agent_status_str(s: &SessionStatusValue) -> &'static str {
    match s {
        SessionStatusValue::Active => "active",
        SessionStatusValue::Dead => "dead",
        SessionStatusValue::Unreachable => "unreachable",
        SessionStatusValue::Orphaned => "orphaned",
    }
}

// ── Display ───────────────────────────────────────────────────────────────────

fn display_all(conn: &Connection, hosts: &[Host], plain: bool) -> Result<()> {
    let mut any = false;

    for host in hosts {
        // Re-read after reconciliation mutations.
        let sessions: Vec<_> = session_repo::list_for_host(conn, host.id)?
            .into_iter()
            .filter(|s| s.status != "dead")
            .collect();

        if sessions.is_empty() {
            continue;
        }
        any = true;

        if plain {
            // Stable tab-separated contract (do not reorder without a version bump):
            // host_alias \t shortname \t uuid \t status \t tmux_name \t workdir
            for s in &sessions {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    host.alias,
                    s.shortname,
                    s.uuid,
                    s.status,
                    s.tmux_name.as_deref().unwrap_or(""),
                    s.workdir.as_deref().unwrap_or(""),
                );
            }
        } else {
            let n = sessions.len();
            println!("{} ({} session{})", host.alias, n, if n == 1 { "" } else { "s" });
            for s in &sessions {
                println!(
                    "  {:<20} {:<36} {:<12} {}",
                    s.shortname,
                    s.uuid,
                    s.status,
                    s.workdir.as_deref().unwrap_or(""),
                );
            }
        }
    }

    if !any {
        println!("no sessions");
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// White-box unit tests only — scenarios requiring access to private symbols.
// Public-API reconciliation contract tests live in `tests/list.rs`.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use mux_state::session_repo::{activate, ReserveParams};
    use mux_state::store::Store;
    use tempfile::TempDir;

    struct MockExec {
        responses: RefCell<VecDeque<(i32, String, String)>>,
    }

    impl MockExec {
        fn new(responses: Vec<(i32, String, String)>) -> Self {
            MockExec { responses: RefCell::new(responses.into()) }
        }

        fn unreachable() -> Self {
            MockExec { responses: RefCell::new(VecDeque::new()) }
        }
    }

    impl RemoteExec for MockExec {
        fn run(&self, _cmd: &str) -> Result<(i32, String, String), MuxError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| MuxError::Other(anyhow::anyhow!("SSH unreachable (mock)")))
        }
    }

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("mux.db")).unwrap();
        (dir, store)
    }

    fn insert_host(conn: &Connection) -> i64 {
        let id = host_repo::insert(conn, "myhost", "user", "192.0.2.1", 22, 1_000_000).unwrap();
        host_repo::update_probe(conn, id, Some("amd64"), Some("/home/user"), Some("tcp")).unwrap();
        id
    }

    fn insert_active_session(conn: &Connection, host_id: i64, uuid: &str, shortname: &str) {
        session_repo::reserve(
            conn,
            &ReserveParams {
                uuid,
                host_id,
                shortname,
                repo_slug: "owner/repo",
                branch: "main",
                created_at: 1_000_000,
            },
        )
        .unwrap();
        activate(conn, uuid, &format!("mux-{shortname}"), "/work/repo", "tcp", 1_000_001).unwrap();
    }

    fn make_ctx<'a, F>(conn: &'a Connection, make_exec: F, plain: bool) -> ListContext<'a, F>
    where
        F: Fn(&Host) -> MockExec,
    {
        ListContext { conn, make_exec, plain }
    }

    // Private function: maps all SessionStatusValue variants to their DB strings.
    #[test]
    fn agent_status_str_all_variants() {
        assert_eq!(agent_status_str(&SessionStatusValue::Active), "active");
        assert_eq!(agent_status_str(&SessionStatusValue::Dead), "dead");
        assert_eq!(agent_status_str(&SessionStatusValue::Unreachable), "unreachable");
        assert_eq!(agent_status_str(&SessionStatusValue::Orphaned), "orphaned");
    }

    // White-box: host with home=None (never probed) causes active sessions → unreachable.
    // Not coverable in tests/list.rs because insert_host always calls update_probe.
    #[tokio::test]
    async fn list_host_not_probed_marks_active_unreachable() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = host_repo::insert(conn, "rawhost", "u", "192.0.2.2", 22, 1_000_000).unwrap();
        let uuid = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        insert_active_session(conn, host_id, uuid, "rawapp");

        run_list(make_ctx(conn, |_| MockExec::unreachable(), false))
            .await
            .unwrap();

        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "unreachable");
    }

    // White-box: agent.lock returns empty string (agent not running) → active → unreachable.
    // The response pattern (exit 0, empty stdout) exercises the NoAgent branch in probe_existing.
    #[tokio::test]
    async fn list_no_agent_marks_active_sessions_unreachable() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let uuid = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        insert_active_session(conn, host_id, uuid, "myapp4");

        run_list(make_ctx(conn, |_| MockExec::new(vec![(0, String::new(), String::new())]), false))
            .await
            .unwrap();

        let s = session_repo::get_by_uuid(conn, uuid).unwrap().unwrap();
        assert_eq!(s.status, "unreachable");
    }
}
