use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "mux", about = "tmux session manager", version)]
struct Cli {
    #[command(subcommand)]
    command: mux_cli::Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    mux_cli::run(cli.command).await
}
