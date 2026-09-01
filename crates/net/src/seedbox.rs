//! Seedbox Control + Transfer over the walker.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use mediaops_core::{
    Allowlist, ControlError, DesiredState, ExitCode, GrabOps, Grabber, RemoteRef, WalkerError,
    nginx_host_ok, panel_fingerprint,
};
use mediaops_proto::control_server::Control;
use mediaops_proto::transfer_server::Transfer;
use mediaops_proto::{
    DeleteRemoteRequest, DeleteRemoteResponse, DfRequest, DfResponse, EdgeApplyRequest,
    EdgeApplyResponse, EdgeCheckRequest, EdgeCheckResponse, ErrorDetail, GetRangeRequest,
    GetRangeResponse, GrabApplyRequest, GrabApplyResponse, GuardPreviewRequest,
    GuardPreviewResponse, KeyDiscoveryRequest, KeyDiscoveryResponse, ListRequest, ListResponse,
    PROTO_PACKAGE, RemoteEntry as WireEntry, StatRequest, StatResponse, UnmonitorRequest,
    UnmonitorResponse, status_from_error_detail,
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
    for entry in reader.flatten() {
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
        _request: Request<UnmonitorRequest>,
    ) -> Result<Response<UnmonitorResponse>, Status> {
        Err(unused("Unmonitor"))
    }

    async fn delete_remote(
        &self,
        _request: Request<DeleteRemoteRequest>,
    ) -> Result<Response<DeleteRemoteResponse>, Status> {
        Err(unused("DeleteRemote"))
    }

    async fn grab_apply(
        &self,
        request: Request<GrabApplyRequest>,
    ) -> Result<Response<GrabApplyResponse>, Status> {
        let (semver, proto_package) = self.handshake();
        if self.grabber == Grabber::None || self.grab_ops.is_none() {
            return Ok(Response::new(GrabApplyResponse {
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
            None => (String::new(), Vec::new()),
        };
        if let Some(ops) = &self.grab_ops {
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
        if self.grab_ops.is_none() {
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
        Err(unused("GuardPreview"))
    }
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
    use mediaops_core::{Allowlist, Grabber};
    use mediaops_proto::{
        DeleteRemoteRequest, EdgeApplyRequest, EdgeCheckRequest, GetRangeRequest, GrabApplyRequest,
        GuardPreviewRequest, KeyDiscoveryRequest, PROTO_PACKAGE, RemoteRef as WireRef, StatRequest,
        UnmonitorRequest,
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

    #[tokio::test]
    async fn unused_control_rpcs_fail_loudly() {
        let root = scratch("unused");
        let seed = seedbox_with(root.clone());
        for (name, result) in [
            (
                "Unmonitor",
                seed.unmonitor(Request::new(UnmonitorRequest {
                    title_id: "movie:tmdb:603".into(),
                }))
                .await
                .err(),
            ),
            (
                "DeleteRemote",
                seed.delete_remote(Request::new(DeleteRemoteRequest { r#ref: None }))
                    .await
                    .err(),
            ),
            (
                "GuardPreview",
                seed.guard_preview(Request::new(GuardPreviewRequest {}))
                    .await
                    .err(),
            ),
        ] {
            let err = result.expect(name);
            assert!(
                err.message()
                    .contains(&format!("{name} is not implemented")),
                "{name}: {}",
                err.message()
            );
        }
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
