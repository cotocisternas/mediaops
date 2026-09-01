//! Prowlarr facade: indexers, applications (url_base required), app-sync, proxies, search.

use serde_json::Value;

use crate::servarr::{ArrClient, ArrError, IndexerIdentity};
use crate::transport::HttpTransport;

pub struct Prowlarr<T> {
    pub client: ArrClient<T>,
}

impl<T: HttpTransport> Prowlarr<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ArrError> {
        Ok(Self {
            client: ArrClient::new(transport, base_url, "/api/v1", api_key)?,
        })
    }

    pub async fn indexers(&self) -> Result<Vec<IndexerIdentity>, ArrError> {
        self.client.indexers().await
    }

    pub async fn applications(&self) -> Result<Value, ArrError> {
        self.client.get_json("applications").await
    }

    pub async fn put_application(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.client
            .put_json(&format!("applications/{id}"), body)
            .await
    }

    pub async fn app_sync(&self) -> Result<Value, ArrError> {
        self.client
            .command(&serde_json::json!({"name": "ApplicationIndexersSync"}))
            .await
    }

    pub async fn proxies(&self) -> Result<Value, ArrError> {
        self.client.get_json("indexerProxy").await
    }

    pub async fn search(&self, query: &str) -> Result<Value, ArrError> {
        self.client.get_json(&format!("search?query={query}")).await
    }

    /// Doctor invariant: application URLs must include the Prowlarr `url_base`.
    pub fn application_url_ok(sync_level_url: &str, url_base: &str) -> bool {
        if url_base.is_empty() {
            return false;
        }
        let base = url_base.trim_end_matches('/');
        sync_level_url.contains(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::CassetteTransport;
    use crate::transport::HttpResponse;

    #[tokio::test]
    async fn applications_replay_and_url_base_check() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/prowlarr/api/v1/applications",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: serde_json::to_vec(&serde_json::json!([
                    {"id": 1, "name": "Sonarr", "syncLevel": "full", "baseUrl": "http://127.0.0.1:8989/sonarr"}
                ]))
                .expect("json"),
            },
        );
        t.push(
            "GET",
            "/prowlarr/api/v1/indexer",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/prowlarr/api/v1/indexerProxy",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/prowlarr/api/v1/search?query=x",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "POST",
            "/prowlarr/api/v1/command",
            None,
            HttpResponse {
                status: 201,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "PUT",
            "/prowlarr/api/v1/applications/1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        let prowlarr = Prowlarr::new(t, "http://127.0.0.1:9696/prowlarr", "k").expect("p");
        let apps = prowlarr.applications().await.expect("apps");
        prowlarr.indexers().await.expect("idx");
        prowlarr.proxies().await.expect("proxies");
        prowlarr.search("x").await.expect("search");
        prowlarr.app_sync().await.expect("sync");
        prowlarr
            .put_application(1, &serde_json::json!({"id": 1}))
            .await
            .expect("put");
        let url = apps[0]["baseUrl"].as_str().expect("url");
        assert!(Prowlarr::<CassetteTransport>::application_url_ok(
            url, "/sonarr"
        ));
        assert!(!Prowlarr::<CassetteTransport>::application_url_ok(
            "http://127.0.0.1:8989/1/",
            "/prowlarr"
        ));
        assert!(!Prowlarr::<CassetteTransport>::application_url_ok(
            "/1/",
            "/prowlarr"
        ));
    }
}
