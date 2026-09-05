//! UDS and TCP serve/connect through one rustls config.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use http::Uri;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ServerConfig};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{TcpListenerStream, UnixListenerStream};
use tonic::transport::{Channel, Endpoint, Server};
use tower::service_fn;

use crate::NetError;
use crate::gateway::HomeGateway;
use crate::mint::SERVER_NAME;
use crate::seedbox::Seedbox;

static TCP_CONNECTS: AtomicU64 = AtomicU64::new(0);

/// How many times [`connect_tcp`] has run in this process (AD-12 tests).
pub fn tcp_connect_count() -> u64 {
    TCP_CONNECTS.load(Ordering::SeqCst)
}

pub async fn serve_tcp(
    listener: TcpListener,
    server: Arc<ServerConfig>,
    seedbox: Seedbox,
) -> Result<(), NetError> {
    let acceptor = TlsAcceptor::from(server);
    let incoming = TcpListenerStream::new(listener)
        .then(move |item| {
            let acceptor = acceptor.clone();
            async move { handshake_incoming(item, acceptor).await }
        })
        .filter_map(|item| item);
    serve_incoming(incoming, seedbox).await
}

pub async fn serve_unix(
    listener: UnixListener,
    server: Arc<ServerConfig>,
    seedbox: Seedbox,
) -> Result<(), NetError> {
    let acceptor = TlsAcceptor::from(server);
    let incoming = UnixListenerStream::new(listener)
        .then(move |item| {
            let acceptor = acceptor.clone();
            async move { handshake_incoming(item, acceptor).await }
        })
        .filter_map(|item| item);
    serve_incoming(incoming, seedbox).await
}

pub async fn serve_home_unix(
    listener: UnixListener,
    server: Arc<ServerConfig>,
    gateway: HomeGateway,
) -> Result<(), NetError> {
    let acceptor = TlsAcceptor::from(server);
    let incoming = UnixListenerStream::new(listener)
        .then(move |item| {
            let acceptor = acceptor.clone();
            async move { handshake_incoming(item, acceptor).await }
        })
        .filter_map(|item| item);
    serve_home_incoming(incoming, gateway).await
}

async fn handshake_incoming<S>(
    item: std::io::Result<S>,
    acceptor: TlsAcceptor,
) -> Option<std::io::Result<tokio_rustls::server::TlsStream<S>>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    match item {
        Ok(stream) => {
            match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await {
                Ok(Ok(tls)) => Some(Ok(tls)),
                Ok(Err(err)) => {
                    tracing::warn!(error = %err, "tls handshake failed");
                    None
                }
                Err(_) => {
                    tracing::warn!("tls handshake timed out");
                    None
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "accept failed");
            None
        }
    }
}

async fn serve_incoming<S, E>(
    incoming: impl tokio_stream::Stream<Item = Result<S, E>> + Send + 'static,
    seedbox: Seedbox,
) -> Result<(), NetError>
where
    S: tonic::transport::server::Connected
        + tokio::io::AsyncRead
        + tokio::io::AsyncWrite
        + Send
        + Unpin
        + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Server::builder()
        .add_service(
            mediaops_proto::control_service_server::ControlServiceServer::new(seedbox.clone()),
        )
        .add_service(mediaops_proto::transfer_service_server::TransferServiceServer::new(seedbox))
        .serve_with_incoming(incoming)
        .await
        .map_err(|err| NetError::Serve(err.to_string()))
}

async fn serve_home_incoming<S, E>(
    incoming: impl tokio_stream::Stream<Item = Result<S, E>> + Send + 'static,
    gateway: HomeGateway,
) -> Result<(), NetError>
where
    S: tonic::transport::server::Connected
        + tokio::io::AsyncRead
        + tokio::io::AsyncWrite
        + Send
        + Unpin
        + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Server::builder()
        .add_service(
            mediaops_proto::control_service_server::ControlServiceServer::new(gateway.clone()),
        )
        .add_service(
            mediaops_proto::transfer_service_server::TransferServiceServer::new(gateway.clone()),
        )
        .add_service(mediaops_proto::gateway_service_server::GatewayServiceServer::new(gateway))
        .serve_with_incoming(incoming)
        .await
        .map_err(|err| NetError::Serve(err.to_string()))
}

pub async fn connect_tcp(
    addr: std::net::SocketAddr,
    client: Arc<ClientConfig>,
) -> Result<Channel, NetError> {
    TCP_CONNECTS.fetch_add(1, Ordering::SeqCst);
    let connector = TlsConnector::from(client);
    let name = ServerName::try_from(SERVER_NAME).map_err(|err| NetError::Tls(err.to_string()))?;
    let svc = service_fn(move |_uri: Uri| {
        let connector = connector.clone();
        let name = name.clone();
        async move {
            let stream = TcpStream::connect(addr).await?;
            let _ = stream.set_nodelay(true);
            let tls = connector.connect(name, stream).await?;
            Ok::<_, std::io::Error>(TokioIo::new(tls))
        }
    });
    Endpoint::from_shared("http://localhost")
        .map_err(|err| NetError::Connect(err.to_string()))?
        .connect_timeout(Duration::from_secs(10))
        .concurrency_limit(1)
        .connect_with_connector(svc)
        .await
        .map_err(|err| NetError::Connect(err.to_string()))
}

pub async fn connect_unix(path: &Path, client: Arc<ClientConfig>) -> Result<Channel, NetError> {
    let connector = TlsConnector::from(client);
    let name = ServerName::try_from(SERVER_NAME).map_err(|err| NetError::Tls(err.to_string()))?;
    let path = path.to_path_buf();
    let svc = service_fn(move |_uri: Uri| {
        let connector = connector.clone();
        let name = name.clone();
        let path = path.clone();
        async move {
            let stream = UnixStream::connect(&path).await?;
            let tls = connector.connect(name, stream).await?;
            Ok::<_, std::io::Error>(TokioIo::new(tls))
        }
    });
    // UDS is the overlay: many Range RPCs multiplex here; WAN pinning is the pool.
    Endpoint::from_shared("http://localhost")
        .map_err(|err| NetError::Connect(err.to_string()))?
        .connect_timeout(Duration::from_secs(10))
        .connect_with_connector(svc)
        .await
        .map_err(|err| NetError::Connect(err.to_string()))
}
