//! SQLite store for mux local state.
//!
//! Spec: docs/03 §Storage location, §SQLite connection settings

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::migrations;

/// Open database handle for the mux local SQLite state.
///
/// Holds a single `rusqlite::Connection`.  Repository implementations
/// (mux-cz6) receive a `&Store` and call [`Store::conn`] to run queries.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) the mux database at `path`.
    ///
    /// Steps performed:
    /// 1. Create the parent directory (and any ancestors) with mode 0700 (Unix).
    /// 2. Open or create `path` with SQLite.
    /// 3. Set the database file to mode 0600 (Unix).
    /// 4. Apply required PRAGMAs on the connection.
    /// 5. Run any pending migrations via [`migrations::run`].
    pub fn open(path: &Path) -> Result<Self> {
        // 1. Create parent directory.
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create state directory {dir:?}"))?;
                #[cfg(unix)]
                set_dir_mode(dir)?;
            }
        }

        // 2. Open the SQLite connection (creates the file if absent).
        let conn = Connection::open(path).with_context(|| format!("open database {path:?}"))?;

        // 3. Restrict file permissions on the DB file itself.
        // NOTE: WAL mode (step 4) also creates `-wal` and `-shm` sidecar files
        // which are not individually chmod'd here — they inherit the process
        // umask.  The 0700 directory mode (step 1) is the real access-control
        // boundary for the whole store.  The 0600 on `mux.db` is defense-in-depth
        // for the case where the directory is traversable (e.g. a shared /tmp).
        #[cfg(unix)]
        set_file_mode(path)?;

        // 4. Apply required PRAGMAs on every connection.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
        .context("apply connection PRAGMAs")?;

        // 5. Run any pending migrations.
        migrations::run(&conn).context("run migrations")?;

        Ok(Store { conn })
    }

    /// Return a reference to the underlying connection.
    ///
    /// Repository implementations use this to execute queries.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(unix)]
fn set_dir_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 0700 {path:?}"))
}

#[cfg(unix)]
fn set_file_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_temp_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store = Store::open(&db_path).expect("Store::open should succeed");
        (dir, store)
    }

    #[test]
    fn open_creates_database_file() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        assert!(!db_path.exists());
        Store::open(&db_path).unwrap();
        assert!(db_path.exists(), "database file should be created");
    }

    #[test]
    fn open_creates_nested_parent_directories() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("nested").join("subdir").join("mux.db");
        Store::open(&db_path).unwrap();
        assert!(
            db_path.exists(),
            "database file should be created in nested dirs"
        );
    }

    #[test]
    fn open_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        Store::open(&db_path).unwrap();
        Store::open(&db_path).unwrap(); // second open should succeed
    }

    #[cfg(unix)]
    #[test]
    fn database_file_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, _store) = open_temp_store();
        // Re-derive path since TempDir is moved
        let dir2 = TempDir::new().unwrap();
        let db_path = dir2.path().join("mux.db");
        Store::open(&db_path).unwrap();
        let meta = std::fs::metadata(&db_path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "database file should have mode 0600, got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_directory_has_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let base = TempDir::new().unwrap();
        let state_dir = base.path().join("state");
        let db_path = state_dir.join("mux.db");
        Store::open(&db_path).unwrap();
        let meta = std::fs::metadata(&state_dir).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "state dir should have mode 0700, got {mode:o}");
    }

    #[test]
    fn all_tables_accessible_through_conn() {
        let (_dir, store) = open_temp_store();
        let conn = store.conn();

        let tables = [
            "hosts",
            "sessions",
            "known_host_fingerprints",
            "agent_versions",
        ];
        for table in tables {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap_or(-1);
            assert!(n >= 0, "table {table:?} should be queryable");
        }
    }

    #[test]
    fn foreign_keys_on_after_open() {
        let (_dir, store) = open_temp_store();
        let fk_enabled: i64 = store
            .conn()
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk_enabled, 1, "foreign_keys PRAGMA should be ON");
    }

    #[test]
    fn wal_mode_after_open() {
        let (_dir, store) = open_temp_store();
        let mode: String = store
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "journal_mode should be WAL");
    }

    // ── concurrent open ───────────────────────────────────────────────────────

    #[test]
    fn two_connections_to_same_db_both_succeed() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store1 = Store::open(&db_path).unwrap();
        let store2 = Store::open(&db_path).unwrap();
        // WAL mode allows concurrent readers; both connections can query.
        let n1: i64 = store1
            .conn()
            .query_row("SELECT COUNT(*) FROM hosts", [], |r| r.get(0))
            .unwrap();
        let n2: i64 = store2
            .conn()
            .query_row("SELECT COUNT(*) FROM hosts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n1, 0);
        assert_eq!(n2, 0);
    }

    #[test]
    fn write_on_first_connection_visible_on_second() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mux.db");
        let store1 = Store::open(&db_path).unwrap();
        let store2 = Store::open(&db_path).unwrap();
        crate::host_repo::insert(store1.conn(), "concurrent-host", "u", "1.2.3.4", 22, 1_000_000)
            .unwrap();
        // Committed writes by store1 are visible to store2's next read transaction.
        let n: i64 = store2
            .conn()
            .query_row("SELECT COUNT(*) FROM hosts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "write on store1 should be visible via store2");
    }
}
