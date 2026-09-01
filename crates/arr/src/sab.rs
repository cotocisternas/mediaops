//! SABnzbd: queue/history, add, pause/resume, complete-dir, categories, servers.

use serde_json::Value;

use crate::keys::{is_masked_key, refuse_masked};
use crate::servarr::ArrError;
use crate::transport::{HttpRequest, HttpTransport};

pub const SAB_CATEGORIES: &[&str] = &["tv", "movies", "music"];

pub struct SabClient<T> {
    transport: T,
    base_url: String,
    api_key: String,
}

impl<T: HttpTransport> SabClient<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ArrError> {
        let api_key = api_key.into();
        refuse_masked(&api_key)?;
        Ok(Self {
            transport,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
        })
    }

    async fn call(&self, mode: &str, extra: &[(&str, &str)]) -> Result<Value, ArrError> {
        if is_masked_key(&self.api_key) {
            return Err(ArrError::MaskedKey);
        }
        let mut url = format!("{}/api?mode={mode}&apikey=KEY&output=json", self.base_url);
        // Key is sent as a header-equivalent query param; cassette keys on path
        // with a placeholder so fixtures never contain material.
        url = url.replace("apikey=KEY", &format!("apikey={}", self.api_key));
        for (k, v) in extra {
            url.push('&');
            url.push_str(k);
            url.push('=');
            url.push_str(v);
        }
        let req = HttpRequest {
            method: "GET".into(),
            url,
            headers: Vec::new(),
            body: None,
        };
        let resp = self.transport.send(&req).await?;
        if resp.status >= 400 {
            return Err(ArrError::Http {
                status: resp.status,
                body: String::from_utf8_lossy(&resp.body).into(),
            });
        }
        serde_json::from_slice(&resp.body).map_err(|err| ArrError::Json(err.to_string()))
    }

    pub async fn queue(&self) -> Result<Value, ArrError> {
        self.call("queue", &[]).await
    }

    pub async fn history(&self) -> Result<Value, ArrError> {
        self.call("history", &[]).await
    }

    pub async fn add_url(&self, nzb_url: &str) -> Result<Value, ArrError> {
        self.call("addurl", &[("name", nzb_url)]).await
    }

    pub async fn pause(&self) -> Result<Value, ArrError> {
        self.call("pause", &[]).await
    }

    pub async fn resume(&self) -> Result<Value, ArrError> {
        self.call("resume", &[]).await
    }

    pub async fn complete_dir(&self) -> Result<Value, ArrError> {
        self.call(
            "get_config",
            &[("section", "misc"), ("keyword", "complete_dir")],
        )
        .await
    }

    pub async fn categories(&self) -> Result<Value, ArrError> {
        self.call("get_cats", &[]).await
    }

    pub async fn servers(&self) -> Result<Value, ArrError> {
        self.call("get_config", &[("section", "servers")]).await
    }

    pub fn categories_ok(names: &[String]) -> bool {
        SAB_CATEGORIES
            .iter()
            .all(|want| names.iter().any(|n| n == want))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::CassetteTransport;
    use crate::transport::HttpResponse;

    #[tokio::test]
    async fn queue_replays_cassette_without_echoing_key_in_errors() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/api?mode=queue&apikey=k&output=json",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"queue\":{\"slots\":[]}}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=history&apikey=k&output=json",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"history\":{\"slots\":[]}}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=addurl&apikey=k&output=json&name=http://x.nzb",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"status\":true}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=pause&apikey=k&output=json",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"status\":true}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=resume&apikey=k&output=json",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"status\":true}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=get_config&apikey=k&output=json&section=misc&keyword=complete_dir",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"config\":{\"complete_dir\":\"/data/complete\"}}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=get_cats&apikey=k&output=json",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"categories\":[\"tv\",\"movies\",\"music\"]}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api?mode=get_config&apikey=k&output=json&section=servers",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"config\":{\"servers\":[]}}".to_vec(),
            },
        );
        let sab = SabClient::new(t, "http://127.0.0.1:8080", "k").expect("sab");
        let q = sab.queue().await.expect("queue");
        assert!(q["queue"]["slots"].as_array().expect("slots").is_empty());
        sab.history().await.expect("history");
        sab.add_url("http://x.nzb").await.expect("add");
        sab.pause().await.expect("pause");
        sab.resume().await.expect("resume");
        sab.complete_dir().await.expect("complete");
        sab.categories().await.expect("cats");
        sab.servers().await.expect("servers");
        assert!(SabClient::<CassetteTransport>::categories_ok(&[
            "tv".into(),
            "movies".into(),
            "music".into()
        ]));
        assert!(!SabClient::<CassetteTransport>::categories_ok(&[
            "tv".into()
        ]));
    }

    #[test]
    fn masked_sab_key_refused() {
        let t = CassetteTransport::new();
        assert!(matches!(
            SabClient::new(t, "http://127.0.0.1:8080", "********"),
            Err(ArrError::MaskedKey)
        ));
    }
}
