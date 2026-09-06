use std::io::{self, IsTerminal};
use std::path::PathBuf;

use clap::Parser;
use mediaops_apiserver::{ApiConfig, serve_api};
use mediaops_home_client::{default_api_socket, default_state_dir};

#[derive(Parser, Debug)]
#[command(name = "mediaops-api", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Serve the Home API on a unix socket.
    Serve {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        api_db: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { socket, api_db } => {
            let config = ApiConfig {
                socket: socket.unwrap_or_else(default_api_socket),
                api_db: api_db.unwrap_or_else(|| default_state_dir().join("api.db")),
            };
            tracing::info!(socket = %config.socket.display(), db = %config.api_db.display(), "mediaops-api");
            serve_api(config).await?;
        }
    }
    Ok(())
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt().with_writer(io::stderr);
    if io::stderr().is_terminal() {
        subscriber.init();
    } else {
        subscriber.json().init();
    }
}
