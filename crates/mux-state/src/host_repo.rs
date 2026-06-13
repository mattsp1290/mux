//! Spec: docs/03 §Hosts table

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::Host;

fn map_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<Host> {
    Ok(Host {
        id: row.get("id")?,
        alias: row.get("alias")?,
        user: row.get("user")?,
        addr: row.get("addr")?,
        port: row.get("port")?,
        arch: row.get("arch")?,
        home: row.get("home")?,
        transport: row.get("transport")?,
        created_at: row.get("created_at")?,
    })
}

/// Insert a host and return the new row id.
pub fn insert(
    conn: &Connection,
    alias: &str,
    user: &str,
    addr: &str,
    port: i64,
    created_at: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO hosts (alias, user, addr, port, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![alias, user, addr, port, created_at],
    )
    .context("insert host")?;
    Ok(conn.last_insert_rowid())
}

/// Fetch a host by its primary key.
pub fn get_by_id(conn: &Connection, id: i64) -> Result<Option<Host>> {
    conn.query_row(
        "SELECT id, alias, user, addr, port, arch, home, transport, created_at \
         FROM hosts WHERE id = ?1",
        params![id],
        map_host,
    )
    .optional()
    .context("get host by id")
}

/// Fetch a host by its unique alias.
pub fn get_by_alias(conn: &Connection, alias: &str) -> Result<Option<Host>> {
    conn.query_row(
        "SELECT id, alias, user, addr, port, arch, home, transport, created_at \
         FROM hosts WHERE alias = ?1",
        params![alias],
        map_host,
    )
    .optional()
    .context("get host by alias")
}

/// List all hosts ordered by alias ascending.
pub fn list(conn: &Connection) -> Result<Vec<Host>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, alias, user, addr, port, arch, home, transport, created_at \
             FROM hosts ORDER BY alias ASC",
        )
        .context("prepare list hosts")?;
    let rows = stmt
        .query_map([], map_host)
        .context("query list hosts")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect host rows")?;
    Ok(rows)
}

/// Update the probe-discovered fields for a host.
pub fn update_probe(
    conn: &Connection,
    id: i64,
    arch: Option<&str>,
    home: Option<&str>,
    transport: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE hosts SET arch = ?1, home = ?2, transport = ?3 WHERE id = ?4",
        params![arch, home, transport, id],
    )
    .context("update host probe fields")?;
    Ok(())
}

/// Delete a host by id. FK cascade removes dependents.
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM hosts WHERE id = ?1", params![id])
        .context("delete host")?;
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

    #[test]
    fn insert_and_get_by_alias() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let id = insert(conn, "myhost", "user1", "10.0.0.1", 22, 1_000_000).unwrap();
        assert!(id > 0);
        let host = get_by_alias(conn, "myhost").unwrap().expect("should exist");
        assert_eq!(host.alias, "myhost");
        assert_eq!(host.user, "user1");
        assert_eq!(host.addr, "10.0.0.1");
        assert_eq!(host.port, 22);
    }

    #[test]
    fn insert_and_get_by_id() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let id = insert(conn, "myhost2", "user2", "10.0.0.2", 2222, 1_000_001).unwrap();
        let host = get_by_id(conn, id).unwrap().expect("should exist");
        assert_eq!(host.id, id);
        assert_eq!(host.alias, "myhost2");
        assert_eq!(host.port, 2222);
    }

    #[test]
    fn list_sorted_by_alias() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        insert(conn, "charlie", "u", "1.1.1.3", 22, 3).unwrap();
        insert(conn, "alice", "u", "1.1.1.1", 22, 1).unwrap();
        insert(conn, "bob", "u", "1.1.1.2", 22, 2).unwrap();
        let hosts = list(conn).unwrap();
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["alice", "bob", "charlie"]);
    }

    #[test]
    fn newly_inserted_host_has_null_probe_fields() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let id = insert(conn, "freshhost", "u", "1.2.3.4", 22, 1_000_000).unwrap();
        let host = get_by_id(conn, id).unwrap().unwrap();
        assert!(host.arch.is_none(), "arch should be NULL on a freshly inserted host");
        assert!(host.home.is_none(), "home should be NULL on a freshly inserted host");
        assert!(host.transport.is_none(), "transport should be NULL on a freshly inserted host");
    }

    #[test]
    fn update_probe_fields() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let id = insert(conn, "probehost", "u", "1.2.3.4", 22, 1_000_000).unwrap();
        let before = get_by_id(conn, id).unwrap().unwrap();
        assert!(before.arch.is_none());

        update_probe(
            conn,
            id,
            Some("aarch64"),
            Some("/home/user"),
            Some("streamlocal"),
        )
        .unwrap();
        let after = get_by_id(conn, id).unwrap().unwrap();
        assert_eq!(after.arch.as_deref(), Some("aarch64"));
        assert_eq!(after.home.as_deref(), Some("/home/user"));
        assert_eq!(after.transport.as_deref(), Some("streamlocal"));
    }

    #[test]
    fn delete_returns_ok() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let id = insert(conn, "delhost", "u", "9.9.9.9", 22, 1_000_000).unwrap();
        delete(conn, id).unwrap();
        let result = get_by_id(conn, id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let result = get_by_alias(conn, "no-such-host").unwrap();
        assert!(result.is_none());
    }
}
