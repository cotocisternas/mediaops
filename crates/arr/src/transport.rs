//! HTTP as a port. Production impl is reqwest; tests replay cassettes (AD-15).

use std::fmt;
use std::future::Future;

/// Outbound request. Headers are ordered pairs so cassettes stay stable.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &redact_secrets(&self.url))
            .field("headers", &redacted_headers(&self.headers))
            .field(
                "body",
                &self
                    .body
                    .as_deref()
                    .map(|b| redact_secrets(&String::from_utf8_lossy(b))),
            )
            .finish()
    }
}

/// Inbound response. Body is raw bytes; JSON parsing is the client's job.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &redacted_headers(&self.headers))
            .field(
                "body",
                &redact_secrets(&String::from_utf8_lossy(&self.body)),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("transport: {0}")]
    Io(String),
    #[error("cassette miss: {0}")]
    CassetteMiss(String),
}

impl TransportError {
    pub fn redacted(self) -> Self {
        match self {
            Self::Io(msg) => Self::Io(redact_secrets(&msg)),
            Self::CassetteMiss(msg) => Self::CassetteMiss(redact_secrets(&msg)),
        }
    }
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

/// Percent-encode a query value (`application/x-www-form-urlencoded` unreserved).
pub fn query_encode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Cassette identity for a URL or already-stripped path: host gone, secrets replaced.
pub fn cassette_path(url_or_path: &str) -> String {
    redact_query_secrets(url_path_and_query(url_or_path))
}

fn redact_query_secrets(path: &str) -> String {
    let Some((base, query)) = path.split_once('?') else {
        return path.to_string();
    };
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_secret_query_key(k) => format!("{k}=KEY"),
            Some((k, v)) => format!("{k}={v}"),
            None => pair.to_string(),
        })
        .collect();
    format!("{base}?{}", pairs.join("&"))
}

fn is_secret_query_key(k: &str) -> bool {
    k.eq_ignore_ascii_case("apikey")
        || k.eq_ignore_ascii_case("password")
        || k.eq_ignore_ascii_case("pass")
}

fn is_secret_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("x-api-key")
        || name.eq_ignore_ascii_case("cookie")
        || name.eq_ignore_ascii_case("set-cookie")
        || name.eq_ignore_ascii_case("authorization")
}

fn redacted_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            let value = if is_secret_header(k) {
                "<redacted>".into()
            } else {
                redact_secrets(v)
            };
            (k.clone(), value)
        })
        .collect()
}

/// Replace secret query/header/JSON values so errors and Debug never echo keys.
pub fn redact_secrets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let needles = [
        "apikey=",
        "password=",
        "pass=",
        "x-api-key:",
        "\"apikey\":\"",
        "\"apikey\": \"",
    ];
    let mut i = 0;
    while i < s.len() {
        let rest = &lower[i..];
        if let Some(nlen) = needles
            .iter()
            .find(|n| rest.starts_with(*n))
            .map(|n| n.len())
        {
            out.push_str(&s[i..i + nlen]);
            out.push_str("KEY");
            i += nlen;
            while i < s.len() {
                let b = s.as_bytes()[i];
                if matches!(
                    b,
                    b'&' | b' ' | b'"' | b'\'' | b',' | b';' | b'}' | b'\n' | b'\r'
                ) {
                    break;
                }
                i += 1;
            }
            continue;
        }
        let ch = s[i..].chars().next().expect("char");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
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

    #[test]
    fn cassette_path_redacts_apikey() {
        assert_eq!(
            cassette_path("http://127.0.0.1:8080/api?mode=queue&apikey=super-secret&output=json"),
            "/api?mode=queue&apikey=KEY&output=json"
        );
        assert_eq!(
            cassette_path("/api?mode=queue&apikey=k&output=json"),
            "/api?mode=queue&apikey=KEY&output=json"
        );
    }

    #[test]
    fn debug_does_not_echo_secrets() {
        let req = HttpRequest {
            method: "GET".into(),
            url: "http://127.0.0.1:8080/api?apikey=super-secret".into(),
            headers: vec![
                ("X-Api-Key".into(), "header-secret".into()),
                ("Cookie".into(), "SID=cookie-secret".into()),
            ],
            body: Some(b"username=admin&password=pw-secret".to_vec()),
        };
        let debug = format!("{req:?}");
        assert!(!debug.contains("super-secret"));
        assert!(!debug.contains("header-secret"));
        assert!(!debug.contains("cookie-secret"));
        assert!(!debug.contains("pw-secret"));
    }

    #[test]
    fn query_encode_escapes_ampersand_and_space() {
        assert_eq!(query_encode("a b&c"), "a%20b%26c");
    }
}
