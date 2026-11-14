mod app;
mod monitor;
mod net;
mod theme;

use app::{initial_backend_url, DirectorApp};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "ptz-director", about = "PTZ Director operator app")]
struct Args {
    /// Authoritative backend URL (remembered in app config)
    #[arg(long)]
    backend: Option<String>,
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ptz_client=info".into()),
        )
        .init();

    let args = Args::parse();
    let backend_url = initial_backend_url(args.backend);

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([1100.0, 700.0])
        .with_title("PTZ Director");

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "PTZ Director",
        options,
        Box::new(move |cc| Ok(Box::new(DirectorApp::new(cc, backend_url)))),
    )
}
