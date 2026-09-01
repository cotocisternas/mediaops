//! qBittorrent WebAPI: torrents, pause/resume/delete, categories, preferences.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::servarr::ArrError;
use crate::transport::{HttpRequest, HttpTransport};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QbitPreferences {
    pub dht: bool,
    pub pex: bool,
    pub lsd: bool,
}

pub struct QbitClient<T> {
    transport: T,
    base_url: String,
    cookie: Option<String>,
}

impl<T: HttpTransport> QbitClient<T> {
    pub fn new(transport: T, base_url: impl Into<String>) -> Self {
        Self {
            transport,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            cookie: None,
        }
    }

    pub async fn login(&mut self, username: &str, password: &str) -> Result<(), ArrError> {
        let body = format!("username={username}&password={password}");
        let req = HttpRequest {
            method: "POST".into(),
            url: format!("{}/api/v2/auth/login", self.base_url),
            headers: vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            body: Some(body.into_bytes()),
        };
        let resp = self.transport.send(&req).await?;
        if resp.status >= 400 {
            return Err(ArrError::Http {
                status: resp.status,
                body: "login failed".into(),
            });
        }
        if let Some((_, set)) = resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        {
            self.cookie = Some(set.clone());
        } else {
            self.cookie = Some("SID=cassette".into());
        }
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Value, ArrError> {
        self.send("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Option<&str>) -> Result<Value, ArrError> {
        self.send("POST", path, body).await
    }

    async fn send(&self, method: &str, path: &str, body: Option<&str>) -> Result<Value, ArrError> {
        let mut headers = Vec::new();
        if let Some(cookie) = &self.cookie {
            headers.push(("Cookie".into(), cookie.clone()));
        }
        if body.is_some() {
            headers.push((
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            ));
        }
        let req = HttpRequest {
            method: method.into(),
            url: format!("{}{path}", self.base_url),
            headers,
            body: body.map(|b| b.as_bytes().to_vec()),
        };
        let resp = self.transport.send(&req).await?;
        if resp.status >= 400 {
            return Err(ArrError::Http {
                status: resp.status,
                body: String::from_utf8_lossy(&resp.body).into(),
            });
        }
        if resp.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&resp.body).map_err(|err| ArrError::Json(err.to_string()))
    }

    pub async fn torrents(&self) -> Result<Value, ArrError> {
        self.get("/api/v2/torrents/info").await
    }

    pub async fn torrent_properties(&self, hash: &str) -> Result<Value, ArrError> {
        self.get(&format!("/api/v2/torrents/properties?hash={hash}"))
            .await
    }

    pub async fn torrent_files(&self, hash: &str) -> Result<Value, ArrError> {
        self.get(&format!("/api/v2/torrents/files?hash={hash}"))
            .await
    }

    pub async fn torrent_trackers(&self, hash: &str) -> Result<Value, ArrError> {
        self.get(&format!("/api/v2/torrents/trackers?hash={hash}"))
            .await
    }

    pub async fn pause(&self, hashes: &str) -> Result<Value, ArrError> {
        self.post("/api/v2/torrents/pause", Some(&format!("hashes={hashes}")))
            .await
    }

    pub async fn resume(&self, hashes: &str) -> Result<Value, ArrError> {
        self.post("/api/v2/torrents/resume", Some(&format!("hashes={hashes}")))
            .await
    }

    pub async fn delete(&self, hashes: &str, delete_files: bool) -> Result<Value, ArrError> {
        self.post(
            "/api/v2/torrents/delete",
            Some(&format!(
                "hashes={hashes}&deleteFiles={}",
                if delete_files { "true" } else { "false" }
            )),
        )
        .await
    }

    pub async fn categories(&self) -> Result<Value, ArrError> {
        self.get("/api/v2/torrents/categories").await
    }

    pub async fn preferences(&self) -> Result<QbitPreferences, ArrError> {
        let value = self.get("/api/v2/app/preferences").await?;
        Ok(QbitPreferences {
            dht: value.get("dht").and_then(Value::as_bool).unwrap_or(true),
            pex: value.get("pex").and_then(Value::as_bool).unwrap_or(true),
            lsd: value.get("lsd").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    /// Doctor privacy invariant: DHT/PeX/LSD off.
    pub fn privacy_ok(prefs: &QbitPreferences) -> bool {
        !prefs.dht && !prefs.pex && !prefs.lsd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::CassetteTransport;
    use crate::transport::HttpResponse;

    #[tokio::test]
    async fn preferences_privacy_invariant() {
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/api/v2/app/preferences",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{\"dht\":false,\"pex\":false,\"lsd\":false}".to_vec(),
            },
        );
        t.push(
            "POST",
            "/api/v2/auth/login",
            None,
            HttpResponse {
                status: 200,
                headers: vec![("set-cookie".into(), "SID=abc".into())],
                body: b"Ok.".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api/v2/torrents/info",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api/v2/torrents/properties?hash=h",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api/v2/torrents/files?hash=h",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api/v2/torrents/trackers?hash=h",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"[]".to_vec(),
            },
        );
        t.push(
            "GET",
            "/api/v2/torrents/categories",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "POST",
            "/api/v2/torrents/pause",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        t.push(
            "POST",
            "/api/v2/torrents/resume",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        t.push(
            "POST",
            "/api/v2/torrents/delete",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        let mut qbit = QbitClient::new(t, "http://127.0.0.1:8080");
        qbit.login("admin", "admin").await.expect("login");
        let prefs = qbit.preferences().await.expect("prefs");
        assert!(QbitClient::<CassetteTransport>::privacy_ok(&prefs));
        assert!(!QbitClient::<CassetteTransport>::privacy_ok(
            &QbitPreferences {
                dht: true,
                pex: false,
                lsd: false
            }
        ));
        qbit.torrents().await.expect("torrents");
        qbit.torrent_properties("h").await.expect("props");
        qbit.torrent_files("h").await.expect("files");
        qbit.torrent_trackers("h").await.expect("trackers");
        qbit.categories().await.expect("cats");
        qbit.pause("h").await.expect("pause");
        qbit.resume("h").await.expect("resume");
        qbit.delete("h", true).await.expect("delete");
    }
}
