use std::net::SocketAddr;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use cmprovision::config::{Config, ServeArgs};
use cmprovision::{AppState, Clock, db, web};

#[derive(Parser)]
#[command(version, about = "Raspberry Pi Compute Module provisioning server")]
struct Cli {
    #[command(flatten)]
    config: Config,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the web interface and provisioning endpoint
    Serve(ServeArgs),
    /// Create or upgrade the database schema
    Migrate,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // ring instead of aws-lc keeps the build free of C toolchains, easing cross-compilation.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    let pool = db::connect(&cli.config.database()).await?;

    match cli.command {
        Command::Migrate => db::migrate(&pool).await,
        Command::Serve(args) => {
            db::migrate(&pool).await?;
            let state = AppState::new(cli.config, pool, Clock::System);
            state.hasher.enqueue_pending(&state.db).await?;
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            tracing::info!("listening on {}", args.listen);
            axum::serve(listener, web::router(state).into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(shutdown())
                .await?;
            Ok(())
        }
    }
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
