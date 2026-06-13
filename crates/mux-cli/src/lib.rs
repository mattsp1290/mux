use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::generate;

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
    Add,
    /// List hosts
    List,
    /// Remove a host
    Remove,
    /// Test connectivity and trust a host
    Test,
    /// Rotate or view trust for a host
    Trust,
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

pub async fn run(command: Command, _mux_home: PathBuf) -> Result<()> {
    match command {
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, &name, &mut std::io::stdout());
            Ok(())
        }
        Command::Init => todo!("mux init"),
        Command::Host { .. } => todo!("mux host"),
        Command::Agent { .. } => todo!("mux agent"),
        Command::Create => todo!("mux create"),
        Command::Attach => todo!("mux attach"),
        Command::List => todo!("mux list"),
        Command::Status => todo!("mux status"),
        Command::Kill => todo!("mux kill"),
    }
}
