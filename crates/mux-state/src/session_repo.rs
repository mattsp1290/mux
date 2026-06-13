//! Spec: docs/03 §Sessions table, §Reservation semantics

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::Session;

/// Parameters for reserving a session slot before the tmux session is created.
pub struct ReserveParams<'a> {
    pub uuid: &'a str,
    pub host_id: i64,
    pub shortname: &'a str,
    pub repo_slug: &'a str,
    pub branch: &'a str,
    pub created_at: i64,
}

/// Parameters for importing a pre-existing tmux session.
pub struct ImportParams<'a> {
    pub uuid: &'a str,
    pub host_id: i64,
    pub shortname: &'a str,
    pub tmux_name: Option<&'a str>,
    pub repo_slug: &'a str,
    pub branch: &'a str,
    pub workdir: Option<&'a str>,
    pub transport_mode: Option<&'a str>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        uuid: row.get("uuid")?,
        host_id: row.get("host_id")?,
        shortname: row.get("shortname")?,
        tmux_name: row.get("tmux_name")?,
        repo_slug: row.get("repo_slug")?,
        branch: row.get("branch")?,
        workdir: row.get("workdir")?,
        transport_mode: row.get("transport_mode")?,
        status: row.get("status")?,
        imported: row.get::<_, i64>("imported")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

const SESSION_COLUMNS: &str = "id, uuid, host_id, shortname, tmux_name, repo_slug, branch, \
                                workdir, transport_mode, status, imported, created_at, updated_at";

/// Reserve a session slot (tmux_name stays NULL until activate).
///
/// Returns the new row id.
pub fn reserve(conn: &Connection, p: &ReserveParams<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO sessions \
         (uuid, host_id, shortname, tmux_name, repo_slug, branch, \
          workdir, transport_mode, status, imported, created_at, updated_at) \
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, NULL, NULL, 'active', 0, ?6, ?6)",
        params![
            p.uuid,
            p.host_id,
            p.shortname,
            p.repo_slug,
            p.branch,
            p.created_at
        ],
    )
    .context("reserve session")?;
    Ok(conn.last_insert_rowid())
}

/// Promote a reservation to a live session by filling in tmux_name, workdir,
/// transport_mode, and updated_at.  Only matches rows where tmux_name is still
/// NULL (guards against double-activation).
///
/// Returns `true` if a row was updated, `false` if the guard blocked (uuid not
/// found or already activated).  Callers performing create-flow rollback must
/// check this to detect a racing activation.
pub fn activate(
    conn: &Connection,
    uuid: &str,
    tmux_name: &str,
    workdir: &str,
    transport_mode: &str,
    updated_at: i64,
) -> Result<bool> {
    let rows = conn
        .execute(
            "UPDATE sessions \
             SET tmux_name = ?1, workdir = ?2, transport_mode = ?3, updated_at = ?4 \
             WHERE uuid = ?5 AND tmux_name IS NULL",
            params![tmux_name, workdir, transport_mode, updated_at, uuid],
        )
        .context("activate session")?;
    Ok(rows > 0)
}

/// Remove a reservation that was never activated (tmux_name still NULL).
///
/// Returns `true` if a row was deleted, `false` if no matching in-flight row
/// was found (already activated or uuid unknown).
pub fn cancel_reservation(conn: &Connection, uuid: &str) -> Result<bool> {
    let rows = conn
        .execute(
            "DELETE FROM sessions WHERE uuid = ?1 AND tmux_name IS NULL",
            params![uuid],
        )
        .context("cancel session reservation")?;
    Ok(rows > 0)
}

/// Fetch a session by its UUID.
pub fn get_by_uuid(conn: &Connection, uuid: &str) -> Result<Option<Session>> {
    conn.query_row(
        &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE uuid = ?1"),
        params![uuid],
        map_session,
    )
    .optional()
    .context("get session by uuid")
}

/// Fetch all activated sessions matching a shortname, across all hosts.
///
/// Used by selector resolution when the caller has no host context.
/// Returns multiple rows when the same shortname exists on different hosts —
/// callers must decide how to handle ambiguity.
pub fn get_by_shortname_global(conn: &Connection, shortname: &str) -> Result<Vec<Session>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE shortname = ?1 AND tmux_name IS NOT NULL \
             ORDER BY created_at ASC"
        ))
        .context("prepare get_by_shortname_global")?;
    let rows = stmt
        .query_map(params![shortname], map_session)
        .context("query get_by_shortname_global")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect get_by_shortname_global rows")?;
    Ok(rows)
}

/// Fetch an activated session by host + shortname (excludes in-flight reservations).
pub fn get_by_shortname(
    conn: &Connection,
    host_id: i64,
    shortname: &str,
) -> Result<Option<Session>> {
    conn.query_row(
        &format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE host_id = ?1 AND shortname = ?2 AND tmux_name IS NOT NULL"
        ),
        params![host_id, shortname],
        map_session,
    )
    .optional()
    .context("get session by shortname")
}

/// List activated sessions for a host, oldest first (docs/07 §List flow step 4).
pub fn list_for_host(conn: &Connection, host_id: i64) -> Result<Vec<Session>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions \
             WHERE host_id = ?1 AND tmux_name IS NOT NULL \
             ORDER BY created_at ASC"
        ))
        .context("prepare list sessions")?;
    let rows = stmt
        .query_map(params![host_id], map_session)
        .context("query list sessions")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect session rows")?;
    Ok(rows)
}

/// Update the status of a session.
pub fn set_status(conn: &Connection, uuid: &str, status: &str, updated_at: i64) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET status = ?1, updated_at = ?2 WHERE uuid = ?3",
        params![status, updated_at, uuid],
    )
    .context("set session status")?;
    Ok(())
}

/// Insert a pre-existing (imported) session with imported=1.
///
/// Returns the new row id.
pub fn import_session(conn: &Connection, p: &ImportParams<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO sessions \
         (uuid, host_id, shortname, tmux_name, repo_slug, branch, \
          workdir, transport_mode, status, imported, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', 1, ?9, ?10)",
        params![
            p.uuid,
            p.host_id,
            p.shortname,
            p.tmux_name,
            p.repo_slug,
            p.branch,
            p.workdir,
            p.transport_mode,
            p.created_at,
            p.updated_at
        ],
    )
    .context("import session")?;
    Ok(conn.last_insert_rowid())
}

/// Delete a session by UUID.
pub fn delete(conn: &Connection, uuid: &str) -> Result<()> {
    conn.execute("DELETE FROM sessions WHERE uuid = ?1", params![uuid])
        .context("delete session")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::TempDir;

    fn open_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store = Store::open(&db_path).unwrap();
        (dir, store)
    }

    fn insert_host(conn: &Connection) -> i64 {
        crate::host_repo::insert(conn, "test", "user", "127.0.0.1", 22, 1_000_000).unwrap()
    }

    fn make_reserve<'a>(uuid: &'a str, host_id: i64) -> ReserveParams<'a> {
        ReserveParams {
            uuid,
            host_id,
            shortname: "myapp",
            repo_slug: "owner/repo",
            branch: "main",
            created_at: 1_000_000,
        }
    }

    #[test]
    fn reserve_and_get_by_uuid() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let id = reserve(conn, &make_reserve("uuid-1", host_id)).unwrap();
        assert!(id > 0);
        let s = get_by_uuid(conn, "uuid-1").unwrap().expect("should exist");
        assert_eq!(s.uuid, "uuid-1");
        assert_eq!(s.host_id, host_id);
        assert!(s.tmux_name.is_none());
        assert!(!s.imported);
    }

    #[test]
    fn activate_sets_fields() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-2", host_id)).unwrap();
        let activated = activate(
            conn,
            "uuid-2",
            "mux-myapp",
            "/home/user/repo",
            "streamlocal",
            2_000_000,
        )
        .unwrap();
        assert!(activated, "first activation should succeed");
        let s = get_by_uuid(conn, "uuid-2").unwrap().expect("should exist");
        assert_eq!(s.tmux_name.as_deref(), Some("mux-myapp"));
        assert_eq!(s.workdir.as_deref(), Some("/home/user/repo"));
        assert_eq!(s.transport_mode.as_deref(), Some("streamlocal"));
        assert_eq!(s.updated_at, 2_000_000);
    }

    #[test]
    fn cancel_reservation_removes_row() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-3", host_id)).unwrap();
        let removed = cancel_reservation(conn, "uuid-3").unwrap();
        assert!(removed, "reservation should have been removed");
        let s = get_by_uuid(conn, "uuid-3").unwrap();
        assert!(s.is_none());
    }

    #[test]
    fn activate_does_not_touch_already_active() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-4", host_id)).unwrap();
        let first = activate(conn, "uuid-4", "mux-first", "/work", "tcp", 2_000_000).unwrap();
        assert!(first, "first activation should succeed");
        // Second activate: guard (tmux_name IS NULL) blocks — returns false, no panic.
        let second = activate(
            conn,
            "uuid-4",
            "mux-second",
            "/other",
            "streamlocal",
            3_000_000,
        )
        .unwrap();
        assert!(!second, "double-activation guard should block");
        let s = get_by_uuid(conn, "uuid-4").unwrap().unwrap();
        assert_eq!(s.tmux_name.as_deref(), Some("mux-first"));
    }

    #[test]
    fn get_by_shortname_excludes_in_flight() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-5", host_id)).unwrap();
        // tmux_name is NULL → should not appear via get_by_shortname
        let s = get_by_shortname(conn, host_id, "myapp").unwrap();
        assert!(s.is_none());
    }

    #[test]
    fn list_for_host_excludes_in_flight() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-6", host_id)).unwrap();
        let sessions = list_for_host(conn, host_id).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn set_status_updates_status() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-7", host_id)).unwrap();
        set_status(conn, "uuid-7", "stopped", 2_000_000).unwrap();
        let s = get_by_uuid(conn, "uuid-7").unwrap().unwrap();
        assert_eq!(s.status, "stopped");
        assert_eq!(s.updated_at, 2_000_000);
    }

    #[test]
    fn import_session_and_get() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        let p = ImportParams {
            uuid: "uuid-8",
            host_id,
            shortname: "imported-app",
            tmux_name: Some("mux-imported-app"),
            repo_slug: "owner/repo",
            branch: "feature",
            workdir: Some("/remote/path"),
            transport_mode: Some("tcp"),
            created_at: 1_000_000,
            updated_at: 1_000_000,
        };
        let id = import_session(conn, &p).unwrap();
        assert!(id > 0);
        let s = get_by_uuid(conn, "uuid-8").unwrap().expect("should exist");
        assert_eq!(s.shortname, "imported-app");
        assert!(s.imported);
        assert_eq!(s.tmux_name.as_deref(), Some("mux-imported-app"));
    }

    #[test]
    fn delete_session() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("uuid-9", host_id)).unwrap();
        delete(conn, "uuid-9").unwrap();
        let s = get_by_uuid(conn, "uuid-9").unwrap();
        assert!(s.is_none());
    }

    #[test]
    fn list_for_host_returns_multiple() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        // Reserve and activate two sessions.
        reserve(
            conn,
            &ReserveParams {
                uuid: "uuid-a",
                host_id,
                shortname: "app-a",
                repo_slug: "owner/repo",
                branch: "main",
                created_at: 1_000_000,
            },
        )
        .unwrap();
        activate(conn, "uuid-a", "mux-app-a", "/work/a", "tcp", 1_000_001).unwrap();

        reserve(
            conn,
            &ReserveParams {
                uuid: "uuid-b",
                host_id,
                shortname: "app-b",
                repo_slug: "owner/repo",
                branch: "dev",
                created_at: 1_000_002,
            },
        )
        .unwrap();
        activate(conn, "uuid-b", "mux-app-b", "/work/b", "tcp", 1_000_003).unwrap();

        let sessions = list_for_host(conn, host_id).unwrap();
        assert_eq!(sessions.len(), 2);
        // Oldest first (created_at ASC per docs/07 §List flow step 4)
        assert_eq!(sessions[0].uuid, "uuid-a");
        assert_eq!(sessions[1].uuid, "uuid-b");
    }

    #[test]
    fn reserve_duplicate_uuid_fails() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        reserve(conn, &make_reserve("dup-uuid", host_id)).unwrap();
        let result = reserve(conn, &make_reserve("dup-uuid", host_id));
        assert!(
            result.is_err(),
            "duplicate uuid should be rejected by UNIQUE constraint"
        );
    }

    #[test]
    fn cascade_delete_host_removes_sessions() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id =
            crate::host_repo::insert(conn, "cascade-host", "u", "1.2.3.4", 22, 1_000_000).unwrap();
        reserve(conn, &make_reserve("cascade-uuid", host_id)).unwrap();
        crate::host_repo::delete(conn, host_id).unwrap();
        let s = get_by_uuid(conn, "cascade-uuid").unwrap();
        assert!(
            s.is_none(),
            "session should be removed by FK ON DELETE CASCADE"
        );
    }
}
