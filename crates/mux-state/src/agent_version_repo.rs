//! Spec: docs/03 §Agent versions table

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::AgentVersion;

fn map_agent_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentVersion> {
    Ok(AgentVersion {
        id: row.get("id")?,
        host_id: row.get("host_id")?,
        version: row.get("version")?,
        deployed_at: row.get("deployed_at")?,
    })
}

/// Upsert an agent version record (one record per host — UNIQUE(host_id)).
///
/// Uses `ON CONFLICT DO UPDATE` to keep the row id stable.
pub fn upsert(conn: &Connection, host_id: i64, version: &str, deployed_at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO agent_versions (host_id, version, deployed_at) \
         VALUES (?1, ?2, ?3) \
         ON CONFLICT(host_id) DO UPDATE SET \
             version     = excluded.version, \
             deployed_at = excluded.deployed_at",
        params![host_id, version, deployed_at],
    )
    .context("upsert agent version")?;
    Ok(())
}

/// Fetch the agent version for a host, if recorded.
pub fn get_for_host(conn: &Connection, host_id: i64) -> Result<Option<AgentVersion>> {
    conn.query_row(
        "SELECT id, host_id, version, deployed_at \
         FROM agent_versions WHERE host_id = ?1",
        params![host_id],
        map_agent_version,
    )
    .optional()
    .context("get agent version for host")
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

    #[test]
    fn upsert_and_get() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        upsert(conn, host_id, "0.1.0", 1_000_000).unwrap();
        let av = get_for_host(conn, host_id).unwrap().expect("should exist");
        assert_eq!(av.host_id, host_id);
        assert_eq!(av.version, "0.1.0");
        assert_eq!(av.deployed_at, 1_000_000);
    }

    #[test]
    fn upsert_updates_existing() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        upsert(conn, host_id, "0.1.0", 1_000_000).unwrap();
        upsert(conn, host_id, "0.2.0", 2_000_000).unwrap();
        let av = get_for_host(conn, host_id).unwrap().expect("should exist");
        assert_eq!(av.version, "0.2.0");
        assert_eq!(av.deployed_at, 2_000_000);
    }
}
