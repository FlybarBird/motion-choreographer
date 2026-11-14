use clap::Parser;
use ptz_server::{spawn_ticker, ShowEngine};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "ptz-server", about = "PTZ Director authoritative show server")]
struct Args {
    /// Listen address
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,

    /// Data directory for audio assets and song drafts
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ptz_server=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();
    let data_dir = args.data_dir.unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ptz-director")
    });

    let engine = ShowEngine::new(data_dir.clone());
    spawn_ticker(Arc::clone(&engine));

    let app = ptz_server::api::router(Arc::clone(&engine));
    info!(
        "PTZ Director server listening on http://{} (api /api/v1, data {})",
        args.listen,
        data_dir.display()
    );
    info!("Default demo songs: Walk-In, Opening Number, Awards Bed");
    info!("Hardware: FR7 + AutoPod simulators (no physical I/O in this build)");

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
