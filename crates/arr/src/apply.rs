//! Grabber set-diff apply and runtime key discovery (GrabOps).

use mediaops_core::{
    BoxFuture, ControlError, DesiredState, EdgeApiReport, GrabApplyReport, GrabDownloadClient,
    GrabIndexer, GrabOps, Grabber, HoldKey, HoldLiveItem, KeyPresence, ReleaseId, TitleId,
    unified_diff,
};
use serde_json::{Value, json};

use crate::keys::{DiscoveredKeys, KeyPaths, discover_keys};
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
            let ops = self.with_desired(desired);
            let keys = discover_keys(&ops.key_paths).map_err(key_to_control)?;
            require_keys_for_desired(desired, &keys)?;
            let mut diffs = Vec::new();
            if let Some(key) = keys.sonarr() {
                let client =
                    ArrClient::new(ops.transport.clone(), &ops.sonarr_base, "/api/v3", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "sonarr", desired, true).await?);
            }
            if let Some(key) = keys.radarr() {
                let client =
                    ArrClient::new(ops.transport.clone(), &ops.radarr_base, "/api/v3", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "radarr", desired, true).await?);
            }
            if let Some(key) = keys.lidarr() {
                let client =
                    ArrClient::new(ops.transport.clone(), &ops.lidarr_base, "/api/v1", key)
                        .map_err(arr_to_control)?;
                diffs.extend(apply_app(&client, "lidarr", desired, true).await?);
            }
            if let Some(key) = keys.prowlarr() {
                let client =
                    ArrClient::new(ops.transport.clone(), &ops.prowlarr_base, "/api/v1", key)
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
            let ops = self.with_desired(desired);
            let diffs = apply_edge_host(&ops, desired).await?;
            Ok(GrabApplyReport {
                noop: diffs.is_empty(),
                diff: diffs.join("\n"),
            })
        })
    }

    fn hold_list(&self) -> BoxFuture<'_, Result<Vec<HoldLiveItem>, ControlError>> {
        Box::pin(async move {
            let keys = discover_keys(&self.key_paths).map_err(key_to_control)?;
            let mut out = Vec::new();
            if let Some(key) = keys.sonarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.sonarr_base, "/api/v3", key)
                        .map_err(arr_to_control)?;
                let queue = client
                    .get_paged_with("queue", "includeSeries=true&includeEpisode=true")
                    .await
                    .map_err(arr_to_control)?;
                out.extend(hold_items_from_queue(&queue));
            }
            if let Some(key) = keys.radarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.radarr_base, "/api/v3", key)
                        .map_err(arr_to_control)?;
                let queue = client
                    .get_paged_with("queue", "includeMovie=true")
                    .await
                    .map_err(arr_to_control)?;
                out.extend(hold_items_from_queue(&queue));
            }
            if let Some(key) = keys.lidarr() {
                let client =
                    ArrClient::new(self.transport.clone(), &self.lidarr_base, "/api/v1", key)
                        .map_err(arr_to_control)?;
                let queue = client
                    .get_paged_with("queue", "includeArtist=true&includeAlbum=true")
                    .await
                    .map_err(arr_to_control)?;
                out.extend(hold_items_from_queue(&queue));
            }
            Ok(out)
        })
    }
}

impl<T: Clone> LocalhostGrabOps<T> {
    fn with_desired(&self, desired: &DesiredState) -> Self {
        Self::new(self.transport.clone(), self.key_paths.clone(), desired)
    }
}

fn require_keys_for_desired(
    desired: &DesiredState,
    keys: &DiscoveredKeys,
) -> Result<(), ControlError> {
    for idx in &desired.grab().indexers {
        let present = match idx.app.as_str() {
            "sonarr" => keys.sonarr().is_some(),
            "radarr" => keys.radarr().is_some(),
            "lidarr" => keys.lidarr().is_some(),
            "prowlarr" => keys.prowlarr().is_some(),
            other => {
                return Err(ControlError::policy(format!("unknown grab app `{other}`")));
            }
        };
        if !present {
            return Err(ControlError::policy(format!("{} API key missing", idx.app)));
        }
    }
    let media_needed = !desired.grab().download_clients.is_empty()
        || !desired.grab().custom_format_packs.is_empty()
        || desired.grab().policy.delay_minutes.is_some()
        || desired.grab().policy.quality_profile.is_some();
    if media_needed && keys.sonarr().is_none() && keys.radarr().is_none() && keys.lidarr().is_none()
    {
        return Err(ControlError::policy("media *arr API key missing"));
    }
    Ok(())
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
        if idx.id.is_none() {
            return Err(ControlError::runtime(format!(
                "{app} indexer `{}` missing id",
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
    if want.is_empty() {
        return Ok(Vec::new());
    }
    let mut diffs = Vec::new();
    for idx in &live {
        if !want.iter().any(|w| w.name == idx.name) {
            let id = idx.id.expect("checked");
            client.delete_indexer(id).await.map_err(arr_to_control)?;
            diffs.push(format!("-{app} indexer {}", idx.name));
        }
    }
    for spec in want {
        match live.iter().find(|l| l.name == spec.name) {
            None => {
                let body = indexer_resource(spec, None);
                client.post_indexer(&body).await.map_err(arr_to_control)?;
                diffs.push(format!("+{app} indexer {}", spec.name));
            }
            Some(live_idx) => {
                let id = live_idx.id.expect("checked");
                let live_json = client
                    .get_json(&format!("indexer/{id}"))
                    .await
                    .map_err(arr_to_control)?;
                let merged = indexer_resource(spec, Some(&live_json));
                if live_json != merged {
                    client
                        .put_indexer(id, &merged)
                        .await
                        .map_err(arr_to_control)?;
                    diffs.push(format!("~{app} indexer {}", spec.name));
                }
            }
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
        if c.id.is_none() {
            return Err(ControlError::runtime(format!(
                "download client `{}` missing id",
                c.name
            )));
        }
    }
    let want = &desired.grab().download_clients;
    if want.is_empty() {
        return Ok(Vec::new());
    }
    let mut diffs = Vec::new();
    for c in &live {
        if !want.iter().any(|w| w.name == c.name) {
            let id = c.id.expect("checked");
            client
                .delete_download_client(id)
                .await
                .map_err(arr_to_control)?;
            diffs.push(format!("-client {}", c.name));
        }
    }
    for spec in want {
        match live.iter().find(|l| l.name == spec.name) {
            None => {
                let body = client_resource(spec, None);
                client
                    .post_download_client(&body)
                    .await
                    .map_err(arr_to_control)?;
                diffs.push(format!("+client {}", spec.name));
            }
            Some(live_c) => {
                let id = live_c.id.expect("checked");
                let live_json = client
                    .get_json(&format!("downloadclient/{id}"))
                    .await
                    .map_err(arr_to_control)?;
                let merged = client_resource(spec, Some(&live_json));
                if live_json != merged {
                    client
                        .put_download_client(id, &merged)
                        .await
                        .map_err(arr_to_control)?;
                    diffs.push(format!("~client {}", spec.name));
                }
            }
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
    let items = live
        .as_array()
        .cloned()
        .ok_or_else(|| ControlError::runtime("customformat not array"))?;
    let mut diffs = Vec::new();
    let mut known = items;
    for pack in packs {
        for name in pack.scores.keys() {
            let existing = known
                .iter()
                .find(|v| v.get("name").and_then(Value::as_str) == Some(name.as_str()))
                .cloned();
            match existing {
                Some(live_cf) => {
                    let id = live_cf
                        .get("id")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| ControlError::runtime(format!("cf `{name}` missing id")))?;
                    let mut body = live_cf.clone();
                    body["name"] = json!(name);
                    if body != live_cf {
                        client
                            .put_custom_format(id, &body)
                            .await
                            .map_err(arr_to_control)?;
                        diffs.push(format!("~cf {name}"));
                    }
                }
                None => {
                    let created = client
                        .post_custom_format(&json!({"name": name}))
                        .await
                        .map_err(arr_to_control)?;
                    known.push(created);
                    diffs.push(format!("+cf {name}"));
                }
            }
        }
    }
    if let Some(profile_name) = &desired.grab().policy.quality_profile {
        diffs.extend(apply_quality_profile_scores(client, profile_name, packs, &known).await?);
    }
    Ok(diffs)
}

async fn apply_quality_profile_scores<T: HttpTransport>(
    client: &ArrClient<T>,
    profile_name: &str,
    packs: &[mediaops_core::CustomFormatPack],
    cfs: &[Value],
) -> Result<Vec<String>, ControlError> {
    let profiles = client.quality_profiles().await.map_err(arr_to_control)?;
    let items = profiles
        .as_array()
        .ok_or_else(|| ControlError::runtime("qualityprofile not array"))?;
    let live = items
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(profile_name))
        .cloned()
        .ok_or_else(|| ControlError::policy(format!("quality profile `{profile_name}` missing")))?;
    let id = live
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ControlError::runtime("quality profile missing id"))?;
    let mut format_items = Vec::new();
    for pack in packs {
        for (name, score) in &pack.scores {
            let format = cfs
                .iter()
                .find(|cf| cf.get("name").and_then(Value::as_str) == Some(name.as_str()))
                .and_then(|cf| cf.get("id").and_then(Value::as_i64))
                .ok_or_else(|| ControlError::runtime(format!("cf `{name}` missing id")))?;
            format_items.push(json!({"format": format, "name": name, "score": score}));
        }
    }
    let mut body = live.clone();
    body["formatItems"] = json!(format_items);
    if body == live {
        return Ok(Vec::new());
    }
    client
        .put_json(&format!("qualityprofile/{id}"), &body)
        .await
        .map_err(arr_to_control)?;
    Ok(vec![format!("~quality profile {profile_name} scores")])
}

async fn apply_policy<T: HttpTransport>(
    client: &ArrClient<T>,
    desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let policy = &desired.grab().policy;
    if policy.delay_minutes.is_none() {
        return Ok(Vec::new());
    }
    let mut diffs = Vec::new();
    if let Some(minutes) = policy.delay_minutes {
        let live = client.delay_profiles().await.map_err(arr_to_control)?;
        let items = live
            .as_array()
            .ok_or_else(|| ControlError::runtime("delayprofile not array"))?;
        if items.is_empty() {
            return Err(ControlError::runtime("no delay profile to apply"));
        }
        for profile in items {
            let id = profile
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| ControlError::runtime("delay profile missing id"))?;
            let mut body = profile.clone();
            body["usenetDelay"] = json!(minutes);
            body["torrentDelay"] = json!(minutes);
            if body.get("delay").is_some() {
                body["delay"] = json!(minutes);
            }
            if body != *profile {
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

fn indexer_resource(spec: &GrabIndexer, live: Option<&Value>) -> Value {
    let mut body = live.cloned().unwrap_or_else(|| json!({}));
    body["name"] = json!(spec.name);
    body["priority"] = json!(spec.priority);
    body["enable"] = json!(spec.enable);
    body["implementation"] = json!(spec.implementation);
    if let Some(protocol) = &spec.protocol {
        body["protocol"] = json!(protocol);
    }
    if let Some(contract) = &spec.config_contract {
        body["configContract"] = json!(contract);
    }
    merge_fields(&mut body, &spec.fields);
    body
}

fn client_resource(spec: &GrabDownloadClient, live: Option<&Value>) -> Value {
    let mut body = live.cloned().unwrap_or_else(|| json!({}));
    body["name"] = json!(spec.name);
    body["priority"] = json!(spec.priority);
    body["enable"] = json!(spec.enable);
    body["implementation"] = json!(spec.implementation_name());
    merge_fields(&mut body, &spec.fields);
    body
}

fn merge_fields(body: &mut Value, desired: &std::collections::BTreeMap<String, String>) {
    if desired.is_empty() {
        return;
    }
    let mut fields = body
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (name, value) in desired {
        if let Some(existing) = fields
            .iter_mut()
            .find(|f| f.get("name").and_then(Value::as_str) == Some(name.as_str()))
        {
            existing["value"] = json!(value);
        } else {
            fields.push(json!({"name": name, "value": value}));
        }
    }
    body["fields"] = json!(fields);
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
    } else if ops.url_bases.contains_key("sonarr") {
        drift.push("sonarr key missing; host unchecked".into());
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
    } else if ops.url_bases.contains_key("radarr") {
        drift.push("radarr key missing; host unchecked".into());
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
    } else if ops.url_bases.contains_key("lidarr") {
        drift.push("lidarr key missing; host unchecked".into());
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
            for app in arr {
                let url = app
                    .get("baseUrl")
                    .or_else(|| app.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let name = app
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let base = app_url_base(ops, &name);
                if !Prowlarr::<T>::application_url_ok(url, base) {
                    drift.push(format!("Prowlarr app URL `{url}` missing {base}"));
                }
            }
        } else {
            drift.push("prowlarr applications missing or not an array".into());
        }
    } else if ops.url_bases.contains_key("prowlarr") {
        drift.push("prowlarr key missing; host unchecked".into());
    }
    Ok(drift)
}

fn app_url_base<'a>(ops: &'a LocalhostGrabOps<impl HttpTransport>, name: &str) -> &'a str {
    ops.url_bases
        .get(name)
        .map(String::as_str)
        .unwrap_or_else(|| match name {
            "sonarr" => "/sonarr",
            "radarr" => "/radarr",
            "lidarr" => "/lidarr",
            "prowlarr" => "/prowlarr",
            _ => "/sonarr",
        })
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
    if matches!(host.bind_address.as_str(), "*" | "0.0.0.0" | "::" | "::0") {
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
    desired: &DesiredState,
) -> Result<Vec<String>, ControlError> {
    let bind = desired
        .edge()
        .map(|e| e.bind.as_str())
        .unwrap_or(ops.bind.as_str());
    let auth = desired
        .edge()
        .map(|e| e.auth.as_str())
        .unwrap_or(ops.auth.as_str());
    if matches!(bind, "*" | "0.0.0.0" | "::" | "::0") {
        return Err(ControlError::policy("refusing bind-to-star"));
    }
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
        let url_base = desired
            .edge()
            .and_then(|e| e.url_bases.get(app))
            .map(String::as_str)
            .unwrap_or_else(|| match app {
                "sonarr" => "/sonarr",
                "radarr" => "/radarr",
                "lidarr" => "/lidarr",
                _ => "/prowlarr",
            });
        let live = client
            .get_json("config/host")
            .await
            .map_err(arr_to_control)?;
        let mut body = live.clone();
        body["bindAddress"] = json!(bind);
        body["urlBase"] = json!(url_base);
        body["authenticationMethod"] = json!(auth);
        if live != body {
            let old = serde_json::to_string_pretty(&live).unwrap_or_default();
            let new = serde_json::to_string_pretty(&body).unwrap_or_default();
            diffs.push(unified_diff(&old, &new, &format!("{app}/config/host")));
            client
                .put_host_config(&body)
                .await
                .map_err(arr_to_control)?;
        }
        if app == "prowlarr" {
            diffs.extend(apply_prowlarr_app_urls(&client, desired, ops).await?);
        }
    }
    Ok(diffs)
}

async fn apply_prowlarr_app_urls<T: HttpTransport>(
    client: &ArrClient<T>,
    desired: &DesiredState,
    ops: &LocalhostGrabOps<T>,
) -> Result<Vec<String>, ControlError> {
    let apps = client
        .get_json("applications")
        .await
        .map_err(arr_to_control)?;
    let Some(arr) = apps.as_array() else {
        return Ok(Vec::new());
    };
    let mut diffs = Vec::new();
    for app in arr {
        let url = app
            .get("baseUrl")
            .or_else(|| app.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let name = app
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let base = desired
            .edge()
            .and_then(|e| e.url_bases.get(&name))
            .map(String::as_str)
            .unwrap_or_else(|| app_url_base(ops, &name));
        if Prowlarr::<T>::application_url_ok(url, base) {
            continue;
        }
        let id = app
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| ControlError::runtime("prowlarr application missing id"))?;
        let mut body = app.clone();
        let port = match name.as_str() {
            "sonarr" => 8989,
            "radarr" => 7878,
            "lidarr" => 8686,
            _ => 9696,
        };
        body["baseUrl"] = json!(format!("http://127.0.0.1:{port}{base}"));
        client
            .put_json(&format!("applications/{id}"), &body)
            .await
            .map_err(arr_to_control)?;
        diffs.push(format!("~prowlarr app {name} url_base"));
    }
    Ok(diffs)
}

fn arr_to_control(err: ArrError) -> ControlError {
    match err {
        ArrError::MaskedKey => ControlError::policy("masked API key refused"),
        ArrError::DuplicateIndexer(name) => {
            ControlError::policy(format!("duplicate indexer `{name}`"))
        }
        ArrError::DuplicateDownloadClient(name) => {
            ControlError::policy(format!("duplicate download client `{name}`"))
        }
        other => ControlError::runtime(other.to_string()),
    }
}

fn key_to_control(err: crate::keys::KeyError) -> ControlError {
    match err {
        crate::keys::KeyError::MaskedKey => ControlError::policy("masked API key refused"),
        crate::keys::KeyError::EmptyKey => ControlError::policy("empty API key refused"),
        crate::keys::KeyError::Io(msg) => ControlError::runtime(msg),
    }
}

/// Map a Servarr queue JSON document to live hold items. Only `arr` parses queue JSON.
///
/// Include when `trackedDownloadState` is `importBlocked` (ci). Skip missing TitleId
/// or `release_id` without error. Path fields are ignored so blocked NZBs never look
/// like library.
pub fn hold_items_from_queue(queue: &Value) -> Vec<HoldLiveItem> {
    let records = queue
        .get("records")
        .and_then(Value::as_array)
        .or_else(|| queue.as_array());
    let Some(records) = records else {
        return Vec::new();
    };
    records.iter().filter_map(hold_item_from_record).collect()
}

fn hold_item_from_record(item: &Value) -> Option<HoldLiveItem> {
    let state = item.get("trackedDownloadState").and_then(Value::as_str)?;
    if !state.eq_ignore_ascii_case("importBlocked") {
        return None;
    }
    let title_id = title_id_from_queue_item(item)?;
    let release_id = release_id_from_queue_item(item)?;
    Some(HoldLiveItem {
        key: HoldKey::new(title_id, release_id),
        added_unix: item
            .get("added")
            .and_then(parse_added)
            .unwrap_or_else(current_unix),
        size: item.get("size").and_then(json_u64).unwrap_or(0),
        reason: reason_from_queue_item(item),
    })
}

fn title_id_from_queue_item(item: &Value) -> Option<TitleId> {
    if let Some(id) = item
        .get("movie")
        .and_then(|m| m.get("tmdbId"))
        .and_then(json_id)
    {
        return TitleId::movie(id).ok();
    }
    if let Some(id) = item
        .get("series")
        .and_then(|s| s.get("tvdbId"))
        .and_then(json_id)
    {
        return TitleId::series(id).ok();
    }
    if let Some(id) = item
        .get("album")
        .and_then(|a| a.get("foreignAlbumId"))
        .and_then(Value::as_str)
    {
        return TitleId::album(id).ok();
    }
    None
}

fn release_id_from_queue_item(item: &Value) -> Option<ReleaseId> {
    let protocol = item.get("protocol").and_then(Value::as_str)?;
    if protocol.eq_ignore_ascii_case("torrent") {
        let download_id = item.get("downloadId").and_then(Value::as_str)?;
        ReleaseId::torrent(download_id).ok()
    } else if protocol.eq_ignore_ascii_case("usenet") {
        let title = item.get("title").and_then(Value::as_str)?;
        ReleaseId::usenet(title).ok()
    } else {
        None
    }
}

fn reason_from_queue_item(item: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(msgs) = item.get("statusMessages").and_then(Value::as_array) {
        for msg in msgs {
            if let Some(arr) = msg.get("messages").and_then(Value::as_array) {
                for line in arr {
                    if let Some(s) = line.as_str()
                        && !s.is_empty()
                    {
                        parts.push(s.to_string());
                    }
                }
            }
        }
    }
    let joined = parts.join("; ");
    if !joined.is_empty() {
        return joined;
    }
    item.get("errorMessage")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn json_id(value: &Value) -> Option<String> {
    if let Some(n) = value.as_u64() {
        return Some(n.to_string());
    }
    if let Some(n) = value.as_i64() {
        if n < 0 {
            return None;
        }
        return Some(n.to_string());
    }
    value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| {
            value.as_f64().and_then(|n| {
                if n.is_finite() && n >= 0.0 {
                    Some(n as u64)
                } else {
                    None
                }
            })
        })
}

fn current_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_added(value: &Value) -> Option<i64> {
    parse_rfc3339(value.as_str()?)
}

fn parse_rfc3339(raw: &str) -> Option<i64> {
    let s = raw.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let year: i32 = s.get(0..4)?.parse().ok()?;
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') {
        return None;
    }
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let min: u32 = s.get(14..16)?.parse().ok()?;
    let sec: u32 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let mut idx = 19;
    if b.get(idx) == Some(&b'.') {
        idx += 1;
        while idx < b.len() && b[idx].is_ascii_digit() {
            idx += 1;
        }
    }
    let rest = s.get(idx..)?;
    let offset_secs: i64 = if rest.is_empty() || rest.eq_ignore_ascii_case("Z") {
        0
    } else if rest.starts_with('+') || rest.starts_with('-') {
        let sign = if rest.starts_with('+') { 1 } else { -1 };
        let rest = &rest[1..];
        let (hh, mm) = if rest.len() >= 5 && rest.as_bytes().get(2) == Some(&b':') {
            (rest.get(0..2)?, rest.get(3..5)?)
        } else if rest.len() >= 4 {
            (rest.get(0..2)?, rest.get(2..4)?)
        } else {
            return None;
        };
        let hh: i64 = hh.parse().ok()?;
        let mm: i64 = mm.parse().ok()?;
        sign * (hh * 3600 + mm * 60)
    } else {
        return None;
    };
    let days = days_from_civil(year, month, day)?;
    Some(days * 86400 + i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec) - offset_secs)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = i64::from(y);
    let m = i64::from(m);
    let d = i64::from(d);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    Some(era * 146097 + doe as i64 - 719468)
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
implementation = "Newznab"

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
            "/sonarr/api/v3/indexer/1",
            None,
            json_ok(json!({"id":1,"name":"NZBgeek","priority":25,"enable":true,"implementation":"Newznab"})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([{"id":1,"name":"SABnzbd","priority":1}])),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient/1",
            None,
            json_ok(json!({"id":1,"name":"SABnzbd","priority":1,"enable":true,"implementation":"Sabnzbd"})),
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
            json_ok(json!([{"id":1,"usenetDelay":0,"torrentDelay":0}])),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/delayprofile/1",
            None,
            json_ok(json!({"id":1,"usenetDelay":5,"torrentDelay":5})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/config/host",
            None,
            json_ok(json!({
                "id": 1,
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
            "/sonarr/api/v3/indexer/1",
            None,
            json_ok(json!({"id":1,"name":"NZBgeek","priority":25,"enable":true,"implementation":"Newznab"})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([{"id":1,"name":"SABnzbd","priority":1}])),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient/1",
            None,
            json_ok(json!({"id":1,"name":"SABnzbd","priority":1,"enable":true,"implementation":"Sabnzbd"})),
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

    #[tokio::test]
    async fn missing_sonarr_key_is_policy_not_noop() {
        let home = scratch("no-key");
        let t = CassetteTransport::new();
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let err = ops.grab_apply(&ds_servarr()).await.expect_err("key");
        assert_eq!(err.exit_code, mediaops_core::ExitCode::PolicyRefusal);
        assert!(err.message.contains("sonarr"), "{}", err.message);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn extra_client_is_deleted_and_priority_updates() {
        let home = scratch("client-diff");
        write_sonarr_key(&home, "k");
        let mut t = CassetteTransport::new();
        t.push(
            "GET",
            "/sonarr/api/v3/indexer",
            None,
            json_ok(json!([{"id":1,"name":"NZBgeek","priority":25}])),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/indexer/1",
            None,
            json_ok(json!({"id":1,"name":"NZBgeek","priority":25,"enable":true,"implementation":"Newznab"})),
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient",
            None,
            json_ok(json!([
                {"id":1,"name":"SABnzbd","priority":9,"implementation":"Sabnzbd"},
                {"id":2,"name":"Extra","priority":1}
            ])),
        );
        t.push(
            "DELETE",
            "/sonarr/api/v3/downloadclient/2",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        t.push(
            "GET",
            "/sonarr/api/v3/downloadclient/1",
            None,
            json_ok(json!({"id":1,"name":"SABnzbd","priority":9,"enable":true,"implementation":"Sabnzbd"})),
        );
        t.push(
            "PUT",
            "/sonarr/api/v3/downloadclient/1",
            None,
            json_ok(json!({"id":1,"name":"SABnzbd","priority":1})),
        );
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let report = ops.grab_apply(&ds_servarr()).await.expect("apply");
        assert!(report.diff.contains("-client Extra"), "{}", report.diff);
        assert!(report.diff.contains("~client SABnzbd"), "{}", report.diff);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn bind_to_star_includes_ipv6_any() {
        let star = HostConfig {
            bind_address: "::".into(),
            url_base: "/sonarr".into(),
            authentication_method: "forms".into(),
        };
        assert!(
            host_config_drift(&star, "/sonarr", "127.0.0.1", "forms", "sonarr")
                .iter()
                .any(|d| d.contains("bind-to-star"))
        );
    }

    #[tokio::test]
    async fn edge_api_check_flags_prowlarr_app_url_without_sonarr_base() {
        let home = scratch("prowlarr-url");
        write_sonarr_key(&home, "k");
        let prowlarr = home.join(".config/Prowlarr/config.xml");
        fs::create_dir_all(prowlarr.parent().expect("p")).expect("mkdir");
        fs::write(&prowlarr, "<Config><ApiKey>k</ApiKey></Config>").expect("xml");
        let mut t = CassetteTransport::new();
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
        t.push(
            "GET",
            "/prowlarr/api/v1/config/host",
            None,
            json_ok(json!({
                "bindAddress": "127.0.0.1",
                "urlBase": "/prowlarr",
                "authenticationMethod": "forms"
            })),
        );
        t.push(
            "GET",
            "/prowlarr/api/v1/applications",
            None,
            json_ok(json!([{"id":1,"name":"Sonarr","baseUrl":"http://127.0.0.1:8989/1/"}])),
        );
        let toml = r#"
schema_version = 1
max_copy_gib = 1
min_free_gib = 0
range_len_mib = 1
max_nvenc = 1
lock = false
grabber = "servarr"
[edge]
url_bases = { sonarr = "/sonarr", prowlarr = "/prowlarr" }
bind = "127.0.0.1"
auth = "forms"
"#;
        let ds = DesiredState::from_toml(toml).expect("ds");
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds);
        let report = ops.edge_api_check().await.expect("check");
        assert!(!report.invariant_ok);
        assert!(report.drift.contains("missing /sonarr"), "{}", report.drift);
        let _ = fs::remove_dir_all(home);
    }

    fn import_blocked_queue() -> Value {
        json!({
            "records": [
                {
                    "title": "The.Matrix.1999.nzb",
                    "size": 12345,
                    "protocol": "usenet",
                    "trackedDownloadState": "importBlocked",
                    "added": "2020-01-01T00:00:00Z",
                    "outputPath": "/data/_incoming/The.Matrix.1999",
                    "statusMessages": [{
                        "title": "The Matrix",
                        "messages": ["No files found are eligible for import"]
                    }],
                    "movie": {"tmdbId": 603}
                },
                {
                    "title": "The.Wire.S01E01",
                    "size": 999,
                    "protocol": "torrent",
                    "downloadId": "DEADBEEF",
                    "trackedDownloadState": "importBlocked",
                    "added": "2020-01-02T00:00:00Z",
                    "statusMessages": [{
                        "messages": ["Sample file only"]
                    }],
                    "series": {"tvdbId": 79126}
                },
                {
                    "title": "Relayer",
                    "size": 50,
                    "protocol": "usenet",
                    "trackedDownloadState": "importBlocked",
                    "added": "2020-01-03T00:00:00Z",
                    "album": {"foreignAlbumId": "0f82b02e-c6cd-4242-b195-93d4bf3e0d63"}
                },
                {
                    "title": "Downloading",
                    "size": 1,
                    "protocol": "usenet",
                    "trackedDownloadState": "downloading",
                    "movie": {"tmdbId": 604}
                },
                {
                    "title": "Missing.TitleId.nzb",
                    "size": 1,
                    "protocol": "usenet",
                    "trackedDownloadState": "importBlocked"
                },
                {
                    "title": "Missing.DownloadId",
                    "size": 1,
                    "protocol": "torrent",
                    "trackedDownloadState": "importBlocked",
                    "movie": {"tmdbId": 605}
                }
            ],
            "totalRecords": 6
        })
    }

    #[test]
    fn hold_items_from_queue_maps_import_blocked_and_omits_incomplete() {
        let items = hold_items_from_queue(&import_blocked_queue());
        assert_eq!(
            items.len(),
            3,
            "missing TitleId/release_id and non-blocked omitted"
        );
        assert_eq!(items[0].key.title_id.render(), "movie:tmdb:603");
        assert_eq!(
            items[0].key.release_id,
            ReleaseId::usenet("The.Matrix.1999.nzb").expect("usenet")
        );
        assert_eq!(items[0].added_unix, 1_577_836_800);
        assert_eq!(items[0].size, 12345);
        assert_eq!(items[0].reason, "No files found are eligible for import");
        assert_eq!(items[1].key.title_id.render(), "series:tvdb:79126");
        assert_eq!(
            items[1].key.release_id,
            ReleaseId::torrent("DEADBEEF").expect("torrent")
        );
        assert_eq!(items[1].reason, "Sample file only");
        assert_eq!(
            items[2].key.title_id.render(),
            "album:mbid:0f82b02e-c6cd-4242-b195-93d4bf3e0d63"
        );
        for item in &items {
            let debug = format!("{item:?}");
            assert!(
                !debug.contains("_incoming") && !debug.contains("outputPath"),
                "blocked NZBs must not rot as a junk drawer of library paths: {debug}"
            );
        }
    }

    #[test]
    fn rfc3339_added_is_unix() {
        assert_eq!(parse_rfc3339("2020-01-01T00:00:00Z"), Some(1_577_836_800));
        assert_eq!(
            parse_rfc3339("2020-01-01T00:00:00.1234567Z"),
            Some(1_577_836_800)
        );
        assert_eq!(
            parse_rfc3339("2020-01-01T01:00:00+01:00"),
            Some(1_577_836_800)
        );
        assert_eq!(parse_rfc3339("2020-01-01T00:00:00"), Some(1_577_836_800));
        assert!(parse_rfc3339("not-a-date").is_none());
    }

    fn one_blocked(extra: Value) -> Value {
        let mut rec = json!({
            "title": "X.nzb",
            "protocol": "usenet",
            "trackedDownloadState": "importBlocked",
            "movie": {"tmdbId": 1}
        });
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                rec[k] = v.clone();
            }
        }
        json!({"records": [rec], "totalRecords": 1})
    }

    #[test]
    fn missing_or_unparsable_added_yields_near_zero_age() {
        for extra in [json!({}), json!({"added": "not-a-date"})] {
            let items = hold_items_from_queue(&one_blocked(extra));
            assert_eq!(items.len(), 1);
            let now = current_unix();
            assert!(
                items[0].age_secs(now) <= 2,
                "age_secs={} added_unix={}",
                items[0].age_secs(now),
                items[0].added_unix
            );
        }
    }

    #[test]
    fn float_size_truncates_toward_zero() {
        let items = hold_items_from_queue(&one_blocked(json!({"size": 12345.9})));
        assert_eq!(items[0].size, 12345);
    }

    #[test]
    fn empty_status_messages_use_error_message() {
        let items = hold_items_from_queue(&one_blocked(json!({
            "statusMessages": [],
            "errorMessage": "Download client timed out"
        })));
        assert_eq!(items[0].reason, "Download client timed out");
        let items = hold_items_from_queue(&one_blocked(json!({
            "statusMessages": [{"messages": ["Eligible mismatch"]}],
            "errorMessage": "ignored"
        })));
        assert_eq!(items[0].reason, "Eligible mismatch");
    }

    fn write_arr_key(home: &std::path::Path, app: &str, key: &str) {
        let path = home.join(format!(".config/{app}/config.xml"));
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, format!("<Config><ApiKey>{key}</ApiKey></Config>")).expect("xml");
    }

    #[tokio::test]
    async fn grab_ops_hold_list_pages_sonarr_radarr_lidarr_cassette() {
        let home = scratch("hold-list");
        write_arr_key(&home, "Sonarr", "k");
        write_arr_key(&home, "Radarr", "k");
        write_arr_key(&home, "Lidarr", "k");
        let mut t = CassetteTransport::new();
        t.push_json(include_str!(
            "../../../fixtures/arr/import_blocked_queue.json"
        ))
        .expect("sonarr cassette");
        t.push_json(include_str!(
            "../../../fixtures/arr/import_blocked_radarr_queue.json"
        ))
        .expect("radarr cassette");
        t.push_json(include_str!(
            "../../../fixtures/arr/import_blocked_lidarr_queue.json"
        ))
        .expect("lidarr cassette");
        let ops = LocalhostGrabOps::new(t, KeyPaths::from_home(&home), &ds_servarr());
        let items = ops.hold_list().await.expect("hold list");
        let ids: Vec<String> = items.iter().map(|i| i.key.title_id.render()).collect();
        assert!(
            ids.contains(&"movie:tmdb:603".into()),
            "sonarr movie missing: {ids:?}"
        );
        assert!(
            ids.contains(&"series:tvdb:79126".into()),
            "sonarr series missing: {ids:?}"
        );
        assert!(
            ids.contains(&"movie:tmdb:27205".into()),
            "radarr movie missing: {ids:?}"
        );
        assert!(
            ids.contains(&"album:mbid:12345678-1234-1234-1234-123456789012".into()),
            "lidarr album missing: {ids:?}"
        );
        assert_eq!(items.len(), 4);
        let _ = fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn grab_ops_hold_list_without_keys_is_empty() {
        let home = scratch("hold-empty");
        let ops = LocalhostGrabOps::new(
            CassetteTransport::new(),
            KeyPaths::from_home(&home),
            &ds_servarr(),
        );
        let items = ops.hold_list().await.expect("empty");
        assert!(items.is_empty());
        let _ = fs::remove_dir_all(home);
    }
}
