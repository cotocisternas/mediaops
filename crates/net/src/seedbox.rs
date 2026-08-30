//! Seedbox Control + Transfer over the walker.

use std::io::{Read, Seek, SeekFrom};
use std::pin::Pin;

use mediaops_core::{Allowlist, ControlError, ExitCode, Grabber, RemoteRef, WalkerError};
use mediaops_proto::control_server::Control;
use mediaops_proto::transfer_server::Transfer;
use mediaops_proto::{
    DeleteRemoteRequest, DeleteRemoteResponse, DfRequest, DfResponse, EdgeCheckRequest,
    EdgeCheckResponse, ErrorDetail, GetRangeRequest, GetRangeResponse, GrabApplyRequest,
    GrabApplyResponse,     GuardPreviewRequest, GuardPreviewResponse, KeyDiscoveryRequest, KeyDiscoveryResponse,
    ListRequest, ListResponse, PROTO_PACKAGE, RemoteEntry as WireEntry, StatRequest, StatResponse,
    UnmonitorRequest, UnmonitorResponse, status_from_error_detail,
};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

#[derive(Clone)]
pub struct Seedbox {
    allowlist: Allowlist,
    semver: String,
    grabber: Grabber,
}

impl Seedbox {
    pub fn new(allowlist: Allowlist, semver: impl Into<String>, grabber: Grabber) -> Self {
        Self {
            allowlist,
            semver: semver.into(),
            grabber,
        }
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

fn unused(name: &str) -> Status {
    status_from_error_detail(&ErrorDetail::from(ControlError {
        exit_code: ExitCode::Runtime,
        message: format!("{name} is not implemented in the seedbox role this epic"),
    }))
}

#[tonic::async_trait]
impl Control for Seedbox {
    async fn df(&self, _request: Request<DfRequest>) -> Result<Response<DfResponse>, Status> {
        let free = self.allowlist.free_bytes().map_err(status_from_walker)?;
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
        _request: Request<GrabApplyRequest>,
    ) -> Result<Response<GrabApplyResponse>, Status> {
        if self.grabber != Grabber::None {
            return Err(unused("GrabApply"));
        }
        let (semver, proto_package) = self.handshake();
        Ok(Response::new(GrabApplyResponse {
            semver,
            proto_package,
        }))
    }

    async fn edge_check(
        &self,
        _request: Request<EdgeCheckRequest>,
    ) -> Result<Response<EdgeCheckResponse>, Status> {
        Err(unused("EdgeCheck"))
    }

    async fn key_discovery(
        &self,
        _request: Request<KeyDiscoveryRequest>,
    ) -> Result<Response<KeyDiscoveryResponse>, Status> {
        Err(unused("KeyDiscovery"))
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
        let entries = self.allowlist.list().map_err(status_from_walker)?;
        let wire = entries
            .iter()
            .map(WireEntry::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(status_from_convert)?;
        Ok(Response::new(ListResponse { entries: wire }))
    }

    async fn stat(&self, request: Request<StatRequest>) -> Result<Response<StatResponse>, Status> {
        let remote = RemoteRef::try_from(request.into_inner()).map_err(status_from_convert)?;
        let entry = self.allowlist.entry(&remote).map_err(status_from_walker)?;
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
            let want = usize::try_from(len).unwrap_or(usize::MAX);
            let mut buf = vec![0_u8; want];
            let n = file.read(&mut buf).map_err(|err| err.to_string())?;
            buf.truncate(n);
            Ok(buf)
        })
        .await
        .map_err(|err| {
            status_from_error_detail(&ErrorDetail::from(ControlError {
                exit_code: ExitCode::Runtime,
                message: err.to_string(),
            }))
        })?
        .map_err(|message| {
            status_from_error_detail(&ErrorDetail::from(ControlError {
                exit_code: ExitCode::Runtime,
                message,
            }))
        })?;

        let chunks = data.chunks(64 * 1024).map(|c| c.to_vec()).collect::<Vec<_>>();
        let stream = tokio_stream::iter(chunks.into_iter().map(|bytes| {
            Ok(GetRangeResponse { data: bytes.into() })
        }));
        Ok(Response::new(Box::pin(stream)))
    }
}
