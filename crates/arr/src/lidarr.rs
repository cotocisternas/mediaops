//! Lidarr facade: artist / album / track / trackfile / metadata profile / parse.

use serde_json::Value;

use crate::servarr::{ArrClient, ArrError};
use crate::transport::HttpTransport;

pub struct Lidarr<T> {
    pub client: ArrClient<T>,
}

impl<T: HttpTransport> Lidarr<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ArrError> {
        Ok(Self {
            client: ArrClient::new(transport, base_url, "/api/v1", api_key)?,
        })
    }

    pub async fn artists(&self) -> Result<Value, ArrError> {
        self.client.get_json("artist").await
    }

    pub async fn albums(&self) -> Result<Value, ArrError> {
        self.client.get_json("album").await
    }

    pub async fn put_album(&self, id: i64, body: &Value) -> Result<Value, ArrError> {
        self.client.put_json(&format!("album/{id}"), body).await
    }

    pub async fn tracks(&self, album_id: i64) -> Result<Value, ArrError> {
        self.client
            .get_json(&format!("track?albumId={album_id}"))
            .await
    }

    pub async fn track_file(&self, id: i64) -> Result<Value, ArrError> {
        self.client.get_json(&format!("trackfile/{id}")).await
    }

    pub async fn metadata_profiles(&self) -> Result<Value, ArrError> {
        self.client.get_json("metadataprofile").await
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
    async fn artists_list_replays_cassette() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/lidarr/api/v1/artist",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/lidarr/api/v1/album",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/lidarr/api/v1/track?albumId=1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/lidarr/api/v1/trackfile/3",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/lidarr/api/v1/metadataprofile",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/lidarr/api/v1/parse?title=x",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "PUT",
            "/lidarr/api/v1/album/1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        let lidarr = Lidarr::new(t, "http://127.0.0.1:8686/lidarr", "k").expect("l");
        assert_eq!(
            lidarr.artists().await.expect("artists"),
            serde_json::json!([])
        );
        lidarr.albums().await.expect("albums");
        lidarr.tracks(1).await.expect("tracks");
        lidarr.track_file(3).await.expect("file");
        lidarr.metadata_profiles().await.expect("meta");
        lidarr.parse("x").await.expect("parse");
        lidarr
            .put_album(1, &serde_json::json!({"id": 1}))
            .await
            .expect("put");
    }
}
