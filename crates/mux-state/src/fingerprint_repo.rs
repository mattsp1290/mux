//! Spec: docs/03 §Known host fingerprints table

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::KnownHostFingerprint;

fn map_fingerprint(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnownHostFingerprint> {
    Ok(KnownHostFingerprint {
        id: row.get("id")?,
        host_id: row.get("host_id")?,
        algorithm: row.get("algorithm")?,
        fingerprint: row.get("fingerprint")?,
        trusted_at: row.get("trusted_at")?,
    })
}

/// Upsert a fingerprint record (insert or replace on host_id + algorithm).
pub fn upsert(
    conn: &Connection,
    host_id: i64,
    algorithm: &str,
    fingerprint: &str,
    trusted_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO known_host_fingerprints \
         (host_id, algorithm, fingerprint, trusted_at) VALUES (?1, ?2, ?3, ?4)",
        params![host_id, algorithm, fingerprint, trusted_at],
    )
    .context("upsert known host fingerprint")?;
    Ok(())
}

/// Fetch a fingerprint by host and algorithm.
pub fn get(
    conn: &Connection,
    host_id: i64,
    algorithm: &str,
) -> Result<Option<KnownHostFingerprint>> {
    conn.query_row(
        "SELECT id, host_id, algorithm, fingerprint, trusted_at \
         FROM known_host_fingerprints WHERE host_id = ?1 AND algorithm = ?2",
        params![host_id, algorithm],
        map_fingerprint,
    )
    .optional()
    .context("get known host fingerprint")
}

/// List all fingerprints for a host.
pub fn list_for_host(conn: &Connection, host_id: i64) -> Result<Vec<KnownHostFingerprint>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, host_id, algorithm, fingerprint, trusted_at \
             FROM known_host_fingerprints WHERE host_id = ?1",
        )
        .context("prepare list fingerprints")?;
    let rows = stmt
        .query_map(params![host_id], map_fingerprint)
        .context("query list fingerprints")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect fingerprint rows")?;
    Ok(rows)
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
        upsert(conn, host_id, "ssh-ed25519", "AAAA1234", 1_000_000).unwrap();
        let fp = get(conn, host_id, "ssh-ed25519")
            .unwrap()
            .expect("should exist");
        assert_eq!(fp.host_id, host_id);
        assert_eq!(fp.algorithm, "ssh-ed25519");
        assert_eq!(fp.fingerprint, "AAAA1234");
        assert_eq!(fp.trusted_at, 1_000_000);
    }

    #[test]
    fn upsert_updates_existing() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host_id = insert_host(conn);
        upsert(conn, host_id, "ssh-ed25519", "AAAA1234", 1_000_000).unwrap();
        upsert(conn, host_id, "ssh-ed25519", "BBBB5678", 2_000_000).unwrap();
        let fp = get(conn, host_id, "ssh-ed25519")
            .unwrap()
            .expect("should exist");
        assert_eq!(fp.fingerprint, "BBBB5678");
        assert_eq!(fp.trusted_at, 2_000_000);
    }

    #[test]
    fn list_for_host_returns_only_that_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let host1 = crate::host_repo::insert(conn, "host1", "u", "1.1.1.1", 22, 1).unwrap();
        let host2 = crate::host_repo::insert(conn, "host2", "u", "2.2.2.2", 22, 2).unwrap();
        upsert(conn, host1, "ssh-ed25519", "FP_HOST1", 1_000_000).unwrap();
        upsert(conn, host2, "ssh-ed25519", "FP_HOST2", 1_000_000).unwrap();
        let fps = list_for_host(conn, host1).unwrap();
        assert_eq!(fps.len(), 1);
        assert_eq!(fps[0].fingerprint, "FP_HOST1");
    }
}
