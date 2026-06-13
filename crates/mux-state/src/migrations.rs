//! SQLite migration runner for the mux state database.
//!
//! Spec: docs/03 §Migrations
//!
//! Rules:
//! - Forward-only; rollbacks are not supported.
//! - Idempotent: `CREATE TABLE IF NOT EXISTS` and `INSERT OR IGNORE` throughout.
//! - Version tracked in `_migrations (id, applied_at)`.
//! - Each migration SQL runs within its own `BEGIN`/`COMMIT` (included in the file).

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Ordered list of (migration_id, SQL).
///
/// Migration files are embedded at compile time.  Add new entries here as
/// additional `migrations/NNN-*.sql` files are created.
static MIGRATIONS: &[(u32, &str)] = &[(
    1,
    include_str!("../../../migrations/001-initial-schema.sql"),
)];

/// Apply any pending migrations to `conn`.
///
/// Ensures `_migrations` exists before querying it, then runs each migration
/// whose `id` is not yet recorded.  Safe to call on every `Store::open` — all
/// already-applied migrations are skipped in O(1) per migration.
pub fn run(conn: &Connection) -> Result<()> {
    // Pre-create _migrations so the SELECT check works on a brand-new database
    // before any migration has run.  The migrations themselves also create it
    // with IF NOT EXISTS, so this is idempotent.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
             id         INTEGER PRIMARY KEY,
             applied_at INTEGER NOT NULL
         );",
    )
    .context("ensure _migrations table exists")?;

    for &(id, sql) in MIGRATIONS {
        let already_applied: bool = conn
            .query_row(
                "SELECT 1 FROM _migrations WHERE id = ?1",
                rusqlite::params![id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !already_applied {
            conn.execute_batch(sql)
                .with_context(|| format!("apply migration {id}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        conn
    }

    #[test]
    fn migrations_run_cleanly_on_fresh_db() {
        let conn = open_in_memory();
        run(&conn).expect("migrations should succeed on fresh DB");
    }

    #[test]
    fn migrations_idempotent_when_run_twice() {
        let conn = open_in_memory();
        run(&conn).unwrap();
        run(&conn).expect("second run should be a no-op");
    }

    #[test]
    fn all_expected_tables_exist_after_migration() {
        let conn = open_in_memory();
        run(&conn).unwrap();

        let expected = [
            "_migrations",
            "hosts",
            "known_host_fingerprints",
            "agent_versions",
            "sessions",
        ];
        for table in expected {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            assert_eq!(count, 1, "table {table:?} should exist after migration");
        }
    }

    #[test]
    fn migration_1_recorded_in_migrations_table() {
        let conn = open_in_memory();
        run(&conn).unwrap();

        let applied: bool = conn
            .query_row("SELECT 1 FROM _migrations WHERE id = 1", [], |_| Ok(true))
            .unwrap_or(false);
        assert!(applied, "migration 1 should be recorded in _migrations");
    }

    #[test]
    fn sessions_table_has_expected_columns() {
        let conn = open_in_memory();
        run(&conn).unwrap();

        // Check that PRAGMA table_info returns rows for key columns
        let col_names: Vec<String> = conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let required = [
            "id",
            "uuid",
            "host_id",
            "shortname",
            "tmux_name",
            "repo_slug",
            "branch",
            "workdir",
            "transport_mode",
            "status",
            "imported",
            "created_at",
            "updated_at",
        ];
        for col in required {
            assert!(
                col_names.iter().any(|c| c == col),
                "sessions column {col:?} missing; found: {col_names:?}"
            );
        }
    }

    #[test]
    fn foreign_keys_enforced_after_migration() {
        let conn = open_in_memory();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        run(&conn).unwrap();

        // Attempting to insert a session with a non-existent host_id should fail
        let result = conn.execute(
            "INSERT INTO sessions (uuid, host_id, shortname, repo_slug, branch, status, imported, created_at, updated_at)
             VALUES ('test-uuid', 9999, 'test', 'org/repo', 'main', 'active', 0, 0, 0)",
            [],
        );
        assert!(result.is_err(), "FK violation should be rejected");
    }
}
