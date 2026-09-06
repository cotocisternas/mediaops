mod socket;
mod tls;

use std::io::{self, IsTerminal};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use mediaops_core::{Actor, Kind, SECRET_NAME, Spec, UnderlayMode, endpoint_fingerprint};
use mediaops_home_client::{HomeApi, default_api_socket, default_gateway_socket, default_tls_dir};
use mediaops_net::{HomeGateway, serve_home_unix};

#[derive(Parser, Debug)]
#[command(name = "mediaops-gateway", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long)]
        api_socket: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        tls_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve {
            api_socket,
            socket,
            tls_dir,
        } => {
            let api_socket = api_socket.unwrap_or_else(default_api_socket);
            let gw_socket = socket.unwrap_or_else(default_gateway_socket);
            let tls_dir = tls_dir.unwrap_or_else(default_tls_dir);
            run(&api_socket, &gw_socket, &tls_dir).await?;
        }
    }
    Ok(())
}

async fn run(api_socket: &Path, gw_socket: &Path, tls_dir: &Path) -> anyhow::Result<()> {
    let api = wait_api(api_socket).await?;
    let secret = wait_secret(&api).await?;
    let Spec::Secret(spec) = &secret.spec else {
        anyhow::bail!("Secret body missing");
    };
    if spec.seedbox_address.is_empty() {
        anyhow::bail!("Secret.seedbox_address is empty");
    }
    let identity = tls::load_identity(tls_dir, spec)?;
    let server = identity.server_config()?;
    let client = tls::pinned_client(&identity)?;
    let listener = socket::bind(gw_socket).await?;
    let addr = parse_grpc_addr(&spec.seedbox_address).await?;
    let fingerprint = endpoint_fingerprint(&spec.seedbox_address, UnderlayMode::Direct);
    let gateway = HomeGateway::connect(addr, client, fingerprint, 1).await?;
    tracing::info!(socket = %gw_socket.display(), upstream = %addr, "gateway listen");
    tokio::select! {
        result = serve_home_unix(listener, server, gateway) => Ok(result?),
        result = secret_changed(&api, &secret) => {
            result?;
            // Process exit closes existing HTTP/2 connections as well as the
            // listener. The supervisor rebuilds every TLS session and pool.
            anyhow::bail!("Secret changed; restarting gateway to reload TLS and upstream");
        }
    }
}

async fn secret_changed(api: &HomeApi, initial: &mediaops_core::HomeObject) -> anyhow::Result<()> {
    // An unchanged Secret can be older than retained watch history. Start at
    // a fresh snapshot, not the object's last-write revision, on every boot.
    let mut watch = api.watch(Some(Kind::Secret), 0).await?;
    // Re-read after subscribing so startup and watch reconnection cannot miss
    // a Secret update between the original Get and opening the stream.
    if api.get(Kind::Secret, SECRET_NAME).await?.spec != initial.spec {
        return Ok(());
    }
    loop {
        let Some(_event) = watch.message().await? else {
            anyhow::bail!("Secret watch ended; restarting gateway to revalidate its identity");
        };
        if api.get(Kind::Secret, SECRET_NAME).await?.spec != initial.spec {
            return Ok(());
        }
    }
}

async fn wait_api(socket: &Path) -> anyhow::Result<HomeApi> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match HomeApi::connect(socket, Actor::Gateway).await {
            Ok(api) => return Ok(api),
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(anyhow::anyhow!("api: {err}"));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn wait_secret(api: &HomeApi) -> anyhow::Result<mediaops_core::HomeObject> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        match api.get(Kind::Secret, SECRET_NAME).await {
            Ok(obj) => return Ok(obj),
            Err(err) if err.is_not_found() => {
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("Secret `{SECRET_NAME}` not applied");
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

async fn parse_grpc_addr(raw: &str) -> anyhow::Result<SocketAddr> {
    if let Ok(addr) = raw.parse::<SocketAddr>() {
        return Ok(addr);
    }
    tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host(raw))
        .await??
        .next()
        .ok_or_else(|| anyhow::anyhow!("seedbox address `{raw}` did not resolve"))
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt().with_writer(io::stderr);
    if io::stderr().is_terminal() {
        subscriber.init();
    } else {
        subscriber.json().init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parse_grpc_addr_happy_and_garbage() {
        assert_eq!(
            parse_grpc_addr("127.0.0.1:50051").await.expect("addr"),
            "127.0.0.1:50051".parse::<SocketAddr>().expect("parse")
        );
        assert!(parse_grpc_addr("not-an-address").await.is_err());
    }
}
