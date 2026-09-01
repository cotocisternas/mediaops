//! Grabber set-diff apply and runtime key discovery (GrabOps).

use mediaops_core::{
    BoxFuture, ControlError, DesiredState, EdgeApiReport, GrabApplyReport, GrabOps, Grabber,
    KeyPresence,
};
use serde_json::{Value, json};

use crate::keys::{KeyPaths, discover_keys};
use crate::prowlarr::Prowlarr;
use crate::servarr::{ArrClient, ArrError, HostConfig};
use crate::transport::HttpTransport;

/// Localhost *arr clients built from discovered keys + url_base.
#[derive(Clone)]
pub struct LocalhostGrabOps<T> {
    transport: T,
    key_paths: KeyPaths,
    sonarr_base: String,
    radarr_base: String,
    lidarr_base: String,
    prowlarr_base: String,
    bind: String,
    auth: String,
    url_bases: std::collections::BTreeMap<String, String>,
}

impl<T> LocalhostGrabOps<T> {
    pub fn new(transport: T, key_paths: KeyPaths, desired: &DesiredState) -> Self {
        let bases = desired
            .edge()
            .map(|e| e.url_bases.clone())
            .unwrap_or_default();
        Self {
            transport,
            key_paths,
            sonarr_base: localhost(
                8989,
                bases.get("sonarr").map(String::as_str).unwrap_or("/sonarr"),
            ),
            radarr_base: localhost(
                7878,
                bases.get("radarr").map(String::as_str).unwrap_or("/radarr"),
            ),
            lidarr_base: localhost(
                8686,
                bases.get("lidarr").map(String::as_str).unwrap_or("/lidarr"),
            ),
            prowlarr_base: localhost(
                9696,
                bases
                    .get("prowlarr")
                    .map(String::as_str)
                    .unwrap_or("/prowlarr"),
            ),
            bind: desired
                .edge()
                .map(|e| e.bind.clone())
                .unwrap_or_else(|| "127.0.0.1".into()),
            auth: desired
                .edge()
                .map(|e| e.auth.clone())
                .unwrap_or_else(|| "forms".into()),
            url_bases: bases,
        }
    }
}

fn localhost(port: u16, url_base: &str) -> String {
    let base = if url_base.starts_with('/') {
        url_base.to_string()
    } else {
        format!("/{url_base}")
    };
    format!("http://127.0.0.1:{port}{base}")
}

impl<T: HttpTransport + Clone + Send + Sync + 'static> GrabOps for LocalhostGrabOps<T> {
    fn grab_apply<'a>(
        &'a self,
        desired: &'a DesiredState,
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
        Box::pin(async move {
            if desired.grabber() != Grabber::Servarr {
                return Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                });
            }
            let keys = discover_keys(&self.key_paths).map_err(key_to_control)?;
            let mut diffs = Vec::new();
            if let Some(key) = keys.sonarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.sonarr_base, "/api/v3", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "sonarr", desired, true).await?);
            }
            if let Some(key) = keys.radarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.radarr_base, "/api/v3", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "radarr", desired, true).await?);
            }
            if let Some(key) = keys.lidarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.lidarr_base, "/api/v1", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "lidarr", desired, true).await?);
            }
            if let Some(key) = keys.prowlarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.prowlarr_base, "/api/v1", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "prowlarr", desired, false).await?);
            }
            Ok(GrabApplyReport {
                noop: diffs.is_empty(),
                diff: diffs.join("\n"),
            })
        })
    }

    fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>> {
        Box::pin(async move {
            let keys = discover_keys(&self.key_paths).map_err(key_to_control)?;
            Ok(keys.presence())
        })
    }

    fn edge_api_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>> {
        Box::pin(async move {
            let drift = check_edge_apps(self).await?;
            Ok(EdgeApiReport {
                fingerprint: String::new(),
                invariant_ok: drift.is_empty(),
                drift: drift.join("; "),
            })
        })
    }

    fn edge_apply<'a>(
        &'a self,
        desired: &'a DesiredState,
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
        Box::pin(async move {
            let diffs = apply_edge_host(self, desired).await?;
            Ok(GrabApplyReport {
                noop: diffs.is_empty(),
                diff: diffs.join("\n"),
            })
        })
    }
}

async fn apply_app<T: HttpTransport>(
    client: &ArrClient<T>,
    app: &str,
    desired: &DesiredState,
    media_app: bool,
) -> Result<Vec<String>, ControlError> {
    let mut diffs = Vec::new();
    diffs.extend(apply_indexers(client, app, desired).await?);
    if media_app {
        diffs.extend(apply_download_clients(client, desired).await?);
        diffs.extend(apply_cf_packs(client, desired).await?);
        diffs.extend(apply_policy(client, desired).await?);
    }
    Ok(diffs)
}

async fn apply_indexers<T: HttpTransport>(
    client: &ArrClient<T>,
    app: &str,
    desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let live = client.indexers().await.map_err(arr_to_control)?;
    let mut seen = std::collections::HashSet::new();
    for idx in &live {
        if !seen.insert(idx.name.as_str()) {
            return Err(ControlError::policy(format!(
                "duplicate indexer `{}`",
                idx.name
            )));
        }
    }
    let want: Vec<_> = desired
        .grab()
        .indexers
        .iter()
        .filter(|i| i.app == app)
        .collect();
    let mut diffs = Vec::new();
    for idx in &live {
        if !want.iter().any(|w| w.name == idx.name) {
            if let Some(id) = idx.id {
                client.delete_indexer(id).await.map_err(arr_to_control)?;
                diffs.push(format!("-{app} indexer {}", idx.name));
            }
        }
    }
    for spec in want {
        match live.iter().find(|l| l.name == spec.name) {
            None => {
                client
                    .post_json(
                        "indexer",
                        &json!({
                            "name": spec.name,
                            "priority": spec.priority,
                            "enable": true
                        }),
                    )
                    .await
                    .map_err(arr_to_control)?;
                diffs.push(format!("+{app} indexer {}", spec.name));
            }
            Some(live_idx) if live_idx.priority != spec.priority => {
                if let Some(id) = live_idx.id {
                    client
                        .put_indexer(
                            id,
                            &json!({
                                "id": id,
                                "name": spec.name,
                                "priority": spec.priority,
                                "enable": true
                            }),
                        )
                        .await
                        .map_err(arr_to_control)?;
                    diffs.push(format!("~{app} indexer {} priority", spec.name));
                }
            }
            Some(_) => {}
        }
    }
    Ok(diffs)
}

async fn apply_download_clients<T: HttpTransport>(
    client: &ArrClient<T>,
    desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let live = client.download_clients().await.map_err(arr_to_control)?;
    let mut seen = std::collections::HashSet::new();
    for c in &live {
        if !seen.insert(c.name.as_str()) {
            return Err(ControlError::policy(format!(
                "duplicate download client `{}`",
                c.name
            )));
        }
    }
    let want = &desired.grab().download_clients;
    let mut diffs = Vec::new();
    for c in &live {
        if !want.iter().any(|w| w.name == c.name) {
            if let Some(id) = c.id {
                client
                    .delete_download_client(id)
                    .await
                    .map_err(arr_to_control)?;
                diffs.push(format!("-client {}", c.name));
            }
        }
    }
    for spec in want {
        match live.iter().find(|l| l.name == spec.name) {
            None => {
                client
                    .post_download_client(&json!({
                        "name": spec.name,
                        "priority": spec.priority,
                        "implementation": spec.kind.as_str(),
                        "enable": true
                    }))
                    .await
                    .map_err(arr_to_control)?;
                diffs.push(format!("+client {}", spec.name));
            }
            Some(live_c) if live_c.priority != spec.priority => {
                if let Some(id) = live_c.id {
                    client
                        .put_download_client(
                            id,
                            &json!({
                                "id": id,
                                "name": spec.name,
                                "priority": spec.priority,
                                "enable": true
                            }),
                        )
                        .await
                        .map_err(arr_to_control)?;
                    diffs.push(format!("~client {} priority", spec.name));
                }
            }
            Some(_) => {}
        }
    }
    Ok(diffs)
}

async fn apply_cf_packs<T: HttpTransport>(
    client: &ArrClient<T>,
    desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let packs = &desired.grab().custom_format_packs;
    if packs.is_empty() {
        return Ok(Vec::new());
    }
    let live = client.custom_formats().await.map_err(arr_to_control)?;
    let items = live.as_array().cloned().unwrap_or_default();
    let mut diffs = Vec::new();
    for pack in packs {
        for name in pack.scores.keys() {
            let existing = items
                .iter()
                .find(|v| v.get("name").and_then(Value::as_str) == Some(name.as_str()));
            match existing {
                Some(live_cf) => {
                    let id = live_cf.get("id").and_then(Value::as_i64).unwrap_or(0);
                    client
                        .put_custom_format(id, live_cf)
                        .await
                        .map_err(arr_to_control)?;
                }
                None => {
                    client
                        .post_custom_format(&json!({"name": name}))
                        .await
                        .map_err(arr_to_control)?;
                    diffs.push(format!("+cf {name}"));
                }
            }
        }
    }
    Ok(diffs)
}

async fn apply_policy<T: HttpTransport>(
    client: &ArrClient<T>,
    desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let policy = &desired.grab().policy;
    if policy.delay_minutes.is_none() && policy.quality_profile.is_none() {
        return Ok(Vec::new());
    }
    let mut diffs = Vec::new();
    if let Some(minutes) = policy.delay_minutes {
        let live = client.delay_profiles().await.map_err(arr_to_control)?;
        if let Some(first) = live.as_array().and_then(|a| a.first()) {
            let id = first.get("id").and_then(Value::as_i64).unwrap_or(1);
            let live_min = first.get("delay").and_then(Value::as_u64).unwrap_or(0);
            if live_min != u64::from(minutes) {
                let mut body = first.clone();
                body["delay"] = json!(minutes);
                client
                    .put_delay_profile(id, &body)
                    .await
                    .map_err(arr_to_control)?;
                diffs.push(format!("~delay {minutes}"));
            }
        }
    }
    Ok(diffs)
}

async fn check_edge_apps<T: HttpTransport + Clone>(
    ops: &LocalhostGrabOps<T>,
) -> Result<Vec<String>, ControlError> {
    let keys = discover_keys(&ops.key_paths).map_err(key_to_control)?;
    let mut drift = Vec::new();
    if let Some(key) = keys.sonarr() {
        drift.extend(
            check_host_client(
                &ArrClient::new(ops.transport.clone(), &ops.sonarr_base, "/api/v3", key)
                    .map_err(arr_to_control)?,
                ops.url_bases
                    .get("sonarr")
                    .map(String::as_str)
                    .unwrap_or("/sonarr"),
                &ops.bind,
                &ops.auth,
                "sonarr",
            )
            .await?,
        );
    }
    if let Some(key) = keys.radarr() {
        drift.extend(
            check_host_client(
                &ArrClient::new(ops.transport.clone(), &ops.radarr_base, "/api/v3", key)
                    .map_err(arr_to_control)?,
                ops.url_bases
                    .get("radarr")
                    .map(String::as_str)
                    .unwrap_or("/radarr"),
                &ops.bind,
                &ops.auth,
                "radarr",
            )
            .await?,
        );
    }
    if let Some(key) = keys.lidarr() {
        drift.extend(
            check_host_client(
                &ArrClient::new(ops.transport.clone(), &ops.lidarr_base, "/api/v1", key)
                    .map_err(arr_to_control)?,
                ops.url_bases
                    .get("lidarr")
                    .map(String::as_str)
                    .unwrap_or("/lidarr"),
                &ops.bind,
                &ops.auth,
                "lidarr",
            )
            .await?,
        );
    }
    if let Some(key) = keys.prowlarr() {
        let client = ArrClient::new(ops.transport.clone(), &ops.prowlarr_base, "/api/v1", key)
            .map_err(arr_to_control)?;
        drift.extend(
            check_host_client(
                &client,
                ops.url_bases
                    .get("prowlarr")
                    .map(String::as_str)
                    .unwrap_or("/prowlarr"),
                &ops.bind,
                &ops.auth,
                "prowlarr",
            )
            .await?,
        );
        let apps = client
            .get_json("applications")
            .await
            .map_err(arr_to_control)?;
        if let Some(arr) = apps.as_array() {
            let base = ops
                .url_bases
                .get("prowlarr")
                .map(String::as_str)
                .unwrap_or("/prowlarr");
            for app in arr {
                let url = app
                    .get("baseUrl")
                    .or_else(|| app.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !Prowlarr::<T>::application_url_ok(url, base) {
                    drift.push(format!("Prowlarr app URL `{url}` missing {base}"));
                }
            }
        }
    }
    Ok(drift)
}

async fn check_host_client<T: HttpTransport>(
    client: &ArrClient<T>,
    url_base: &str,
    bind: &str,
    auth: &str,
    app: &str,
) -> Result<Vec<String>, ControlError> {
    let host = client.host_config().await.map_err(arr_to_control)?;
    Ok(host_config_drift(&host, url_base, bind, auth, app))
}

pub fn host_config_drift(
    host: &HostConfig,
    url_base: &str,
    bind: &str,
    auth: &str,
    app: &str,
) -> Vec<String> {
    let mut drift = Vec::new();
    if host.bind_address == "*" || host.bind_address == "0.0.0.0" {
        drift.push(format!("{app} bind-to-star"));
    } else if host.bind_address != bind {
        drift.push(format!("{app} bind {}", host.bind_address));
    }
    if host.url_base.is_empty() {
        drift.push(format!("{app} missing url_base"));
    } else if host.url_base != url_base {
        drift.push(format!("{app} url_base {}", host.url_base));
    }
    if !host.authentication_method.eq_ignore_ascii_case(auth) {
        drift.push(format!("{app} auth {}", host.authentication_method));
    }
    drift
}

async fn apply_edge_host<T: HttpTransport + Clone>(
    ops: &LocalhostGrabOps<T>,
    _desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let keys = discover_keys(&ops.key_paths).map_err(key_to_control)?;
    let mut diffs = Vec::new();
    for (app, base, prefix, key) in [
        ("sonarr", ops.sonarr_base.as_str(), "/api/v3", keys.sonarr()),
        ("radarr", ops.radarr_base.as_str(), "/api/v3", keys.radarr()),
        ("lidarr", ops.lidarr_base.as_str(), "/api/v1", keys.lidarr()),
        (
            "prowlarr",
            ops.prowlarr_base.as_str(),
            "/api/v1",
            keys.prowlarr(),
        ),
    ] {
        let Some(key) = key else { continue };
        let client =
            ArrClient::new(ops.transport.clone(), base, prefix, key).map_err(arr_to_control)?;
        let live = client.host_config().await.map_err(arr_to_control)?;
        let url_base = ops
            .url_bases
            .get(app)
            .map(String::as_str)
            .unwrap_or_else(|| match app {
                "sonarr" => "/sonarr",
                "radarr" => "/radarr",
                "lidarr" => "/lidarr",
                _ => "/prowlarr",
            });
        if live.bind_address == ops.bind
            && live.url_base == url_base
            && live.authentication_method.eq_ignore_ascii_case(&ops.auth)
        {
            continue;
        }
        let body = json!({
            "bindAddress": ops.bind,
            "urlBase": url_base,
            "authenticationMethod": ops.auth,
        });
        client
            .put_host_config(&body)
            .await
            .map_err(arr_to_control)?;
        diffs.push(format!("~{app} host bind/url_base/auth"));
    }
    Ok(diffs)
}

fn arr_to_control(err: ArrError) -> ControlError {
    match err {
        ArrError::MaskedKey => ControlError::policy("masked API key refused"),
        ArrError::DuplicateIndexer(name) => {
            ControlError::policy(format!("duplicate indexer `{name}`"))
        }
        other => ControlError::runtime(other.to_string()),
    }
}

fn key_to_control(err: crate::keys::KeyError) -> ControlError {
    match err {
        crate::keys::KeyError::MaskedKey => ControlError::policy("masked API key refused"),
        crate::keys::KeyError::Io(msg) => ControlError::runtime(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cassette::CassetteTransport;
    use crate::transport::HttpResponse;
    use std::fs;

    fn json_ok(body: Value) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&body).expect("json"),
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-arr-apply-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn ds_servarr() -> DesiredState {
        DesiredState::from_toml(
            r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 0
range_len_mib = 1
max_nvenc = 1
lock = false
grabber = "servarr"

[[grab.indexers]]
name = "NZBgeek"
priority = 25
app = "sonarr"

[[grab.download_clients]]
name = "SABnzbd"
priority = 1
kind = "sabnzbd"
"#,
        )
        .expect("ds")
    }

    fn write_sonarr_key(home: &std::path::Path, key: &str) {
        let path = home.join(".config/Sonarr/config.xml");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, format!("<Config><ApiKey>{key}</ApiKey></Config>")).expect("xml");
    }

    #[tokio::test]
    async fn second_apply_is_noop_when_sets_match() {
        let home = scratch("noop");
        write_sonarr_key(&home, "k");
        let mut t = CassetteTransport::new();
        t.push("GET", "/sonarr/api/v3/indexer", None, json_ok(json!([])));
        t.push(
            "POST",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(json!({"id":1,"name":"NZBgeek","priority":25})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([])),
        );
        t.push(
            "POST",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!({"id":1,"name":"SABnzbd"})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(json!([{"id":1,"name":"NZBgeek","priority":25}])),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([{"id":1,"name":"SABnzbd","priority":1}])),
        );
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let first = ops.grab_apply(&ds_servarr()).await.expect("first");
        assert!(!first.noop, "{}", first.diff);
        assert!(first.diff.contains("+sonarr indexer NZBgeek"));
        let second = ops.grab_apply(&ds_servarr()).await.expect("second");
        assert!(
            second.noop,
            "second apply must be no-op, got {}",
            second.diff
        );
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn duplicate_nzbgeek_live_is_conflict_not_append() {
        let home = scratch("dup");
        write_sonarr_key(&home, "k");
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(json!([
                {"id":1,"name":"NZBgeek","priority":25},
                {"id":2,"name":"NZBgeek","priority":50}
            ])),
        );
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let err = ops.grab_apply(&ds_servarr()).await.expect_err("dup");
        assert_eq!(err.exit_code, mediaops_core::ExitCode::PolicyRefusal);
        assert!(err.message.contains("NZBgeek"));
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn masked_key_in_config_xml_is_refused() {
        let home = scratch("mask");
        write_sonarr_key(&home, "********");
        let t = CassetteTransport::new();
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let err = ops.grab_apply(&ds_servarr()).await.expect_err("masked");
        assert_eq!(err.exit_code, mediaops_core::ExitCode::PolicyRefusal);
        assert!(!err.message.contains("********") || err.message.contains("masked"));
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn grabber_none_is_handshake_noop() {
        let home = scratch("none");
        let ds = DesiredState::from_toml(
            "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 1\nmax_nvenc = 1\nlock = false\n",
        )
        .expect("ds");
        let ops = LocalhostGrabOps::new(CassetteTransport::new(), KeyPaths::from_home(&home), &ds);
        let report = ops.grab_apply(&ds).await.expect("none");
        assert!(report.noop);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn bind_to_star_missing_url_base_and_prowlarr_id_path_are_drift() {
        let star = HostConfig {
            bind_address: "*".into(),
            url_base: "/sonarr".into(),
            authentication_method: "forms".into(),
        };
        assert!(
            host_config_drift(&star, "/sonarr", "127.0.0.1", "forms", "sonarr")
                .iter()
                .any(|d| d.contains("bind-to-star"))
        );
        let missing = HostConfig {
            bind_address: "127.0.0.1".into(),
            url_base: String::new(),
            authentication_method: "forms".into(),
        };
        assert!(
            host_config_drift(&missing, "/sonarr", "127.0.0.1", "forms", "sonarr")
                .iter()
                .any(|d| d.contains("missing url_base"))
        );
        assert!(!Prowlarr::<CassetteTransport>::application_url_ok(
            "http://127.0.0.1:8989/1/",
            "/prowlarr"
        ));
        assert!(Prowlarr::<CassetteTransport>::application_url_ok(
            "http://127.0.0.1:8989/prowlarr/1/",
            "/prowlarr"
        ));
    }

    #[tokio::test]
    async fn edge_api_check_reports_bind_to_star() {
        let home = scratch("edge-star");
        write_sonarr_key(&home, "k");
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(json!({
                "bindAddress": "*",
                "urlBase": "/sonarr",
                "authenticationMethod": "forms"
            })),
        );
        let ds = ds_servarr();
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds);
        let report = ops.edge_api_check().await.expect("check");
        assert!(!report.invariant_ok);
        assert!(report.drift.contains("bind-to-star"));
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn cf_pack_policy_and_edge_apply_mutate_then_noop() {
        let home = scratch("cf-edge");
        write_sonarr_key(&home, "k");
        let toml = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 0
range_len_mib = 1
max_nvenc = 1
lock = false
grabber = "servarr"
[[grab.custom_format_packs]]
name = "prefer-h264"
scores = { "x264" = 100 }
[grab.policy]
delay_minutes = 5
[edge]
url_bases = { sonarr = "/sonarr" }
bind = "127.0.0.1"
auth = "forms"
"#;
        let ds = DesiredState::from_toml(toml).expect("ds");
        let mut t = CassetteTransport::new();
        t.push("GET", "/sonarr/api/v3/indexer", None, json_ok(json!([])));
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([])),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/customformat",
            None,
            json_ok(json!([])),
        );
        t.push(
            "POST",
            "/sonarr/api/v3/customformat",
            None,
            json_ok(json!({"id":1,"name":"x264"})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/delayprofile",
            None,
            json_ok(json!([{"id":1,"delay":0}])),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/delayprofile/1",
            None,
            json_ok(json!({"id":1,"delay":5})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(json!({
                "bindAddress": "*",
                "urlBase": "",
                "authenticationMethod": "none"
            })),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(json!({
                "bindAddress": "127.0.0.1",
                "urlBase": "/sonarr",
                "authenticationMethod": "forms"
            })),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(json!({
                "bindAddress": "127.0.0.1",
                "urlBase": "/sonarr",
                "authenticationMethod": "forms"
            })),
        );
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds);
        let grab = ops.grab_apply(&ds).await.expect("grab");
        assert!(!grab.noop);
        let edge = ops.edge_apply(&ds).await.expect("edge");
        assert!(!edge.noop);
        let edge2 = ops.edge_apply(&ds).await.expect("edge2");
        assert!(edge2.noop, "{}", edge2.diff);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn extra_live_indexer_is_deleted() {
        let home = scratch("del-idx");
        write_sonarr_key(&home, "k");
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(json!([
                {"id":1,"name":"NZBgeek","priority":25},
                {"id":2,"name":"Other","priority":1}
            ])),
        );
        t.push(
            "DELETE",
            "/sonarr/api/v3/indexer/2",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([{"id":1,"name":"SABnzbd","priority":1}])),
        );
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let report = ops.grab_apply(&ds_servarr()).await.expect("apply");
        assert!(
            report.diff.contains("-sonarr indexer Other"),
            "{}",
            report.diff
        );
        let _ = fs::remove_dir_all(home);
    }
}
