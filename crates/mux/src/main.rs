use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "mux", about = "tmux session manager", version)]
struct Cli {
    #[command(subcommand)]
    command: mux_cli::Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    mux_cli::run(cli.command).await
}
