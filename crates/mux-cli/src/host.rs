use anyhow::{bail, Result};
use rusqlite::Connection;
use std::str::FromStr;

use mux_core::types::{HostAlias, Port};
use mux_state::host_repo;

use crate::HostAction;

pub async fn run_host(action: HostAction, conn: &Connection) -> Result<()> {
    match action {
        HostAction::Add { alias, user_at_addr, port } => cmd_add(conn, alias, user_at_addr, port),
        HostAction::List => cmd_list(conn),
        HostAction::Remove { alias, yes } => cmd_remove(conn, alias, yes).await,
        HostAction::Test { .. } => todo!("mux host test"),
        HostAction::Trust { .. } => todo!("mux host trust"),
    }
}

fn cmd_add(conn: &Connection, alias: String, user_at_addr: String, port: u16) -> Result<()> {
    // Validate alias
    let alias = HostAlias::from_str(&alias)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Split user@addr on first @
    let at = user_at_addr.find('@')
        .ok_or_else(|| anyhow::anyhow!("expected user@addr, got: {:?}", user_at_addr))?;
    let user = &user_at_addr[..at];
    let addr = &user_at_addr[at + 1..];

    if user.is_empty() { bail!("user part of user@addr must not be empty"); }
    if addr.is_empty() { bail!("addr part of user@addr must not be empty"); }

    // Tilde expansion: if addr starts with ~, expand to $HOME/<rest>
    let addr = if addr.starts_with('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        let rest = &addr[1..];
        if rest.is_empty() { home } else { format!("{home}{rest}") }
    } else {
        addr.to_owned()
    };

    // Validate port
    let port = Port::from_str(&port.to_string())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    match host_repo::insert(conn, alias.as_str(), user, &addr, port.value() as i64, created_at) {
        Ok(_) => Ok(()),
        Err(e) => {
            // Check full error chain (anyhow wraps the SQLite error with context)
            let msg = format!("{e:#}").to_lowercase();
            if msg.contains("unique constraint failed") || msg.contains("constraint failed") {
                bail!("host '{}' already exists", alias.as_str())
            }
            Err(e)
        }
    }
}

fn cmd_list(conn: &Connection) -> Result<()> {
    let hosts = host_repo::list(conn)?;

    if hosts.is_empty() {
        println!("No hosts configured. Use 'mux host add' to add one.");
        return Ok(());
    }

    let alias_w = hosts.iter().map(|h| h.alias.len()).max().unwrap_or(5).max(5);
    let user_addr_w = hosts.iter().map(|h| format!("{}@{}", h.user, h.addr).len()).max().unwrap_or(12).max("USER@ADDR".len());
    let port_w = "PORT".len();
    let arch_w = "ARCH".len().max(7);

    println!(
        "{:<alias_w$}  {:<user_addr_w$}  {:<port_w$}  {:<arch_w$}  {}",
        "ALIAS", "USER@ADDR", "PORT", "ARCH", "HOME"
    );

    for host in &hosts {
        let user_addr = format!("{}@{}", host.user, host.addr);
        let arch = host.arch.as_deref().unwrap_or("-");
        let home = host.home.as_deref().unwrap_or("-");
        println!(
            "{:<alias_w$}  {:<user_addr_w$}  {:<port_w$}  {:<arch_w$}  {}",
            host.alias, user_addr, host.port, arch, home
        );
    }

    Ok(())
}

async fn cmd_remove(conn: &Connection, alias: String, yes: bool) -> Result<()> {
    let alias = HostAlias::from_str(&alias)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let host = host_repo::get_by_alias(conn, alias.as_str())?
        .ok_or_else(|| anyhow::anyhow!("host '{}' not found", alias.as_str()))?;

    if !yes {
        eprint!(
            "Remove host '{}' ({}@{}:{})? [y/N] ",
            host.alias, host.user, host.addr, host.port
        );
        use std::io::BufRead;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let response = line.trim().to_lowercase();
        if response != "y" {
            println!("Aborted.");
            return Ok(());
        }
    }

    host_repo::delete(conn, host.id)?;
    println!("Removed host '{}'.", host.alias);
    Ok(())
}

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

    #[test]
    fn add_success_inserts_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "myhost".to_owned(), "user@10.0.0.1".to_owned(), 22).unwrap();
        let host = host_repo::get_by_alias(conn, "myhost").unwrap().expect("should exist");
        assert_eq!(host.alias, "myhost");
        assert_eq!(host.user, "user");
        assert_eq!(host.addr, "10.0.0.1");
        assert_eq!(host.port, 22);
    }

    #[test]
    fn add_duplicate_alias_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "myhost".to_owned(), "user@10.0.0.1".to_owned(), 22).unwrap();
        let err = cmd_add(conn, "myhost".to_owned(), "user2@10.0.0.2".to_owned(), 22)
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "got: {err}");
    }

    #[test]
    fn add_invalid_alias_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_add(conn, "has.dot".to_owned(), "user@10.0.0.1".to_owned(), 22)
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn add_missing_at_errors() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_add(conn, "myhost".to_owned(), "noatsign".to_owned(), 22)
            .unwrap_err();
        assert!(err.to_string().contains("user@addr"), "got: {err}");
    }

    #[test]
    fn add_tilde_expansion() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        // Set HOME to a known value for the test
        std::env::set_var("HOME", "/home/testuser");
        cmd_add(conn, "tildehost".to_owned(), "user@~/workspace".to_owned(), 22).unwrap();
        let host = host_repo::get_by_alias(conn, "tildehost").unwrap().expect("should exist");
        assert_eq!(host.addr, "/home/testuser/workspace");
    }

    #[test]
    fn list_empty_prints_message() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        // Should not panic
        cmd_list(conn).unwrap();
    }

    #[test]
    fn list_one_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "prod".to_owned(), "alice@prod.example".to_owned(), 2222).unwrap();
        // Should not panic and should list the host
        cmd_list(conn).unwrap();
    }

    #[tokio::test]
    async fn remove_yes_removes_host() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "toremove".to_owned(), "user@10.0.0.5".to_owned(), 22).unwrap();
        cmd_remove(conn, "toremove".to_owned(), true).await.unwrap();
        let result = host_repo::get_by_alias(conn, "toremove").unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn remove_yes_host_not_found() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        let err = cmd_remove(conn, "nosuchhost".to_owned(), true).await.unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn remove_yes_cascade() {
        let (_dir, store) = open_store();
        let conn = store.conn();
        cmd_add(conn, "cascadehost".to_owned(), "user@10.0.0.6".to_owned(), 22).unwrap();
        let host = host_repo::get_by_alias(conn, "cascadehost").unwrap().unwrap();
        // Insert a fingerprint for this host
        conn.execute(
            "INSERT INTO known_host_fingerprints (host_id, algorithm, fingerprint, trusted_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![host.id, "ed25519", "AAAA1234", 1_000_000i64],
        ).unwrap();
        // Remove the host
        cmd_remove(conn, "cascadehost".to_owned(), true).await.unwrap();
        // Fingerprint should be gone
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM known_host_fingerprints WHERE host_id = ?1",
            rusqlite::params![host.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "fingerprints should be cascade-deleted");
    }
}
