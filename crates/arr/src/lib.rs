//! Grabber HTTP behind [`HttpTransport`] (AD-15). Linked only into `mediaopsd`.

mod apply;
mod cassette;
mod keys;
mod lidarr;
mod prowlarr;
mod qbit;
mod radarr;
mod reqwest_impl;
mod sab;
mod servarr;
mod sonarr;
mod transport;

pub use apply::{LocalhostGrabOps, hold_items_from_queue, host_config_drift};
pub use cassette::{CassetteTransport, cassette_body_digest, cassette_key};
pub use keys::{
    DiscoveredKeys, KeyError, KeyPaths, discover_keys, discover_sab_key, discover_servarr_key,
    is_masked_key, refuse_key, refuse_masked,
};
pub use lidarr::Lidarr;
pub use prowlarr::Prowlarr;
pub use qbit::{QbitClient, QbitPreferences, parse_torrents_info};
pub use radarr::Radarr;
pub use reqwest_impl::ReqwestTransport;
pub use sab::{SAB_CATEGORIES, SabClient};
pub use servarr::{
    ArrClient, ArrError, DownloadClientIdentity, HostConfig, IndexerIdentity, parse_host_config,
};
pub use sonarr::Sonarr;
pub use transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn workspace_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace")
    }

    #[tokio::test]
    async fn workspace_cassettes_replay_named_http_failures() {
        let transport = CassetteTransport::from_workspace_fixtures().expect("load fixtures/arr");
        let masked = transport
            .send(&HttpRequest {
                method: "POST".into(),
                url: "http://127.0.0.1:8989/sonarr/api/v3/indexer/test".into(),
                headers: vec![("X-Api-Key".into(), "********".into())],
                body: Some(
                    serde_json::to_vec(&serde_json::json!({
                        "name": "NZBgeek",
                        "apiKey": "********"
                    }))
                    .expect("body"),
                ),
            })
            .await
            .expect("masked cassette");
        assert_eq!(masked.status, 401);

        let dup = transport
            .send(&HttpRequest {
                method: "POST".into(),
                url: "http://127.0.0.1:8989/sonarr/api/v3/indexer".into(),
                headers: Vec::new(),
                body: Some(
                    serde_json::to_vec(&serde_json::json!({
                        "name": "NZBgeek",
                        "priority": 25
                    }))
                    .expect("body"),
                ),
            })
            .await
            .expect("duplicate cassette");
        assert_eq!(dup.status, 409);
    }

    #[tokio::test]
    async fn every_named_arr_resource_has_a_cassette_hit() {
        let mut t = CassetteTransport::new();
        let resources = [
            "config/host",
            "config/ui",
            "config/naming",
            "config/mediamanagement",
            "qualityprofile",
            "qualitydefinition",
            "customformat",
            "delayprofile",
            "indexer",
            "downloadclient",
            "importlist",
            "rootfolder",
            "tag",
            "notification",
            "system/status",
            "health",
            "diskspace",
            "queue?page=1&pageSize=200",
            "history?page=1&pageSize=200",
            "blocklist?page=1&pageSize=200",
            "wanted/missing?page=1&pageSize=200",
            "wanted/cutoff?page=1&pageSize=200",
            "calendar",
            "command",
            "system/backup",
            "filesystem?path=%2Fdata",
            "manualimport",
            "release?term=matrix",
        ];
        for resource in resources {
            t.push(
                "GET",
                &format!("/sonarr/api/v3/{resource}"),
                None,
                HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: b"[]".to_vec(),
                },
            );
        }
        t.push(
            "POST",
            "/sonarr/api/v3/command",
            None,
            HttpResponse {
                status: 201,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "POST",
            "/sonarr/api/v3/release",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "POST",
            "/sonarr/api/v3/manualimport",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            },
        );
        t.push(
            "DELETE",
            "/sonarr/api/v3/indexer/1",
            None,
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: Vec::new(),
            },
        );
        let client = ArrClient::new(t, "http://127.0.0.1:8989/sonarr", "/api/v3", "k").expect("c");
        for call in [
            client.ui_config().await,
            client.naming().await,
            client.media_management().await,
            client.quality_profiles().await,
            client.quality_definitions().await,
            client.custom_formats().await,
            client.delay_profiles().await,
            client.import_lists().await,
            client.root_folders().await,
            client.tags().await,
            client.notifications().await,
            client.system_status().await,
            client.health().await,
            client.diskspace().await,
            client.queue().await,
            client.history().await,
            client.blocklist().await,
            client.wanted_missing().await,
            client.wanted_cutoff().await,
            client.calendar().await,
            client.commands().await,
            client.backups().await,
            client.filesystem("/data").await,
            client.manual_import().await,
            client.release_search("matrix").await,
        ] {
            call.expect("resource cassette");
        }
        client
            .command(&serde_json::json!({"name": "RefreshMonitoredDownloads"}))
            .await
            .expect("command");
        client
            .grab_release(&serde_json::json!({"guid": "x"}))
            .await
            .expect("grab");
        client
            .post_manual_import(&serde_json::json!([]))
            .await
            .expect("import");
        client.delete_indexer(1).await.expect("delete");
    }

    #[test]
    fn forbidden_grabber_stubs_are_absent() {
        let root = workspace_root();
        let mut hits = Vec::new();
        for tree in ["crates", "bins"] {
            walk_forbidden(&root.join(tree), &root, &mut hits);
        }
        assert!(
            hits.is_empty(),
            "{} / {} must not exist, including stubs: {hits:?}",
            concat!("Auto", "brr"),
            concat!("Baz", "arr"),
        );
    }

    fn walk_forbidden(dir: &Path, workspace: &Path, hits: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                    continue;
                }
                walk_forbidden(&path, workspace, hits);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            // Needles split so this file does not match itself.
            for needle in [concat!("Auto", "brr"), concat!("Baz", "arr")] {
                if source
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
                {
                    let rel = path.strip_prefix(workspace).unwrap_or(&path);
                    hits.push(format!("{}: {needle}", rel.display()));
                }
            }
        }
    }

    #[test]
    fn cassette_key_is_method_path_digest() {
        let req = HttpRequest {
            method: "GET".into(),
            url: "http://127.0.0.1:8989/sonarr/api/v3/indexer".into(),
            headers: Vec::new(),
            body: None,
        };
        let key = cassette_key(&req);
        assert!(key.starts_with("GET /sonarr/api/v3/indexer "));
        assert_eq!(key.split_whitespace().count(), 3);
    }
}
