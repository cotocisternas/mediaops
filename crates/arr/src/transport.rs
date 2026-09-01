//! HTTP as a port. Production impl is reqwest; tests replay cassettes (AD-15).

use std::future::Future;

/// Outbound request. Headers are ordered pairs so cassettes stay stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Inbound response. Body is raw bytes; JSON parsing is the client's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("transport: {0}")]
    Io(String),
    #[error("cassette miss: {0}")]
    CassetteMiss(String),
}

pub trait HttpTransport: Send + Sync {
    fn send(
        &self,
        req: &HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send;
}

impl<T: HttpTransport + ?Sized> HttpTransport for std::sync::Arc<T> {
    fn send(
        &self,
        req: &HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send {
        let inner = std::sync::Arc::clone(self);
        let req = req.clone();
        async move { inner.as_ref().send(&req).await }
    }
}

/// Path + query of `url`, host stripped so cassettes are address-independent.
pub fn url_path_and_query(url: &str) -> &str {
    if let Some(scheme) = url.find("://") {
        let rest = &url[scheme + 3..];
        if let Some(slash) = rest.find('/') {
            return &rest[slash..];
        }
        return "/";
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_and_query_strips_host() {
        assert_eq!(
            url_path_and_query("http://127.0.0.1:8989/sonarr/api/v3/indexer"),
            "/sonarr/api/v3/indexer"
        );
        assert_eq!(
            url_path_and_query("http://127.0.0.1:8080/api?mode=queue"),
            "/api?mode=queue"
        );
        assert_eq!(url_path_and_query("http://127.0.0.1:8989"), "/");
        assert_eq!(url_path_and_query("/already/path"), "/already/path");
    }
}
