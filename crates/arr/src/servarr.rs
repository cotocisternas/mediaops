//! Shared Servarr HTTP: `X-Api-Key`, `url_base`, generic JSON verbs.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::keys::{is_masked_key, refuse_key};
use crate::transport::{
    HttpRequest, HttpResponse, HttpTransport, TransportError, query_encode, redact_secrets,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArrError {
    #[error("masked API key refused")]
    MaskedKey,
    #[error("transport: {0}")]
    Transport(String),
    #[error("http {status}")]
    Http { status: u16, body: String },
    #[error("json: {0}")]
    Json(String),
    #[error("duplicate indexer `{0}`")]
    DuplicateIndexer(String),
    #[error("duplicate download client `{0}`")]
    DuplicateDownloadClient(String),
    #[error("{0}")]
    Other(String),
}

impl From<TransportError> for ArrError {
    fn from(err: TransportError) -> Self {
        Self::Transport(redact_secrets(&err.redacted().to_string()))
    }
}

impl From<crate::keys::KeyError> for ArrError {
    fn from(err: crate::keys::KeyError) -> Self {
        match err {
            crate::keys::KeyError::MaskedKey => Self::MaskedKey,
            crate::keys::KeyError::EmptyKey => Self::Other("empty API key".into()),
            crate::keys::KeyError::Io(msg) => Self::Other(msg),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexerIdentity {
    pub id: Option<i64>,
    pub name: String,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadClientIdentity {
    pub id: Option<i64>,
    pub name: String,
    pub priority: i32,
    #[serde(default)]
    pub implementation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConfig {
    pub bind_address: String,
    pub url_base: String,
    pub authentication_method: String,
}

pub struct ArrClient<T> {
    transport: T,
    base_url: String,
    api_prefix: String,
    api_key: String,
}

impl<T> fmt::Debug for ArrClient<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArrClient")
            .field("base_url", &self.base_url)
            .field("api_prefix", &self.api_prefix)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl<T: HttpTransport> ArrClient<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        api_prefix: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ArrError> {
        let api_key = api_key.into();
        refuse_key(&api_key)?;
        Ok(Self {
            transport,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_prefix: api_prefix.into(),
            api_key,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, resource: &str) -> String {
        let prefix = self.api_prefix.trim_matches('/');
        let resource = resource.trim_start_matches('/');
        format!("{}/{prefix}/{resource}", self.base_url)
    }

    async fn send(
        &self,
        method: &str,
        resource: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, ArrError> {
        let bytes = match body {
            Some(value) => {
                Some(serde_json::to_vec(value).map_err(|err| ArrError::Json(err.to_string()))?)
            }
            None => None,
        };
        let req = HttpRequest {
            method: method.into(),
            url: self.url(resource),
            headers: vec![
                ("X-Api-Key".into(), self.api_key.clone()),
                ("Accept".into(), "application/json".into()),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: bytes,
        };
        if req
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("X-Api-Key") && is_masked_key(v))
        {
            return Err(ArrError::MaskedKey);
        }
        self.transport.send(&req).await.map_err(ArrError::from)
    }

    pub async fn get_json(&self, resource: &str) -> Result<Value, ArrError> {
        self.json("GET", resource, None).await
    }

    pub async fn put_json(&self, resource: &str, body: &Value) -> Result<Value, ArrError> {
        self.json("PUT", resource, Some(body)).await
    }

    pub async fn post_json(&self, resource: &str, body: &Value) -> Result<Value, ArrError> {
        self.json("POST", resource, Some(body)).await
    }

    pub async fn delete(&self, resource: &str) -> Result<(), ArrError> {
        let resp = self.send("DELETE", resource, None).await?;
        if resp.status >= 400 {
            return Err(http_error(&resp));
        }
        Ok(())
    }

    async fn json(
        &self,
        method: &str,
        resource: &str,
        body: Option<&Value>,
    ) -> Result<Value, ArrError> {
        let resp = self.send(method, resource, body).await?;
        if resp.status >= 400 {
            return Err(http_error(&resp));
        }
        if resp.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&resp.body).map_err(|err| ArrError::Json(err.to_string()))
    }

    pub async fn host_config(&self) -> Result<HostConfig, ArrError> {
        parse_host_config(&self.get_json("config/host").await?)
    }

    pub async fn put_host_config(&self, body: &Value) -> Result<Value, ArrError> {
        self.put_json("config/host", body).await
    }

    pub async fn ui_config(&self) -> Result<Value, ArrError> {
        self.get_json("config/ui").await
    }

    pub async fn naming(&self) -> Result<Value, ArrError> {
        self.get_json("config/naming").await
    }

    pub async fn media_management(&self) -> Result<Value, ArrError> {
        self.get_json("config/mediamanagement").await
    }

    pub async fn quality_profiles(&self) -> Result<Value, ArrError> {
        self.get_json("qualityprofile").await
    }

    pub async fn quality_definitions(&self) -> Result<Value, ArrError> {
        self.get_json("qualitydefinition").await
    }

    pub async fn custom_formats(&self) -> Result<Value, ArrError> {
        self.get_json("customformat").await
    }

    pub async fn put_custom_format(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.put_json(&format!("customformat/{id}"), body).await
    }

    pub async fn post_custom_format(&self, body: &Value) -> Result<Value, ArrError> {
        self.post_json("customformat", body).await
    }

    pub async fn delay_profiles(&self) -> Result<Value, ArrError> {
        self.get_json("delayprofile").await
    }

    pub async fn put_delay_profile(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.put_json(&format!("delayprofile/{id}"), body).await
    }

    pub async fn indexers(&self) -> Result<Vec<IndexerIdentity>, ArrError> {
        let value = self.get_json("indexer").await?;
        identities_from_array(&value, indexer_from_value)
    }

    pub async fn put_indexer(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.put_json(&format!("indexer/{id}"), body).await
    }

    pub async fn post_indexer(&self, body: &Value) -> Result<Value, ArrError> {
        let name = body
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("indexer");
        if live_has_name(&self.indexers().await?, name) {
            return Err(ArrError::DuplicateIndexer(name.to_string()));
        }
        match self.post_json("indexer", body).await {
            Err(ArrError::Http { status: 409, .. }) => {
                Err(ArrError::DuplicateIndexer(name.to_string()))
            }
            other => other,
        }
    }

    pub async fn delete_indexer(&self, id: i64) -> Result<(), ArrError> {
        self.delete(&format!("indexer/{id}")).await
    }

    pub async fn download_clients(&self) -> Result<Vec<DownloadClientIdentity>, ArrError> {
        let value = self.get_json("downloadclient").await?;
        identities_from_array(&value, client_from_value)
    }

    pub async fn put_download_client(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.put_json(&format!("downloadclient/{id}"), body).await
    }

    pub async fn post_download_client(&self, body: &Value) -> Result<Value, ArrError> {
        let name = body
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("downloadclient");
        match self.post_json("downloadclient", body).await {
            Err(ArrError::Http { status: 409, .. }) => {
                Err(ArrError::DuplicateDownloadClient(name.to_string()))
            }
            other => other,
        }
    }

    pub async fn delete_download_client(&self, id: i64) -> Result<(), ArrError> {
        self.delete(&format!("downloadclient/{id}")).await
    }

    pub async fn import_lists(&self) -> Result<Value, ArrError> {
        self.get_json("importlist").await
    }

    pub async fn root_folders(&self) -> Result<Value, ArrError> {
        self.get_json("rootfolder").await
    }

    pub async fn tags(&self) -> Result<Value, ArrError> {
        self.get_json("tag").await
    }

    pub async fn notifications(&self) -> Result<Value, ArrError> {
        self.get_json("notification").await
    }

    pub async fn system_status(&self) -> Result<Value, ArrError> {
        self.get_json("system/status").await
    }

    pub async fn health(&self) -> Result<Value, ArrError> {
        self.get_json("health").await
    }

    pub async fn diskspace(&self) -> Result<Value, ArrError> {
        self.get_json("diskspace").await
    }

    pub async fn queue(&self) -> Result<Value, ArrError> {
        self.get_paged("queue").await
    }

    pub async fn history(&self) -> Result<Value, ArrError> {
        self.get_paged("history").await
    }

    pub async fn blocklist(&self) -> Result<Value, ArrError> {
        self.get_paged("blocklist").await
    }

    pub async fn wanted_missing(&self) -> Result<Value, ArrError> {
        self.get_paged("wanted/missing").await
    }

    pub async fn wanted_cutoff(&self) -> Result<Value, ArrError> {
        self.get_paged("wanted/cutoff").await
    }

    pub async fn calendar(&self) -> Result<Value, ArrError> {
        self.get_json("calendar").await
    }

    pub async fn command(&self, body: &Value) -> Result<Value, ArrError> {
        self.post_json("command", body).await
    }

    pub async fn commands(&self) -> Result<Value, ArrError> {
        self.get_json("command").await
    }

    pub async fn backups(&self) -> Result<Value, ArrError> {
        self.get_json("system/backup").await
    }

    pub async fn filesystem(&self, path: &str) -> Result<Value, ArrError> {
        self.get_json(&format!("filesystem?path={}", query_encode(path)))
            .await
    }

    pub async fn manual_import(&self) -> Result<Value, ArrError> {
        self.get_json("manualimport").await
    }

    pub async fn post_manual_import(&self, body: &Value) -> Result<Value, ArrError> {
        self.post_json("manualimport", body).await
    }

    pub async fn release_search(&self, query: &str) -> Result<Value, ArrError> {
        self.get_json(&format!("release?term={}", query_encode(query)))
            .await
    }

    pub async fn grab_release(&self, body: &Value) -> Result<Value, ArrError> {
        self.post_json("release", body).await
    }

    pub async fn test_indexer(&self, body: &Value) -> Result<Value, ArrError> {
        if body_contains_masked_key(body) {
            return Err(ArrError::MaskedKey);
        }
        self.post_json("indexer/test", body).await
    }

    async fn get_paged(&self, resource: &str) -> Result<Value, ArrError> {
        const PAGE_SIZE: u32 = 200;
        let mut page: u32 = 1;
        let mut all = Vec::new();
        let mut total: Option<u64> = None;
        loop {
            let path = format!("{resource}?page={page}&pageSize={PAGE_SIZE}");
            let value = self.get_json(&path).await?;
            if let Some(arr) = value.as_array() {
                if page == 1 && all.is_empty() {
                    return Ok(value);
                }
                all.extend(arr.iter().cloned());
                break;
            }
            let batch = value
                .get("records")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            total = value.get("totalRecords").and_then(Value::as_u64).or(total);
            let n = batch.len();
            all.extend(batch);
            if n == 0 {
                break;
            }
            if let Some(t) = total {
                if (all.len() as u64) >= t {
                    break;
                }
            } else if n < PAGE_SIZE as usize {
                break;
            }
            page += 1;
            if page > 100 {
                return Err(ArrError::Other(format!(
                    "paged {resource} exceeded 100 pages"
                )));
            }
        }
        Ok(serde_json::json!({"records": all, "totalRecords": all.len()}))
    }
}

fn body_contains_masked_key(body: &Value) -> bool {
    if body
        .get("apiKey")
        .and_then(Value::as_str)
        .is_some_and(is_masked_key)
    {
        return true;
    }
    let Some(fields) = body.get("fields").and_then(Value::as_array) else {
        return false;
    };
    fields.iter().any(|field| {
        let name = field.get("name").and_then(Value::as_str).unwrap_or("");
        (name.eq_ignore_ascii_case("apiKey") || name.eq_ignore_ascii_case("api_key"))
            && field
                .get("value")
                .and_then(Value::as_str)
                .is_some_and(is_masked_key)
    })
}

pub(crate) fn http_error(resp: &HttpResponse) -> ArrError {
    let body = String::from_utf8_lossy(&resp.body);
    let body = redact_secrets(&body);
    let body = if is_masked_key(&body) {
        "<redacted>".into()
    } else {
        truncate(&body, 256)
    };
    ArrError::Http {
        status: resp.status,
        body,
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(n.min(s.len()));
        s[..end].to_string()
    }
}

fn identities_from_array<T>(
    value: &Value,
    map: fn(&Value) -> Result<T, ArrError>,
) -> Result<Vec<T>, ArrError> {
    let Some(items) = value.as_array() else {
        return Err(ArrError::Json("expected array".into()));
    };
    items.iter().map(map).collect()
}

fn indexer_from_value(value: &Value) -> Result<IndexerIdentity, ArrError> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ArrError::Json("indexer missing name".into()))?;
    Ok(IndexerIdentity {
        id: value.get("id").and_then(Value::as_i64),
        name: name.to_string(),
        priority: i32::try_from(value.get("priority").and_then(Value::as_i64).unwrap_or(25))
            .unwrap_or(25),
    })
}

fn client_from_value(value: &Value) -> Result<DownloadClientIdentity, ArrError> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ArrError::Json("download client missing name".into()))?;
    Ok(DownloadClientIdentity {
        id: value.get("id").and_then(Value::as_i64),
        name: name.to_string(),
        priority: i32::try_from(value.get("priority").and_then(Value::as_i64).unwrap_or(1))
            .unwrap_or(1),
        implementation: value
            .get("implementation")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn live_has_name(indexers: &[IndexerIdentity], name: &str) -> bool {
    indexers.iter().any(|idx| idx.name == name)
}

pub fn parse_host_config(value: &Value) -> Result<HostConfig, ArrError> {
    Ok(HostConfig {
        bind_address: require_str(value, "bindAddress")?,
        url_base: require_str(value, "urlBase")?,
        authentication_method: require_str(value, "authenticationMethod")?,
    })
}

fn require_str(value: &Value, key: &str) -> Result<String, ArrError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ArrError::Json(format!("missing {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::CassetteTransport;

    fn json_ok(body: Value) -> crate::transport::HttpResponse {
        crate::transport::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(&body).expect("json"),
        }
    }

    #[test]
    fn masked_key_is_refused_at_construction() {
        let t = CassetteTransport::new();
        let err = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "********")
            .expect_err("masked");
        assert_eq!(err, ArrError::MaskedKey);
        let err = ArrClient::new(
            CassetteTransport::new(),
            "http://127.0.0.1:8989/sonarr",
            "/api/v3",
            "",
        )
        .expect_err("empty");
        assert!(matches!(err, ArrError::Other(msg) if msg.contains("empty")));
    }

    #[test]
    fn debug_does_not_echo_key() {
        let t = CassetteTransport::new();
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "secret-key")
            .expect("client");
        let debug = format!("{client:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-key"));
    }

    #[tokio::test]
    async fn indexer_list_and_duplicate_post_conflict() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(serde_json::json!([{"id":1,"name":"NZBgeek","priority":25}])),
        );
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let idx = client.indexers().await.expect("list");
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].name, "NZBgeek");
        let err = client
            .post_indexer(&serde_json::json!({"name":"NZBgeek","priority":25}))
            .await
            .expect_err("dup");
        assert!(matches!(err, ArrError::DuplicateIndexer(name) if name == "NZBgeek"));
    }

    #[tokio::test]
    async fn test_indexer_refuses_masked_api_key_in_body() {
        let t = CassetteTransport::new();
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let err = client
            .test_indexer(&serde_json::json!({"name":"NZBgeek","apiKey":"********"}))
            .await
            .expect_err("masked");
        assert_eq!(err, ArrError::MaskedKey);
        let err = client
            .test_indexer(&serde_json::json!({
                "name": "NZBgeek",
                "fields": [{"name": "apiKey", "value": "********"}]
            }))
            .await
            .expect_err("fields");
        assert_eq!(err, ArrError::MaskedKey);
    }

    #[tokio::test]
    async fn post_indexer_maps_409_cassette_to_duplicate() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(serde_json::json!([])),
        );
        let fixture = include_str!("../../../fixtures/arr/duplicate_nzbgeek.json");
        t.push_json(fixture).expect("fixture");
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let err = client
            .post_indexer(&serde_json::json!({"name":"NZBgeek","priority":25}))
            .await
            .expect_err("409");
        assert!(matches!(err, ArrError::DuplicateIndexer(name) if name == "NZBgeek"));
    }

    #[tokio::test]
    async fn post_download_client_maps_409() {
        let mut t = CassetteTransport::new();
        t.push(
            "POST",
            "/sonarr/api/v3/downloadclient",
            None,
            crate::transport::HttpResponse {
                status: 409,
                headers: Vec::new(),
                body: b"{\"message\":\"Should be unique\"}".to_vec(),
            },
        );
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let err = client
            .post_download_client(&serde_json::json!({"name":"SABnzbd"}))
            .await
            .expect_err("409");
        assert!(matches!(err, ArrError::DuplicateDownloadClient(name) if name == "SABnzbd"));
    }

    #[tokio::test]
    async fn test_indexer_maps_401_cassette() {
        let mut t = CassetteTransport::new();
        let fixture = include_str!("../../../fixtures/arr/indexer_test_unauthorized.json");
        t.push_json(fixture).expect("fixture");
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let err = client
            .test_indexer(&serde_json::json!({
                "name": "NZBgeek",
                "apiKey": "not-stars"
            }))
            .await
            .expect_err("401");
        assert!(matches!(err, ArrError::Http { status: 401, .. }));
    }

    #[tokio::test]
    async fn nameless_indexer_is_json_error() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(serde_json::json!([{"id": 1}])),
        );
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let err = client.indexers().await.expect_err("name");
        assert!(matches!(err, ArrError::Json(_)));
    }

    #[test]
    fn parse_host_config_requires_fields() {
        let err = parse_host_config(&serde_json::json!({})).expect_err("missing");
        assert!(matches!(err, ArrError::Json(_)));
    }

    #[test]
    fn truncate_does_not_panic_on_char_boundary() {
        let s = "é".repeat(200);
        let out = truncate(&s, 1);
        assert!(s.is_char_boundary(out.len()));
        assert!(out.len() <= 1);
    }

    #[test]
    fn http_error_redacts_key_shaped_json() {
        let err = http_error(&crate::transport::HttpResponse {
            status: 401,
            headers: Vec::new(),
            body: br#"{"message":"bad","apiKey":"super-secret"}"#.to_vec(),
        });
        let ArrError::Http { body, .. } = err else {
            panic!("http");
        };
        assert!(!body.contains("super-secret"), "{body}");
    }

    #[tokio::test]
    async fn paged_queue_walks_records() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/queue?page=1&pageSize=200",
            None,
            json_ok(serde_json::json!({
                "records": [{"id": 1}],
                "totalRecords": 2,
                "page": 1,
                "pageSize": 1
            })),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/queue?page=2&pageSize=200",
            None,
            json_ok(serde_json::json!({
                "records": [{"id": 2}],
                "totalRecords": 2,
                "page": 2,
                "pageSize": 1
            })),
        );
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let queue = client.queue().await.expect("queue");
        assert_eq!(queue["records"].as_array().expect("records").len(), 2);
    }

    #[tokio::test]
    async fn host_config_parses_bind_and_url_base() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(serde_json::json!({
                "bindAddress": "127.0.0.1",
                "urlBase": "/sonarr",
                "authenticationMethod": "forms"
            })),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(serde_json::json!({"bindAddress":"127.0.0.1"})),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/customformat/1",
            None,
            json_ok(serde_json::json!({"id":1})),
        );
        t.push(
            "POST",
            "/sonarr/api/v3/customformat",
            None,
            json_ok(serde_json::json!({"id":2})),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/delayprofile/1",
            None,
            json_ok(serde_json::json!({"id":1})),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/indexer/1",
            None,
            json_ok(serde_json::json!({"id":1})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(serde_json::json!([{"id":1,"name":"SABnzbd","priority":1,"implementation":"Sabnzbd"}])),
        );
        t.push(
            "POST",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(serde_json::json!({"id":2})),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/downloadclient/1",
            None,
            json_ok(serde_json::json!({"id":1})),
        );
        t.push(
            "DELETE",
            "/sonarr/api/v3/downloadclient/1",
            None,
            crate::transport::HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        let host = client.host_config().await.expect("host");
        assert_eq!(host.bind_address, "127.0.0.1");
        assert_eq!(host.url_base, "/sonarr");
        assert_eq!(host.authentication_method, "forms");
        client
            .put_host_config(&serde_json::json!({"bindAddress":"127.0.0.1"}))
            .await
            .expect("put host");
        client
            .put_custom_format(1, &serde_json::json!({"id":1}))
            .await
            .expect("put cf");
        client
            .post_custom_format(&serde_json::json!({"name":"x264"}))
            .await
            .expect("post cf");
        client
            .put_delay_profile(1, &serde_json::json!({"id":1}))
            .await
            .expect("delay");
        client
            .put_indexer(1, &serde_json::json!({"id":1}))
            .await
            .expect("put idx");
        let clients = client.download_clients().await.expect("clients");
        assert_eq!(clients[0].name, "SABnzbd");
        client
            .post_download_client(&serde_json::json!({"name":"qBittorrent"}))
            .await
            .expect("post dc");
        client
            .put_download_client(1, &serde_json::json!({"id":1}))
            .await
            .expect("put dc");
        client.delete_download_client(1).await.expect("del dc");
    }
}
