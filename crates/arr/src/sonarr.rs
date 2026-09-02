//! Sonarr facade: series / season / episode / episodefile / parse.

use serde_json::Value;

use crate::servarr::{ArrClient, ArrError};
use crate::transport::HttpTransport;

pub struct Sonarr<T> {
    pub client: ArrClient<T>,
}

impl<T: HttpTransport> Sonarr<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ArrError> {
        Ok(Self {
            client: ArrClient::new(transport, base_url, "/api/v3", api_key)?,
        })
    }

    pub async fn series(&self) -> Result<Value, ArrError> {
        self.client.get_json("series").await
    }

    pub async fn put_series(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.client.put_json(&format!("series/{id}"), body).await
    }

    pub async fn seasons(&self, series_id: i64) -> Result<Value, ArrError> {
        let series = self.client.get_json(&format!("series/{series_id}")).await?;
        Ok(series
            .get("seasons")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())))
    }

    pub async fn episodes(&self, series_id: i64) -> Result<Value, ArrError> {
        self.client
            .get_json(&format!("episode?seriesId={series_id}"))
            .await
    }

    pub async fn episode_file(&self, id: i64) -> Result<Value, ArrError> {
        self.client.get_json(&format!("episodefile/{id}")).await
    }

    pub async fn parse(&self, title: &str) -> Result<Value, ArrError> {
        self.client
            .get_json(&format!(
                "parse?title={}",
                crate::transport::query_encode(title)
            ))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::CassetteTransport;
    use crate::transport::HttpResponse;

    #[tokio::test]
    async fn series_list_replays_cassette() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/series",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/sonarr/api/v3/series/1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"id\":1,\"seasons\":[]}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/sonarr/api/v3/episode?seriesId=1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/sonarr/api/v3/episodefile/9",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/sonarr/api/v3/parse?title=x",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/series/1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        let sonarr = Sonarr::new(t, "http://127.0.0.1:8989/sonarr", "k").expect("s");
        let value = sonarr.series().await.expect("series");
        assert_eq!(value, serde_json::json!([]));
        sonarr.seasons(1).await.expect("seasons");
        sonarr.episodes(1).await.expect("episodes");
        sonarr.episode_file(9).await.expect("file");
        sonarr.parse("x").await.expect("parse");
        sonarr
            .put_series(1, &serde_json::json!({"id": 1}))
            .await
            .expect("put");
    }
}
