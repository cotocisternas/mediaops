//! UDS and TCP serve/connect through one rustls config.

use std::path::Path;
use std::sync::Arc;

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
use crate::mint::SERVER_NAME;
use crate::seedbox::Seedbox;

pub async fn serve_tcp(
    listener: TcpListener,
    server: Arc<ServerConfig>,
    seedbox: Seedbox,
) -> Result<(), NetError> {
    let acceptor = TlsAcceptor::from(server);
    let incoming = TcpListenerStream::new(listener)
        .then(move |item| {
            let acceptor = acceptor.clone();
            async move {
                match item {
                    Ok(stream) => match acceptor.accept(stream).await {
                        Ok(tls) => Some(Ok(tls)),
                        Err(err) => {
                            tracing::warn!(error = %err, "tls handshake failed");
                            None
                        }
                    },
                    Err(err) => Some(Err(err)),
                }
            }
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
            async move {
                match item {
                    Ok(stream) => match acceptor.accept(stream).await {
                        Ok(tls) => Some(Ok(tls)),
                        Err(err) => {
                            tracing::warn!(error = %err, "tls handshake failed");
                            None
                        }
                    },
                    Err(err) => Some(Err(err)),
                }
            }
        })
        .filter_map(|item| item);
    serve_incoming(incoming, seedbox).await
}

async fn serve_incoming<S, E>(
    incoming: impl tokio_stream::Stream<Item = Result<S, E>> + Send + 'static,
    seedbox: Seedbox,
) -> Result<(), NetError>
where
    S: tonic::transport::server::Connected + tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Server::builder()
        .add_service(mediaops_proto::control_server::ControlServer::new(
            seedbox.clone(),
        ))
        .add_service(mediaops_proto::transfer_server::TransferServer::new(seedbox))
        .serve_with_incoming(incoming)
        .await
        .map_err(|err| NetError::Serve(err.to_string()))
}

pub async fn connect_tcp(addr: std::net::SocketAddr, client: Arc<ClientConfig>) -> Result<Channel, NetError> {
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
    Endpoint::from_shared("http://localhost")
        .map_err(|err| NetError::Connect(err.to_string()))?
        .connect_with_connector(svc)
        .await
        .map_err(|err| NetError::Connect(err.to_string()))
}
