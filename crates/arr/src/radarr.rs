//! Radarr facade: movie / moviefile / collections / parse.

use serde_json::Value;

use crate::servarr::{ArrClient, ArrError};
use crate::transport::HttpTransport;

pub struct Radarr<T> {
    pub client: ArrClient<T>,
}

impl<T: HttpTransport> Radarr<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ArrError> {
        Ok(Self {
            client: ArrClient::new(transport, base_url, "/api/v3", api_key)?,
        })
    }

    pub async fn movies(&self) -> Result<Value, ArrError> {
        self.client.get_json("movie").await
    }

    pub async fn put_movie(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.client.put_json(&format!("movie/{id}"), body).await
    }

    pub async fn movie_file(&self, id: i64) -> Result<Value, ArrError> {
        self.client.get_json(&format!("moviefile/{id}")).await
    }

    pub async fn collections(&self) -> Result<Value, ArrError> {
        self.client.get_json("collection").await
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
    async fn movies_list_replays_cassette() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/radarr/api/v3/movie",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/radarr/api/v3/moviefile/2",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/radarr/api/v3/collection",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/radarr/api/v3/parse?title=x",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "PUT",
            "/radarr/api/v3/movie/1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        let radarr = Radarr::new(t, "http://127.0.0.1:7878/radarr", "k").expect("r");
        assert_eq!(
            radarr.movies().await.expect("movies"),
            serde_json::json!([])
        );
        radarr.movie_file(2).await.expect("file");
        radarr.collections().await.expect("collections");
        radarr.parse("x").await.expect("parse");
        radarr
            .put_movie(1, &serde_json::json!({"id": 1}))
            .await
            .expect("put");
    }
}
