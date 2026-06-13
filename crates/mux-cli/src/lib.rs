use anyhow::Result;
use clap::Subcommand;

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

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Init => todo!("mux init"),
        Command::Host { .. } => todo!("mux host"),
        Command::Agent { .. } => todo!("mux agent"),
        Command::Create => todo!("mux create"),
        Command::Attach => todo!("mux attach"),
        Command::List => todo!("mux list"),
        Command::Status => todo!("mux status"),
        Command::Kill => todo!("mux kill"),
        Command::Completions { .. } => todo!("mux completions"),
    }
}
