//! Replay recorded JSON request/response fixtures through [`HttpTransport`].

use std::fs;
use std::path::{Path, PathBuf};

use mediaops_core::Blake3Hex;
use serde::Deserialize;

use crate::transport::{
    HttpRequest, HttpResponse, HttpTransport, TransportError, url_path_and_query,
};

#[derive(Debug, Deserialize)]
struct CassetteFile {
    request: CassetteRequest,
    response: CassetteResponse,
}

#[derive(Debug, Deserialize)]
struct CassetteRequest {
    method: String,
    path: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    body_json: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CassetteResponse {
    status: u16,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    body_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct Entry {
    method: String,
    path: String,
    body_digest: Option<String>,
    response: HttpResponse,
}

/// In-memory cassette set. Misses are errors, never live HTTP.
#[derive(Debug, Clone, Default)]
pub struct CassetteTransport {
    entries: Vec<Entry>,
}

impl CassetteTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/arr")
    }

    pub fn from_dir(dir: &Path) -> Result<Self, TransportError> {
        let mut transport = Self::new();
        let reader = fs::read_dir(dir).map_err(|err| TransportError::Io(err.to_string()))?;
        let mut files: Vec<PathBuf> = reader
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        files.sort();
        for path in files {
            let text =
                fs::read_to_string(&path).map_err(|err| TransportError::Io(err.to_string()))?;
            transport.push_json(&text)?;
        }
        Ok(transport)
    }

    pub fn from_workspace_fixtures() -> Result<Self, TransportError> {
        Self::from_dir(&Self::fixtures_dir())
    }

    pub fn push_json(&mut self, text: &str) -> Result<(), TransportError> {
        let file: CassetteFile =
            serde_json::from_str(text).map_err(|err| TransportError::Io(err.to_string()))?;
        let body_digest = request_digest(&file.request)?;
        let body = response_body(&file.response)?;
        self.entries.push(Entry {
            method: file.request.method.to_ascii_uppercase(),
            path: file.request.path,
            body_digest,
            response: HttpResponse {
                status: file.response.status,
                headers: file.response.headers,
                body,
            },
        });
        Ok(())
    }

    pub fn push(&mut self, method: &str, path: &str, body: Option<&[u8]>, response: HttpResponse) {
        self.entries.push(Entry {
            method: method.to_ascii_uppercase(),
            path: path.to_string(),
            body_digest: body.map(cassette_body_digest),
            response,
        });
    }

    fn lookup(&self, req: &HttpRequest) -> Result<&HttpResponse, TransportError> {
        let method = req.method.to_ascii_uppercase();
        let path = url_path_and_query(&req.url);
        let digest = req
            .body
            .as_deref()
            .map(cassette_body_digest)
            .unwrap_or_else(|| cassette_body_digest(&[]));
        let mut wildcard = None;
        for entry in &self.entries {
            if entry.method != method || entry.path != path {
                continue;
            }
            match &entry.body_digest {
                Some(want) if want == &digest => return Ok(&entry.response),
                None => wildcard = Some(&entry.response),
                Some(_) => continue,
            }
        }
        wildcard.ok_or_else(|| TransportError::CassetteMiss(format!("{method} {path} {digest}")))
    }
}

impl HttpTransport for CassetteTransport {
    async fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.lookup(req).cloned()
    }
}

pub fn cassette_body_digest(body: &[u8]) -> String {
    Blake3Hex::of_bytes(body).as_str().to_string()
}

pub fn cassette_key(req: &HttpRequest) -> String {
    let path = url_path_and_query(&req.url);
    let digest = req
        .body
        .as_deref()
        .map(cassette_body_digest)
        .unwrap_or_else(|| cassette_body_digest(&[]));
    format!("{} {path} {digest}", req.method.to_ascii_uppercase())
}

fn request_digest(req: &CassetteRequest) -> Result<Option<String>, TransportError> {
    if let Some(value) = &req.body_json {
        let bytes = serde_json::to_vec(value).map_err(|err| TransportError::Io(err.to_string()))?;
        return Ok(Some(cassette_body_digest(&bytes)));
    }
    if let Some(text) = &req.body {
        return Ok(Some(cassette_body_digest(text.as_bytes())));
    }
    Ok(None)
}

fn response_body(resp: &CassetteResponse) -> Result<Vec<u8>, TransportError> {
    if let Some(value) = &resp.body_json {
        return serde_json::to_vec(value).map_err(|err| TransportError::Io(err.to_string()));
    }
    Ok(resp.body.clone().unwrap_or_default().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn miss_is_an_error_not_live_http() {
        let t = CassetteTransport::new();
        let err = t
            .send(&HttpRequest {
                method: "GET".into(),
                url: "http://127.0.0.1/sonarr/api/v3/indexer".into(),
                headers: Vec::new(),
                body: None,
            })
            .await
            .expect_err("miss");
        assert!(matches!(err, TransportError::CassetteMiss(_)));
    }

    #[tokio::test]
    async fn exact_body_wins_over_wildcard_path() {
        let mut t = CassetteTransport::new();
        t.push(
            "POST",
            "/api/v3/indexer",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"wild".to_vec(),
            },
        );
        t.push(
            "POST",
            "/api/v3/indexer",
            Some(b"{\"name\":\"NZBgeek\"}"),
            HttpResponse {
                status: 400,
                headers: Vec::new(),
                body: b"dup".to_vec(),
            },
        );
        let resp = t
            .send(&HttpRequest {
                method: "POST".into(),
                url: "http://127.0.0.1:9696/api/v3/indexer".into(),
                headers: Vec::new(),
                body: Some(b"{\"name\":\"NZBgeek\"}".to_vec()),
            })
            .await
            .expect("hit");
        assert_eq!(resp.status, 400);
        assert_eq!(resp.body, b"dup");
    }
}
