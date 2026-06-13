use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::generate;

pub mod agent;
pub mod agent_start;
pub mod attach;
pub mod create;
pub mod host;
pub mod kill;
pub mod list;
pub mod mux_home;
pub mod status;

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
    Create {
        /// Repository (owner/repo or git@host:path.git)
        repo: String,
        /// Remote host alias
        #[arg(long)]
        host: String,
        /// Branch to check out (defaults to main)
        #[arg(long)]
        branch: Option<String>,
    },
    /// Attach to an existing session
    Attach {
        /// UUID or shortname of the session to attach
        selector: String,
    },
    /// List sessions
    List {
        /// Tab-separated output without ANSI (machine-readable)
        #[arg(long)]
        plain: bool,
    },
    /// Show session status
    Status {
        /// UUID or shortname of the session
        selector: String,
    },
    /// Kill a session
    Kill {
        /// UUID or shortname of the session to kill
        selector: String,
    },
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
    Deploy {
        /// Host alias to deploy to
        alias: String,
    },
    /// Stream agent logs
    Logs {
        /// Host alias
        alias: String,
        /// Follow log output (tail -f semantics)
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// Stop the agent on a host
    Stop {
        /// Host alias
        alias: String,
    },
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
        Command::Agent { action } => {
            let db_path = mux_home.join("mux.db");
            let store = mux_state::store::Store::open(&db_path)?;
            match action {
                AgentAction::Deploy { alias } => {
                    // Real SSH execution not yet wired (no production SSH executor).
                    // The core logic (run_agent_deploy) is tested in isolation via
                    // MockDeployHost; wire up once a real SshHost impl lands.
                    let _ = alias;
                    anyhow::bail!("mux agent deploy: SSH execution not yet implemented")
                }
                AgentAction::Logs { alias, follow } => {
                    let _ = (store, alias, follow);
                    anyhow::bail!("mux agent logs: SSH execution not yet implemented")
                }
                AgentAction::Stop { alias } => {
                    let _ = (store, alias);
                    anyhow::bail!("mux agent stop: SSH execution not yet implemented")
                }
            }
        }
        // TODO: wire to run_create once a real SshHost SSH impl lands (currently
        // no production SSH executor exists; the create module is tested in isolation).
        Command::Create { .. } => anyhow::bail!("mux create: SSH execution not yet implemented"),
        // TODO: wire to prepare_attach + exec once a real SshHost SSH impl lands.
        Command::Attach { .. } => anyhow::bail!("mux attach: SSH execution not yet implemented"),
        // TODO: wire to run_list once a real RemoteExec SSH impl lands.
        Command::List { .. } => anyhow::bail!("mux list: SSH execution not yet implemented"),
        // TODO: wire to run_status once a real RemoteExec SSH impl lands.
        Command::Status { .. } => anyhow::bail!("mux status: SSH execution not yet implemented"),
        // TODO: wire to run_kill once a real SshHost SSH impl lands (currently no
        // production SSH executor exists; kill is tested via MockSshHost in isolation).
        Command::Kill { .. } => anyhow::bail!("mux kill: SSH execution not yet implemented"),
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
