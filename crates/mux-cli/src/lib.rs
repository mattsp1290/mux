use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::generate;

pub mod agent_start;
pub mod host;
pub mod mux_home;

/// The full mux CLI definition, exported so `mux-cli` can generate completions.
#[derive(Debug, Parser)]
#[command(name = "mux", about = "tmux session manager", version)]
pub struct Cli {
    /// Override the mux state directory (default: $MUX_HOME or ~/.mux)
    #[arg(long, global = true)]
    pub mux_home: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize mux state directory
    Init,
    /// Manage remote hosts
    Host {
        #[command(subcommand)]
        action: HostAction,
    },
    /// Manage the remote mux-agent
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Create a new session
    Create,
    /// Attach to an existing session
    Attach,
    /// List sessions
    List,
    /// Show session status
    Status,
    /// Kill a session
    Kill,
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum HostAction {
    /// Add a host
    Add {
        /// Host alias (alphanumeric, hyphens, underscores; max 64 chars)
        alias: String,
        /// Remote user and address as user@addr
        user_at_addr: String,
        /// SSH port (1-65535, default 22)
        #[arg(long, short = 'p', default_value = "22")]
        port: u16,
    },
    /// List configured hosts
    List,
    /// Remove a host
    Remove {
        /// Host alias to remove
        alias: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Test connectivity and trust a host
    Test {
        alias: String,
    },
    /// Rotate or view trust for a host
    Trust {
        alias: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// Deploy the agent binary to a host
    Deploy,
    /// Stream agent logs
    Logs,
    /// Stop the agent on a host
    Stop,
}

pub async fn run(command: Command, mux_home: PathBuf) -> Result<()> {
    match command {
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, &name, &mut std::io::stdout());
            Ok(())
        }
        Command::Init => {
            let db_path = mux_home.join("mux.db");
            mux_state::store::Store::open(&db_path)?;
            Ok(())
        }
        Command::Host { action } => {
            let db_path = mux_home.join("mux.db");
            let store = mux_state::store::Store::open(&db_path)?;
            crate::host::run_host(action, store.conn()).await
        }
        Command::Agent { .. } => todo!("mux agent"),
        Command::Create => todo!("mux create"),
        Command::Attach => todo!("mux attach"),
        Command::List => todo!("mux list"),
        Command::Status => todo!("mux status"),
        Command::Kill => todo!("mux kill"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn init_creates_state_directory_and_db() {
        let tmp = TempDir::new().unwrap();
        let mux_home = tmp.path().join(".mux");
        run(Command::Init, mux_home.clone()).await.unwrap();
        assert!(mux_home.exists(), "mux_home should be created");
        assert!(mux_home.join("mux.db").exists(), "mux.db should be created");
    }

    #[tokio::test]
    async fn init_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let mux_home = tmp.path().join(".mux");
        run(Command::Init, mux_home.clone()).await.unwrap();
        run(Command::Init, mux_home.clone()).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn init_sets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let mux_home = tmp.path().join(".mux");
        run(Command::Init, mux_home.clone()).await.unwrap();
        let dir_mode = std::fs::metadata(&mux_home).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "mux_home should be 0700, got {dir_mode:o}");
        let db_mode = std::fs::metadata(mux_home.join("mux.db"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(db_mode, 0o600, "mux.db should be 0600, got {db_mode:o}");
    }

    #[tokio::test]
    async fn init_creates_no_config_file() {
        let tmp = TempDir::new().unwrap();
        let mux_home = tmp.path().join(".mux");
        run(Command::Init, mux_home.clone()).await.unwrap();
        let entries: Vec<_> = std::fs::read_dir(&mux_home)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                // Only mux.db and WAL sidecars should exist; no config files
                !s.starts_with("mux.db")
            })
            .collect();
        assert!(
            entries.is_empty(),
            "no config file should be created by init; found: {entries:?}"
        );
    }
}
