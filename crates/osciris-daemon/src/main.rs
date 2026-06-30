use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use osciris_daemon::{default_state_dir, DaemonService, LocalEndpoint};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "osciris-daemon",
    version,
    about = "Per-user OSCIRIS node desktop daemon"
)]
struct Args {
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("osciris_daemon=info")),
        )
        .init();

    let args = Args::parse();
    let endpoint = args
        .endpoint
        .map(LocalEndpoint::from_override)
        .unwrap_or_else(LocalEndpoint::default_for_user);
    let state_dir = args.state_dir.unwrap_or_else(default_state_dir);
    let service = DaemonService::new(state_dir)?;

    tracing::info!(endpoint = %endpoint, "OSCIRIS daemon listening");
    service.serve(endpoint).await
}
