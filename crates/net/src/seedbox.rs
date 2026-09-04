//! Seedbox Control + Transfer over the walker.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use mediaops_core::{
    Allowlist, ControlError, DeleteRemoteOutcome, DesiredState, ExitCode, GrabOps, Grabber,
    GuardPreviewItem, HoldKey, HoldLiveItem, RemoteRef, TitleId, WalkerError, nginx_host_ok,
    panel_fingerprint,
};
use mediaops_proto::control_server::Control;
use mediaops_proto::transfer_server::Transfer;
use mediaops_proto::{
    DeleteRemoteRequest, DeleteRemoteResponse, DeleteRemoteResult, DfRequest, DfResponse,
    EdgeApplyRequest, EdgeApplyResponse, EdgeCheckRequest, EdgeCheckResponse, ErrorDetail,
    GetRangeRequest, GetRangeResponse, GrabApplyRequest, GrabApplyResponse, GuardPreviewRequest,
    GuardPreviewResponse, HoldListRequest, HoldListResponse, HoldLiveItem as WireHold,
    HoldRejectRequest, HoldRejectResponse, KeyDiscoveryRequest, KeyDiscoveryResponse, ListRequest,
    ListResponse, PROTO_PACKAGE, RemoteEntry as WireEntry, StatRequest, StatResponse,
    UnmonitorRequest, UnmonitorResponse, WantedMissingRequest, WantedMissingResponse,
    status_from_error_detail,
};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

#[derive(Clone)]
pub struct Seedbox {
    allowlist: Allowlist,
    semver: String,
    grabber: Grabber,
    grab_ops: Option<Arc<dyn GrabOps>>,
    nginx_dir: Option<PathBuf>,
}

impl Seedbox {
    pub fn new(allowlist: Allowlist, semver: impl Into<String>, grabber: Grabber) -> Self {
        Self {
            allowlist,
            semver: semver.into(),
            grabber,
            grab_ops: None,
            nginx_dir: None,
        }
    }

    pub fn with_grab_ops(mut self, grab_ops: Option<Arc<dyn GrabOps>>) -> Self {
        self.grab_ops = grab_ops;
        self
    }

    pub fn with_nginx_dir(mut self, nginx_dir: PathBuf) -> Self {
        self.nginx_dir = Some(nginx_dir);
        self
    }

    fn handshake(&self) -> (String, String) {
        (self.semver.clone(), PROTO_PACKAGE.to_string())
    }
}

fn status_from_walker(err: WalkerError) -> Status {
    status_from_error_detail(&ErrorDetail::from(ControlError {
        exit_code: ExitCode::Runtime,
        message: err.to_string(),
    }))
}

fn status_from_convert(err: mediaops_proto::ConvertError) -> Status {
    status_from_error_detail(&ErrorDetail::from(ControlError {
        exit_code: ExitCode::Runtime,
        message: err.to_string(),
    }))
}

fn status_from_join(err: tokio::task::JoinError) -> Status {
    status_from_error_detail(&ErrorDetail::from(ControlError {
        exit_code: ExitCode::Runtime,
        message: err.to_string(),
    }))
}

fn nginx_fingerprint(dir: &std::path::Path) -> Result<(String, Vec<String>), String> {
    let mut files = Vec::new();
    let mut drift = Vec::new();
    let reader = std::fs::read_dir(dir).map_err(|err| err.to_string())?;
    for entry in reader {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("conf") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app.conf")
            .to_string();
        let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        if !nginx_host_ok(&text) {
            drift.push(format!("panel Host rewrite in {name}"));
        }
        files.push((name, bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let refs: Vec<(String, &[u8])> = files
        .iter()
        .map(|(n, b)| (n.clone(), b.as_slice()))
        .collect();
    Ok((panel_fingerprint(&refs), drift))
}

fn unused(name: &str) -> Status {
    status_from_error_detail(&ErrorDetail::from(ControlError {
        exit_code: ExitCode::Runtime,
        message: format!("{name} is not implemented in the seedbox role this epic"),
    }))
}

#[tonic::async_trait]
impl Control for Seedbox {
    async fn df(&self, _request: Request<DfRequest>) -> Result<Response<DfResponse>, Status> {
        let allowlist = self.allowlist.clone();
        let free = tokio::task::spawn_blocking(move || allowlist.free_bytes())
            .await
            .map_err(status_from_join)?
            .map_err(status_from_walker)?;
        let (semver, proto_package) = self.handshake();
        Ok(Response::new(DfResponse {
            semver,
            proto_package,
            free_bytes: free,
        }))
    }

    async fn unmonitor(
        &self,
        request: Request<UnmonitorRequest>,
    ) -> Result<Response<UnmonitorResponse>, Status> {
        if self.grabber == Grabber::None {
            return Err(status_from_error_detail(&ErrorDetail::from(
                ControlError::usage("grabber is none; unmonitor requires a grabber"),
            )));
        }
        let title_id = TitleId::try_from(request.into_inner()).map_err(status_from_convert)?;
        if let Some(ops) = &self.grab_ops {
            ops.unmonitor(&title_id)
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?;
        } else {
            return Err(status_from_error_detail(&ErrorDetail::from(
                ControlError::usage(
                    "grabber ops were not injected; unmonitor requires grabber ops",
                ),
            )));
        }
        let (semver, proto_package) = self.handshake();
        Ok(Response::new(UnmonitorResponse {
            semver,
            proto_package,
        }))
    }

    async fn delete_remote(
        &self,
        request: Request<DeleteRemoteRequest>,
    ) -> Result<Response<DeleteRemoteResponse>, Status> {
        let remote = RemoteRef::try_from(request.into_inner()).map_err(status_from_convert)?;
        let torrents = match qbit_or_skip(self.grab_ops.as_deref()).await {
            Ok(t) => t,
            Err(_) => {
                let (semver, proto_package) = self.handshake();
                return Ok(Response::new(DeleteRemoteResponse {
                    semver,
                    proto_package,
                    result: DeleteRemoteResult::SkippedSeeding as i32,
                }));
            }
        };
        let allowlist = self.allowlist.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            delete_remote_locked(&allowlist, &remote, &torrents)
        })
        .await
        .map_err(status_from_join)?
        .map_err(status_from_walker)?;
        let (semver, proto_package) = self.handshake();
        Ok(Response::new(DeleteRemoteResponse {
            semver,
            proto_package,
            result: match outcome {
                DeleteRemoteOutcome::Deleted => DeleteRemoteResult::Deleted as i32,
                DeleteRemoteOutcome::SkippedSeeding => DeleteRemoteResult::SkippedSeeding as i32,
            },
        }))
    }

    async fn grab_apply(
        &self,
        request: Request<GrabApplyRequest>,
    ) -> Result<Response<GrabApplyResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        if self.grabber == Grabber::None {
            return Ok(Response::new(GrabApplyResponse {
                semver,
                proto_package,
                noop: true,
                diff: String::new(),
            }));
        }
        if self.grab_ops.is_none() {
            return Err(unused("GrabApply"));
        }
        let toml = request.into_inner().desired_state_toml;
        let desired = DesiredState::from_toml_bytes(&toml).map_err(|err| {
            status_from_error_detail(&ErrorDetail::from(ControlError::runtime(err.to_string())))
        })?;
        let report = self
            .grab_ops
            .as_ref()
            .expect("checked")
            .grab_apply(&desired)
            .await
            .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?;
        Ok(Response::new(GrabApplyResponse {
            semver,
            proto_package,
            noop: report.noop,
            diff: report.diff,
        }))
    }

    async fn edge_check(
        &self,
        _request: Request<EdgeCheckRequest>,
    ) -> Result<Response<EdgeCheckResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        let (fingerprint, mut drift) = match &self.nginx_dir {
            Some(dir) => nginx_fingerprint(dir).map_err(|err| {
                status_from_error_detail(&ErrorDetail::from(ControlError::runtime(err)))
            })?,
            None => {
                return Err(status_from_error_detail(&ErrorDetail::from(
                    ControlError::runtime("nginx_dir required for panel fingerprint"),
                )));
            }
        };
        if self.grabber == Grabber::Servarr
            && let Some(ops) = &self.grab_ops
        {
            let api = ops
                .edge_api_check()
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?;
            if !api.drift.is_empty() {
                drift.push(api.drift);
            }
        }
        let drift = drift.join("; ");
        Ok(Response::new(EdgeCheckResponse {
            semver,
            proto_package,
            fingerprint,
            invariant_ok: drift.is_empty(),
            drift,
        }))
    }

    async fn edge_apply(
        &self,
        request: Request<EdgeApplyRequest>,
    ) -> Result<Response<EdgeApplyResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        if self.grabber != Grabber::Servarr || self.grab_ops.is_none() {
            return Ok(Response::new(EdgeApplyResponse {
                semver,
                proto_package,
                noop: true,
                diff: String::new(),
            }));
        }
        let toml = request.into_inner().desired_state_toml;
        let desired = DesiredState::from_toml_bytes(&toml).map_err(|err| {
            status_from_error_detail(&ErrorDetail::from(ControlError::runtime(err.to_string())))
        })?;
        let report = self
            .grab_ops
            .as_ref()
            .expect("checked")
            .edge_apply(&desired)
            .await
            .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?;
        Ok(Response::new(EdgeApplyResponse {
            semver,
            proto_package,
            noop: report.noop,
            diff: report.diff,
        }))
    }

    async fn key_discovery(
        &self,
        _request: Request<KeyDiscoveryRequest>,
    ) -> Result<Response<KeyDiscoveryResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        let presence = if let Some(ops) = &self.grab_ops {
            ops.key_discovery()
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?
        } else {
            mediaops_core::KeyPresence::default()
        };
        Ok(Response::new(KeyDiscoveryResponse {
            semver,
            proto_package,
            sonarr_key_present: presence.sonarr_key_present,
            radarr_key_present: presence.radarr_key_present,
            lidarr_key_present: presence.lidarr_key_present,
            prowlarr_key_present: presence.prowlarr_key_present,
            sab_key_present: presence.sab_key_present,
            qbit_key_present: presence.qbit_key_present,
        }))
    }

    async fn guard_preview(
        &self,
        _request: Request<GuardPreviewRequest>,
    ) -> Result<Response<GuardPreviewResponse>, Status> {
        let torrents = match &self.grab_ops {
            Some(ops) => ops
                .qbit_snapshot()
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?,
            None => Vec::new(),
        };
        let allowlist = self.allowlist.clone();
        let items = tokio::task::spawn_blocking(move || attach_guard_remotes(&allowlist, torrents))
            .await
            .map_err(status_from_join)?;
        let (semver, proto_package) = self.handshake();
        let items = items
            .iter()
            .map(mediaops_proto::GuardPreviewItem::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(status_from_convert)?;
        Ok(Response::new(GuardPreviewResponse {
            semver,
            proto_package,
            items,
        }))
    }

    async fn hold_list(
        &self,
        _request: Request<HoldListRequest>,
    ) -> Result<Response<HoldListResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        let items = if self.grabber == Grabber::None {
            Vec::new()
        } else if let Some(ops) = &self.grab_ops {
            let mut items = ops
                .hold_list()
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?;
            let allowlist = self.allowlist.clone();
            items = tokio::task::spawn_blocking(move || {
                for item in &mut items {
                    attach_remote_from_output_path(&allowlist, item);
                }
                items
            })
            .await
            .map_err(status_from_join)?;
            items.iter().map(WireHold::from).collect()
        } else {
            Vec::new()
        };
        Ok(Response::new(HoldListResponse {
            semver,
            proto_package,
            items,
        }))
    }

    async fn wanted_missing(
        &self,
        _request: Request<WantedMissingRequest>,
    ) -> Result<Response<WantedMissingResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        let title_id = if self.grabber == Grabber::None {
            Vec::new()
        } else if let Some(ops) = &self.grab_ops {
            ops.wanted_missing()
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?
                .into_iter()
                .map(|id| id.render())
                .collect()
        } else {
            Vec::new()
        };
        Ok(Response::new(WantedMissingResponse {
            semver,
            proto_package,
            title_id,
        }))
    }

    async fn hold_reject(
        &self,
        request: Request<HoldRejectRequest>,
    ) -> Result<Response<HoldRejectResponse>, Status> {
        if self.grabber == Grabber::None {
            return Err(status_from_error_detail(&ErrorDetail::from(
                ControlError::usage("grabber is none; hold reject requires a grabber"),
            )));
        }
        let key = HoldKey::try_from(request.into_inner()).map_err(status_from_convert)?;
        if let Some(ops) = &self.grab_ops {
            ops.hold_reject(&key)
                .await
                .map_err(|err| status_from_error_detail(&ErrorDetail::from(err)))?;
        } else {
            return Err(status_from_error_detail(&ErrorDetail::from(
                ControlError::usage("grabber is none; hold reject requires a grabber"),
            )));
        }
        let (semver, proto_package) = self.handshake();
        Ok(Response::new(HoldRejectResponse {
            semver,
            proto_package,
        }))
    }
}

async fn qbit_or_skip(
    grab_ops: Option<&dyn GrabOps>,
) -> Result<Vec<GuardPreviewItem>, ControlError> {
    match grab_ops {
        Some(ops) => ops.qbit_snapshot().await,
        None => Ok(Vec::new()),
    }
}

fn delete_remote_locked(
    allowlist: &Allowlist,
    remote: &RemoteRef,
    torrents: &[GuardPreviewItem],
) -> Result<DeleteRemoteOutcome, WalkerError> {
    let path = allowlist.absolute(remote)?;
    let entry = allowlist.entry(remote)?;
    if entry.nlink() > 1 {
        return Ok(DeleteRemoteOutcome::SkippedSeeding);
    }
    if torrents
        .iter()
        .any(|t| t.covers_path(&path) && t.blocks_delete())
    {
        return Ok(DeleteRemoteOutcome::SkippedSeeding);
    }
    allowlist.unlink(remote)?;
    Ok(DeleteRemoteOutcome::Deleted)
}

fn attach_guard_remotes(
    allowlist: &Allowlist,
    mut torrents: Vec<GuardPreviewItem>,
) -> Vec<GuardPreviewItem> {
    let Ok(entries) = allowlist.list() else {
        return torrents;
    };
    for torrent in &mut torrents {
        if torrent.remote.is_some() {
            continue;
        }
        for entry in &entries {
            let Ok(path) = allowlist.absolute(entry.r#ref()) else {
                continue;
            };
            if torrent.covers_path(&path) {
                torrent.remote = Some(entry.r#ref().clone());
                break;
            }
        }
    }
    torrents
}

fn attach_remote_from_output_path(allowlist: &Allowlist, item: &mut HoldLiveItem) {
    if item.remote.is_some() {
        return;
    }
    let Some(raw) = item.output_path.as_deref() else {
        return;
    };
    let path = std::path::Path::new(raw);
    if let Ok(remote) = allowlist.resolve(path)
        && allowlist.entry(&remote).is_ok()
    {
        item.remote = Some(remote);
        return;
    }
    let Ok(entries) = allowlist.list() else {
        return;
    };
    let mut media = Vec::new();
    for entry in entries {
        let Ok(abs) = allowlist.absolute(entry.r#ref()) else {
            continue;
        };
        if !(abs == path || abs.starts_with(path)) {
            continue;
        }
        let Some(name) = abs.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_hold_media_name(name) {
            media.push(entry.r#ref().clone());
        }
    }
    if media.len() == 1 {
        item.remote = Some(media.remove(0));
    }
}

fn is_hold_media_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("sample") {
        return false;
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    matches!(
        ext,
        "mkv" | "mp4" | "m4v" | "avi" | "ts" | "flac" | "mp3" | "m4a"
    )
}

#[tonic::async_trait]
impl Transfer for Seedbox {
    async fn list(&self, _request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let allowlist = self.allowlist.clone();
        let entries = tokio::task::spawn_blocking(move || allowlist.list())
            .await
            .map_err(status_from_join)?
            .map_err(status_from_walker)?;
        let wire = entries
            .iter()
            .map(WireEntry::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(status_from_convert)?;
        Ok(Response::new(ListResponse { entries: wire }))
    }

    async fn stat(&self, request: Request<StatRequest>) -> Result<Response<StatResponse>, Status> {
        let remote = RemoteRef::try_from(request.into_inner()).map_err(status_from_convert)?;
        let allowlist = self.allowlist.clone();
        let entry = tokio::task::spawn_blocking(move || allowlist.entry(&remote))
            .await
            .map_err(status_from_join)?
            .map_err(status_from_walker)?;
        Ok(Response::new(StatResponse {
            entry: Some(WireEntry::try_from(&entry).map_err(status_from_convert)?),
        }))
    }

    type GetRangeStream =
        Pin<Box<dyn Stream<Item = Result<GetRangeResponse, Status>> + Send + 'static>>;

    async fn get_range(
        &self,
        request: Request<GetRangeRequest>,
    ) -> Result<Response<Self::GetRangeStream>, Status> {
        let req = request.into_inner();
        let offset = req.offset;
        let len = req.len;
        let remote = RemoteRef::try_from(req).map_err(status_from_convert)?;
        let allowlist = self.allowlist.clone();
        let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let mut file = allowlist.open(&remote).map_err(|err| err.to_string())?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|err| err.to_string())?;
            let remain = file
                .metadata()
                .map_err(|err| err.to_string())?
                .len()
                .saturating_sub(offset);
            let want_u64 = len.min(remain).min(64 * 1024 * 1024);
            let mut buf = Vec::new();
            file.take(want_u64)
                .read_to_end(&mut buf)
                .map_err(|err| err.to_string())?;
            Ok(buf)
        })
        .await
        .map_err(status_from_join)?
        .map_err(|message| {
            status_from_error_detail(&ErrorDetail::from(ControlError {
                exit_code: ExitCode::Runtime,
                message,
            }))
        })?;

        let chunks = data
            .chunks(64 * 1024)
            .map(|c| c.to_vec())
            .collect::<Vec<_>>();
        let stream = tokio_stream::iter(
            chunks
                .into_iter()
                .map(|bytes| Ok(GetRangeResponse { data: bytes.into() })),
        );
        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        Allowlist, BoxFuture, ControlError, DesiredState, EdgeApiReport, GrabApplyReport, GrabOps,
        Grabber, GuardPreviewItem, HoldKey, HoldLiveItem, KeyPresence, ReleaseId, TitleId,
    };
    use mediaops_proto::{
        DeleteRemoteRequest, EdgeApplyRequest, EdgeCheckRequest, GetRangeRequest, GrabApplyRequest,
        GuardPreviewRequest, HoldListRequest, HoldRejectRequest, KeyDiscoveryRequest,
        PROTO_PACKAGE, RemoteRef as WireRef, StatRequest, UnmonitorRequest, WantedMissingRequest,
    };
    use std::io::Write;
    use tokio_stream::StreamExt;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-seedbox-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn seedbox_with(root: std::path::PathBuf) -> Seedbox {
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root).expect("root");
        Seedbox::new(allowlist, "0.1.0", Grabber::None)
    }

    fn write_file(path: &std::path::Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let mut f = std::fs::File::create(path).expect("create");
        f.write_all(bytes).expect("write");
    }

    fn movie_rel() -> &'static str {
        "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv"
    }

    fn seeding_item(content_path: &str) -> GuardPreviewItem {
        GuardPreviewItem {
            hash: "aa".into(),
            state: "uploading".into(),
            ratio: 2.0,
            is_private: false,
            content_path: content_path.into(),
            save_path: String::new(),
            remote: None,
        }
    }

    fn private_under_goal_item(content_path: &str) -> GuardPreviewItem {
        GuardPreviewItem {
            hash: "bb".into(),
            state: "pausedDL".into(),
            ratio: 0.4,
            is_private: true,
            content_path: content_path.into(),
            save_path: String::new(),
            remote: None,
        }
    }

    #[tokio::test]
    async fn delete_remote_unlinks_usenet_when_qbit_has_no_match() {
        let root = scratch("unlink-usenet");
        write_file(&root.join(movie_rel()), b"copy");
        let seed = seedbox_with(root.clone());
        let remote = mediaops_core::RemoteRef::from_wire_parts(
            "seedbox".into(),
            std::path::PathBuf::from(movie_rel()),
        )
        .expect("ref");
        let resp = seed
            .delete_remote(Request::new(DeleteRemoteRequest {
                r#ref: Some(WireRef {
                    root_id: remote.root_id().into(),
                    rel_path: remote.rel_path().display().to_string(),
                }),
            }))
            .await
            .expect("delete")
            .into_inner();
        assert_eq!(resp.result, DeleteRemoteResult::Deleted as i32);
        assert!(!root.join(movie_rel()).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn edge_apply_grabber_none_with_ops_is_handshake_noop() {
        let root = scratch("edge-none");
        let ops = std::sync::Arc::new(FakeGrabOps::default());
        let seed = seedbox_with(root.clone()).with_grab_ops(Some(ops.clone()));
        let resp = seed
            .edge_apply(Request::new(EdgeApplyRequest::default()))
            .await
            .expect("noop")
            .into_inner();
        assert!(resp.noop);
        assert!(resp.diff.is_empty());
        assert_eq!(
            *ops.edge_apply_calls.lock().expect("lock"),
            0,
            "grabber=None must not call GrabOps::edge_apply"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_remote_skipped_seeding_leaves_the_file() {
        let root = scratch("skip-seed");
        let path = root.join(movie_rel());
        write_file(&path, b"seed");
        let seed =
            seedbox_with(root.clone()).with_grab_ops(Some(std::sync::Arc::new(FakeGrabOps {
                qbit: vec![seeding_item(&path.display().to_string())],
                qbit_down: false,
                ..Default::default()
            })));
        let resp = seed
            .delete_remote(Request::new(DeleteRemoteRequest {
                r#ref: Some(WireRef {
                    root_id: "seedbox".into(),
                    rel_path: movie_rel().into(),
                }),
            }))
            .await
            .expect("skip")
            .into_inner();
        assert_eq!(resp.result, DeleteRemoteResult::SkippedSeeding as i32);
        assert!(path.exists(), "seeding file must remain");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_remote_private_under_goal_leaves_the_file() {
        let root = scratch("skip-private");
        let path = root.join(movie_rel());
        write_file(&path, b"priv");
        let seed =
            seedbox_with(root.clone()).with_grab_ops(Some(std::sync::Arc::new(FakeGrabOps {
                qbit: vec![private_under_goal_item(&path.display().to_string())],
                ..Default::default()
            })));
        let resp = seed
            .delete_remote(Request::new(DeleteRemoteRequest {
                r#ref: Some(WireRef {
                    root_id: "seedbox".into(),
                    rel_path: movie_rel().into(),
                }),
            }))
            .await
            .expect("skip")
            .into_inner();
        assert_eq!(resp.result, DeleteRemoteResult::SkippedSeeding as i32);
        assert!(path.exists(), "private-under-goal file must remain");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_remote_qbit_down_is_skipped_seeding() {
        let root = scratch("qbit-down");
        write_file(&root.join(movie_rel()), b"x");
        let seed =
            seedbox_with(root.clone()).with_grab_ops(Some(std::sync::Arc::new(FakeGrabOps {
                qbit_down: true,
                ..Default::default()
            })));
        let resp = seed
            .delete_remote(Request::new(DeleteRemoteRequest {
                r#ref: Some(WireRef {
                    root_id: "seedbox".into(),
                    rel_path: movie_rel().into(),
                }),
            }))
            .await
            .expect("fail-closed")
            .into_inner();
        assert_eq!(resp.result, DeleteRemoteResult::SkippedSeeding as i32);
        assert!(root.join(movie_rel()).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_remote_nlink_greater_than_one_leaves_the_file() {
        let root = scratch("hardlink");
        let path = root.join(movie_rel());
        write_file(&path, b"link");
        std::fs::hard_link(&path, root.join("other.bin")).expect("hardlink");
        let seed = seedbox_with(root.clone());
        let resp = seed
            .delete_remote(Request::new(DeleteRemoteRequest {
                r#ref: Some(WireRef {
                    root_id: "seedbox".into(),
                    rel_path: movie_rel().into(),
                }),
            }))
            .await
            .expect("skip")
            .into_inner();
        assert_eq!(resp.result, DeleteRemoteResult::SkippedSeeding as i32);
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn guard_preview_attaches_allowlisted_remote() {
        let root = scratch("guard");
        let path = root.join(movie_rel());
        write_file(&path, b"x");
        let seed =
            seedbox_with(root.clone()).with_grab_ops(Some(std::sync::Arc::new(FakeGrabOps {
                qbit: vec![seeding_item(&path.display().to_string())],
                ..Default::default()
            })));
        let resp = seed
            .guard_preview(Request::new(GuardPreviewRequest {}))
            .await
            .expect("preview")
            .into_inner();
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].state, "uploading");
        assert_eq!(
            resp.items[0].r#ref.as_ref().map(|r| r.rel_path.as_str()),
            Some(movie_rel())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn grab_apply_with_grabber_none_is_handshake() {
        let root = scratch("grab");
        let seed = seedbox_with(root.clone());
        let resp = seed
            .grab_apply(Request::new(GrabApplyRequest::default()))
            .await
            .expect("grab none")
            .into_inner();
        assert_eq!(resp.semver, "0.1.0");
        assert_eq!(resp.proto_package, PROTO_PACKAGE);
        assert!(resp.noop);
        let keys = seed
            .key_discovery(Request::new(KeyDiscoveryRequest {}))
            .await
            .expect("keys")
            .into_inner();
        assert!(!keys.sonarr_key_present);
        let holds = seed
            .hold_list(Request::new(HoldListRequest {}))
            .await
            .expect("hold list none")
            .into_inner();
        assert_eq!(holds.proto_package, PROTO_PACKAGE);
        assert!(holds.items.is_empty());
        let wanted = seed
            .wanted_missing(Request::new(WantedMissingRequest {}))
            .await
            .expect("wanted none")
            .into_inner();
        assert_eq!(wanted.proto_package, PROTO_PACKAGE);
        assert!(wanted.title_id.is_empty());
        let none_err = seed
            .unmonitor(Request::new(UnmonitorRequest {
                title_id: "movie:tmdb:603".into(),
            }))
            .await
            .expect_err("grabber=None unmonitor is usage");
        let detail = mediaops_proto::error_detail_from_status(&none_err).expect("detail");
        assert_eq!(detail.exit_code, i32::from(mediaops_core::ExitCode::Usage));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn unmonitor_without_grab_ops_is_usage_distinct_from_grabber_none() {
        let root = scratch("unmonitor-no-ops");
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");
        let seed = Seedbox::new(allowlist, "0.1.0", Grabber::Servarr);
        let err = seed
            .unmonitor(Request::new(UnmonitorRequest {
                title_id: "movie:tmdb:603".into(),
            }))
            .await
            .expect_err("ops missing");
        let detail = mediaops_proto::error_detail_from_status(&err).expect("detail");
        assert_eq!(detail.exit_code, i32::from(mediaops_core::ExitCode::Usage));
        assert!(
            !err.message().contains("grabber is none"),
            "{}",
            err.message()
        );
        assert!(err.message().contains("not injected"), "{}", err.message());
        let _ = std::fs::remove_dir_all(root);
    }

    struct FakeGrabOps {
        items: Vec<HoldLiveItem>,
        wanted: Vec<TitleId>,
        unmonitored: std::sync::Mutex<Vec<TitleId>>,
        qbit: Vec<GuardPreviewItem>,
        qbit_down: bool,
        edge_apply_calls: std::sync::Mutex<usize>,
    }

    impl Default for FakeGrabOps {
        fn default() -> Self {
            Self {
                items: Vec::new(),
                wanted: Vec::new(),
                unmonitored: std::sync::Mutex::new(Vec::new()),
                qbit: Vec::new(),
                qbit_down: false,
                edge_apply_calls: std::sync::Mutex::new(0),
            }
        }
    }

    impl GrabOps for FakeGrabOps {
        fn grab_apply<'a>(
            &'a self,
            _: &'a DesiredState,
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: true,
                    diff: String::new(),
                })
            })
        }
        fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>> {
            Box::pin(async { Ok(KeyPresence::default()) })
        }
        fn edge_api_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>> {
            Box::pin(async {
                Ok(EdgeApiReport {
                    fingerprint: String::new(),
                    invariant_ok: true,
                    drift: String::new(),
                })
            })
        }
        fn edge_apply<'a>(
            &'a self,
            _: &'a DesiredState,
        ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
            *self.edge_apply_calls.lock().expect("lock") += 1;
            Box::pin(async {
                Ok(GrabApplyReport {
                    noop: false,
                    diff: "would PUT host config".into(),
                })
            })
        }
        fn hold_list(&self) -> BoxFuture<'_, Result<Vec<HoldLiveItem>, ControlError>> {
            let items = self.items.clone();
            Box::pin(async move { Ok(items) })
        }
        fn hold_reject<'a>(&'a self, _: &'a HoldKey) -> BoxFuture<'a, Result<(), ControlError>> {
            Box::pin(async { Ok(()) })
        }
        fn wanted_missing(&self) -> BoxFuture<'_, Result<Vec<TitleId>, ControlError>> {
            let wanted = self.wanted.clone();
            Box::pin(async move { Ok(wanted) })
        }
        fn unmonitor<'a>(
            &'a self,
            title_id: &'a TitleId,
        ) -> BoxFuture<'a, Result<(), ControlError>> {
            self.unmonitored
                .lock()
                .expect("lock")
                .push(title_id.clone());
            Box::pin(async { Ok(()) })
        }
        fn qbit_snapshot(&self) -> BoxFuture<'_, Result<Vec<GuardPreviewItem>, ControlError>> {
            let down = self.qbit_down;
            let items = self.qbit.clone();
            Box::pin(async move {
                if down {
                    return Err(ControlError::runtime("qbit down"));
                }
                Ok(items)
            })
        }
    }

    #[tokio::test]
    async fn hold_list_uses_grab_ops_when_grabber_is_servarr() {
        let root = scratch("hold-ops");
        let item = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::parse("deadbeef").expect("id"),
            ),
            1,
            2,
            "blocked",
        );
        let seed =
            seedbox_with(root.clone()).with_grab_ops(Some(std::sync::Arc::new(FakeGrabOps {
                items: vec![item.clone()],
                ..Default::default()
            })));
        // grabber stays None in seedbox_with — None must still be empty.
        let none = seed
            .hold_list(Request::new(HoldListRequest {}))
            .await
            .expect("none")
            .into_inner();
        assert!(none.items.is_empty());

        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");
        let servarr = Seedbox::new(allowlist, "0.1.0", Grabber::Servarr).with_grab_ops(Some(
            std::sync::Arc::new(FakeGrabOps {
                items: vec![item.clone()],
                ..Default::default()
            }),
        ));
        let listed = servarr
            .hold_list(Request::new(HoldListRequest {}))
            .await
            .expect("servarr")
            .into_inner();
        assert_eq!(listed.items.len(), 1);
        assert_eq!(listed.items[0].title_id, "movie:tmdb:603");
        assert_eq!(listed.items[0].release_id, "deadbeef");

        let rejected = servarr
            .hold_reject(Request::new(HoldRejectRequest {
                title_id: "movie:tmdb:603".into(),
                release_id: "deadbeef".into(),
            }))
            .await
            .expect("reject servarr")
            .into_inner();
        assert_eq!(rejected.proto_package, PROTO_PACKAGE);
        let none_err = seed
            .hold_reject(Request::new(HoldRejectRequest {
                title_id: "movie:tmdb:603".into(),
                release_id: "deadbeef".into(),
            }))
            .await
            .expect_err("grabber=None reject is usage");
        let detail = mediaops_proto::error_detail_from_status(&none_err).expect("detail");
        assert_eq!(detail.exit_code, i32::from(mediaops_core::ExitCode::Usage));

        let ops = FakeGrabOps {
            wanted: vec![TitleId::movie("603").expect("title")],
            ..Default::default()
        };
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");
        let servarr = Seedbox::new(allowlist, "0.1.0", Grabber::Servarr)
            .with_grab_ops(Some(std::sync::Arc::new(ops)));
        let wanted = servarr
            .wanted_missing(Request::new(WantedMissingRequest {}))
            .await
            .expect("wanted")
            .into_inner();
        assert_eq!(wanted.title_id, vec!["movie:tmdb:603".to_string()]);
        let unmonitored = servarr
            .unmonitor(Request::new(UnmonitorRequest {
                title_id: "movie:tmdb:603".into(),
            }))
            .await
            .expect("unmonitor")
            .into_inner();
        assert_eq!(unmonitored.proto_package, PROTO_PACKAGE);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn hold_list_maps_output_path_through_allowlist_to_remote_ref() {
        let root = scratch("hold-remote");
        write_file(&root.join("The.Matrix.1999.mkv"), b"abcd");
        let mut item = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::parse("deadbeef").expect("id"),
            ),
            1,
            4,
            "blocked",
        );
        item.output_path = Some(root.join("The.Matrix.1999.mkv").display().to_string());
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");
        let seed = Seedbox::new(allowlist, "0.1.0", Grabber::Servarr).with_grab_ops(Some(
            std::sync::Arc::new(FakeGrabOps {
                items: vec![item],
                ..Default::default()
            }),
        ));
        let listed = seed
            .hold_list(Request::new(HoldListRequest {}))
            .await
            .expect("list")
            .into_inner();
        assert_eq!(listed.items.len(), 1);
        let remote = listed.items[0].remote.as_ref().expect("remote");
        assert_eq!(remote.root_id, "seedbox");
        assert_eq!(remote.rel_path, "The.Matrix.1999.mkv");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn hold_list_maps_directory_output_path_to_the_one_media_file() {
        let root = scratch("hold-dir");
        let dir = root.join("The.Matrix.1999");
        write_file(&dir.join("movie.nfo"), b"nfo");
        write_file(&dir.join("poster.jpg"), b"jpg");
        write_file(&dir.join("sample.mkv"), b"sample");
        write_file(&dir.join("The.Matrix.1999.mkv"), b"abcd");
        let mut item = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::parse("deadbeef").expect("id"),
            ),
            1,
            4,
            "blocked",
        );
        item.output_path = Some(dir.display().to_string());
        let mut allowlist = Allowlist::new();
        allowlist.add_root("seedbox", root.clone()).expect("root");
        let seed = Seedbox::new(allowlist, "0.1.0", Grabber::Servarr).with_grab_ops(Some(
            std::sync::Arc::new(FakeGrabOps {
                items: vec![item],
                ..Default::default()
            }),
        ));
        let listed = seed
            .hold_list(Request::new(HoldListRequest {}))
            .await
            .expect("list")
            .into_inner();
        let remote = listed.items[0].remote.as_ref().expect("remote");
        assert_eq!(remote.root_id, "seedbox");
        assert_eq!(remote.rel_path, "The.Matrix.1999/The.Matrix.1999.mkv");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn panel_host_rewrite_changes_fingerprint_and_drifts() {
        let root = scratch("nginx");
        let nginx = root.join("nginx");
        std::fs::create_dir_all(&nginx).expect("mkdir");
        std::fs::write(
            nginx.join("sonarr.conf"),
            "proxy_set_header Host 127.0.0.1;\n",
        )
        .expect("write");
        let seed = seedbox_with(root.clone()).with_nginx_dir(nginx.clone());
        let check = seed
            .edge_check(Request::new(EdgeCheckRequest {}))
            .await
            .expect("check")
            .into_inner();
        assert!(!check.invariant_ok);
        assert!(check.drift.contains("Host rewrite"));
        let good = "proxy_set_header Host $host;\n";
        std::fs::write(nginx.join("sonarr.conf"), good).expect("rewrite");
        let repaired = seed
            .edge_check(Request::new(EdgeCheckRequest {}))
            .await
            .expect("repaired")
            .into_inner();
        assert!(repaired.invariant_ok);
        assert_ne!(check.fingerprint, repaired.fingerprint);
        let apply = seed
            .edge_apply(Request::new(EdgeApplyRequest::default()))
            .await
            .expect("edge apply none")
            .into_inner();
        assert!(apply.noop);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stat_unknown_path_is_runtime_status() {
        let root = scratch("stat");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let seed = seedbox_with(root.clone());
        let err = seed
            .stat(Request::new(StatRequest {
                r#ref: Some(WireRef {
                    root_id: "seedbox".into(),
                    rel_path: "missing.bin".into(),
                }),
            }))
            .await
            .expect_err("missing");
        assert!(!err.message().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn get_range_past_eof_is_empty_and_oversize_is_clamped_to_remain() {
        let root = scratch("range");
        write_file(&root.join("a.bin"), b"abcdefghij");
        let seed = seedbox_with(root.clone());
        let wire = WireRef {
            root_id: "seedbox".into(),
            rel_path: "a.bin".into(),
        };
        let past = seed
            .get_range(Request::new(GetRangeRequest {
                r#ref: Some(wire.clone()),
                offset: 10,
                len: 4,
            }))
            .await
            .expect("eof")
            .into_inner();
        let chunks: Vec<_> = past.collect().await;
        assert!(chunks.is_empty() || chunks.iter().all(|c| c.as_ref().unwrap().data.is_empty()));

        let oversize = seed
            .get_range(Request::new(GetRangeRequest {
                r#ref: Some(wire),
                offset: 8,
                len: 100,
            }))
            .await
            .expect("clamp")
            .into_inner();
        let mut body = Vec::new();
        let mut stream = oversize;
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk.expect("chunk").data);
        }
        assert_eq!(body, b"ij");
        let _ = std::fs::remove_dir_all(root);
    }
}
