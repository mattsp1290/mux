use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = mux_cli::Cli::parse();

    // Completions do not need a state directory — handle before resolve_mux_home so
    // that `mux completions bash` works in environments without HOME.
    if let mux_cli::Command::Completions { .. } = &cli.command {
        match mux_cli::run(cli.command, std::path::PathBuf::new()).await {
            Ok(()) => return,
            Err(e) => {
                eprintln!("mux: {e}");
                std::process::exit(1);
            }
        }
    }

    let mux_home = match mux_cli::mux_home::resolve_mux_home(cli.mux_home) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mux: {e}");
            std::process::exit(e.exit_code());
        }
    };

    match mux_cli::run(cli.command, mux_home).await {
        Ok(()) => {}
        Err(e) => {
            let (exit_code, msg) =
                if let Some(mux_err) = e.downcast_ref::<mux_core::error::MuxError>() {
                    (mux_err.exit_code(), format!("mux: {mux_err}"))
                } else {
                    (1i32, format!("mux: {e}"))
                };
            eprintln!("{msg}");
            std::process::exit(exit_code);
        }
    }
}
