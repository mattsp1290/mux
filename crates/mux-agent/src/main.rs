use anyhow::{Context, Result};
use mux_rpc::server::RpcServer;
use mux_tmux::adapter::TmuxAdapter;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mux_home = resolve_mux_home()?;
    let bind_host = std::env::var("MUX_AGENT_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("MUX_AGENT_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let bind_addr = format!("{bind_host}:{port}");

    let tmux = TmuxAdapter::new();
    let server = RpcServer::new_with_tmux(bind_addr, tmux);
    let bound = server.bind().await.context("failed to bind RPC server")?;

    let local_addr = bound.local_addr();
    tracing::info!("mux-agent listening on {local_addr}");

    write_lock_file(&mux_home, local_addr)?;

    let result = bound.serve().await;

    // Clean up lock file on exit
    let lock_path = mux_home.join("agent.lock");
    let _ = std::fs::remove_file(&lock_path);

    result
}

fn resolve_mux_home() -> Result<PathBuf> {
    if let Ok(val) = std::env::var("MUX_HOME") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
    let home = std::env::var("HOME").context("HOME not set and MUX_HOME not set")?;
    Ok(PathBuf::from(home).join(".mux"))
}

fn write_lock_file(mux_home: &std::path::Path, addr: std::net::SocketAddr) -> Result<()> {
    std::fs::create_dir_all(mux_home).context("failed to create mux_home directory")?;
    let pid = std::process::id();
    let tcp_url = format!("tcp://{addr}");
    let content = serde_json::json!({ "pid": pid, "tcp_url": tcp_url });
    let json = serde_json::to_vec(&content)?;
    let tmp_path = mux_home.join("agent.lock.tmp");
    let lock_path = mux_home.join("agent.lock");
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, &lock_path)?;
    Ok(())
}
