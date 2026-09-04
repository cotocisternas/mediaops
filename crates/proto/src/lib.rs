//! Generated `mediaops.v1` contract and the only wire↔domain conversions.
//!
//! Codegen is `tonic-prost-build`. RPC request/response types are generated;
//! this crate owns `From`/`TryFrom`, [`status_from_error_detail`], and
//! [`error_detail_from_status`]. [`ControlPortClient`] is the canonical
//! [`mediaops_core::ControlPort`] implementation over the generated client.

tonic::include_proto!("mediaops.v1");

use std::path::PathBuf;

use mediaops_core::{
    BoxFuture, Bytes, ControlError, ControlPort, DeleteRemoteOutcome, DfSnapshot, EdgeApiReport,
    ExitCode, GrabApplyReport, GuardPreviewItem as CoreGuardPreviewItem, HoldError, HoldKey,
    HoldLiveItem as CoreHoldLiveItem, KeyPresence, Placement as CorePlacement, ReleaseId,
    RemoteEntry as CoreRemoteEntry, RemoteRef as CoreRemoteRef, TitleId, TitleIdError, WalkerError,
};
use prost::Message;

/// Control responses carry this package name (AD-22). Additive evolution stays here.
pub const PROTO_PACKAGE: &str = "mediaops.v1";

/// Refuse a daemon whose proto package we do not speak (AD-22).
pub fn check_handshake(proto_package: &str) -> Result<(), ControlError> {
    if proto_package != PROTO_PACKAGE {
        return Err(ControlError {
            exit_code: ExitCode::Runtime,
            message: format!("unsupported proto package `{proto_package}`; want {PROTO_PACKAGE}"),
        });
    }
    Ok(())
}

fn accept_handshake(proto_package: &str, semver: &str) -> Result<(), ControlError> {
    check_handshake(proto_package)?;
    if let Some(msg) = minor_skew_warning(semver, env!("CARGO_PKG_VERSION")) {
        tracing::warn!("{msg}");
    }
    Ok(())
}

/// Warn when major matches and minor differs. Refuse is package-only.
pub fn minor_skew_warning(daemon_semver: &str, cli_semver: &str) -> Option<String> {
    let (d_maj, d_min, _) = mediaops_core::parse_semver(daemon_semver)?;
    let (c_maj, c_min, _) = mediaops_core::parse_semver(cli_semver)?;
    if d_maj == c_maj && d_min != c_min {
        Some(format!(
            "daemon {daemon_semver} minor-skew vs cli {cli_semver}"
        ))
    } else {
        None
    }
}

/// Failure converting a generated wire value to a domain type (or the reverse).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    #[error(transparent)]
    Walker(#[from] WalkerError),
    #[error("rel_path is not valid UTF-8")]
    NonUtf8Path,
    #[error("missing nested field `{0}`")]
    MissingField(&'static str),
    #[error("unspecified or unknown DeleteRemoteResult `{0}`")]
    UnknownDeleteRemoteResult(i32),
    #[error("missing ErrorDetail in Status details")]
    MissingErrorDetail,
    #[error("invalid ErrorDetail encoding")]
    InvalidErrorDetail,
    #[error("unknown exit_code `{0}`")]
    UnknownExitCode(i32),
    #[error(transparent)]
    TitleId(#[from] TitleIdError),
    #[error(transparent)]
    Hold(#[from] HoldError),
    #[error("invalid placement field `{0}`")]
    InvalidPlacement(&'static str),
}

fn require_nested<T>(value: Option<T>, field: &'static str) -> Result<T, ConvertError> {
    value.ok_or(ConvertError::MissingField(field))
}

fn exit_code_from_i32(n: i32) -> Result<ExitCode, ConvertError> {
    match n {
        0 => Ok(ExitCode::Ok),
        1 => Ok(ExitCode::Runtime),
        2 => Ok(ExitCode::Usage),
        3 => Ok(ExitCode::LockConflict),
        4 => Ok(ExitCode::DriftVerify),
        5 => Ok(ExitCode::PolicyRefusal),
        other => Err(ConvertError::UnknownExitCode(other)),
    }
}

fn convert_to_control(err: ConvertError) -> ControlError {
    ControlError {
        exit_code: ExitCode::Runtime,
        message: err.to_string(),
    }
}

impl TryFrom<RemoteRef> for CoreRemoteRef {
    type Error = ConvertError;

    fn try_from(value: RemoteRef) -> Result<Self, Self::Error> {
        Ok(CoreRemoteRef::from_wire_parts(
            value.root_id,
            PathBuf::from(value.rel_path),
        )?)
    }
}

impl TryFrom<&CoreRemoteRef> for RemoteRef {
    type Error = ConvertError;

    fn try_from(value: &CoreRemoteRef) -> Result<Self, Self::Error> {
        remote_ref_to_wire_utf8(value)
    }
}

fn remote_ref_to_wire_utf8(value: &CoreRemoteRef) -> Result<RemoteRef, ConvertError> {
    let rel_path = value
        .rel_path()
        .to_str()
        .ok_or(ConvertError::NonUtf8Path)?
        .to_owned();
    Ok(RemoteRef {
        root_id: value.root_id().to_owned(),
        rel_path,
    })
}

/// Hold live remotes must not be dropped: UTF-8 paths stay exact, else lossy.
fn remote_ref_to_wire(value: &CoreRemoteRef) -> RemoteRef {
    match remote_ref_to_wire_utf8(value) {
        Ok(wire) => wire,
        Err(_) => RemoteRef {
            root_id: value.root_id().to_owned(),
            rel_path: value.rel_path().to_string_lossy().into_owned(),
        },
    }
}

impl TryFrom<CoreRemoteRef> for RemoteRef {
    type Error = ConvertError;

    fn try_from(value: CoreRemoteRef) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

impl TryFrom<RemoteEntry> for CoreRemoteEntry {
    type Error = ConvertError;

    fn try_from(value: RemoteEntry) -> Result<Self, Self::Error> {
        let r#ref = CoreRemoteRef::try_from(require_nested(value.r#ref, "ref")?)?;
        Ok(CoreRemoteEntry::from_wire_parts(
            r#ref,
            value.len,
            value.mtime,
            value.nlink,
        ))
    }
}

impl TryFrom<&CoreRemoteEntry> for RemoteEntry {
    type Error = ConvertError;

    fn try_from(value: &CoreRemoteEntry) -> Result<Self, Self::Error> {
        Ok(Self {
            r#ref: Some(RemoteRef::try_from(value.r#ref())?),
            len: value.len(),
            mtime: value.mtime(),
            nlink: value.nlink(),
        })
    }
}

impl TryFrom<CoreRemoteEntry> for RemoteEntry {
    type Error = ConvertError;

    fn try_from(value: CoreRemoteEntry) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

impl TryFrom<StatRequest> for CoreRemoteRef {
    type Error = ConvertError;

    fn try_from(value: StatRequest) -> Result<Self, Self::Error> {
        Self::try_from(require_nested(value.r#ref, "ref")?)
    }
}

impl TryFrom<DeleteRemoteRequest> for CoreRemoteRef {
    type Error = ConvertError;

    fn try_from(value: DeleteRemoteRequest) -> Result<Self, Self::Error> {
        Self::try_from(require_nested(value.r#ref, "ref")?)
    }
}

impl TryFrom<GetRangeRequest> for CoreRemoteRef {
    type Error = ConvertError;

    fn try_from(value: GetRangeRequest) -> Result<Self, Self::Error> {
        Self::try_from(require_nested(value.r#ref, "ref")?)
    }
}

impl TryFrom<StatResponse> for CoreRemoteEntry {
    type Error = ConvertError;

    fn try_from(value: StatResponse) -> Result<Self, Self::Error> {
        Self::try_from(require_nested(value.entry, "entry")?)
    }
}

impl TryFrom<ListResponse> for Vec<CoreRemoteEntry> {
    type Error = ConvertError;

    fn try_from(value: ListResponse) -> Result<Self, Self::Error> {
        value.entries.into_iter().map(TryFrom::try_from).collect()
    }
}

impl TryFrom<DeleteRemoteResult> for DeleteRemoteOutcome {
    type Error = ConvertError;

    fn try_from(value: DeleteRemoteResult) -> Result<Self, Self::Error> {
        match value {
            DeleteRemoteResult::Deleted => Ok(Self::Deleted),
            DeleteRemoteResult::SkippedSeeding => Ok(Self::SkippedSeeding),
            DeleteRemoteResult::QbitUnavailable => Ok(Self::QbitUnavailable),
            DeleteRemoteResult::Unspecified => {
                Err(ConvertError::UnknownDeleteRemoteResult(value as i32))
            }
        }
    }
}

impl From<DeleteRemoteOutcome> for DeleteRemoteResult {
    fn from(outcome: DeleteRemoteOutcome) -> Self {
        match outcome {
            DeleteRemoteOutcome::Deleted => Self::Deleted,
            DeleteRemoteOutcome::SkippedSeeding => Self::SkippedSeeding,
            DeleteRemoteOutcome::QbitUnavailable => Self::QbitUnavailable,
        }
    }
}

impl TryFrom<DeleteRemoteResponse> for DeleteRemoteOutcome {
    type Error = ConvertError;

    fn try_from(value: DeleteRemoteResponse) -> Result<Self, Self::Error> {
        let result = DeleteRemoteResult::try_from(value.result)
            .map_err(|_| ConvertError::UnknownDeleteRemoteResult(value.result))?;
        Self::try_from(result)
    }
}

impl From<&TitleId> for UnmonitorRequest {
    fn from(title_id: &TitleId) -> Self {
        Self {
            title_id: title_id.render(),
        }
    }
}

impl From<&CoreHoldLiveItem> for HoldLiveItem {
    fn from(item: &CoreHoldLiveItem) -> Self {
        Self {
            title_id: item.key.title_id.render(),
            release_id: item.key.release_id.as_str().to_string(),
            added_unix: item.added_unix,
            size: item.size,
            reason: item.reason.clone(),
            remote: item.remote.as_ref().map(remote_ref_to_wire),
            placement: item.placement.as_ref().map(Placement::from),
        }
    }
}

impl From<CoreHoldLiveItem> for HoldLiveItem {
    fn from(item: CoreHoldLiveItem) -> Self {
        Self::from(&item)
    }
}

impl TryFrom<HoldLiveItem> for CoreHoldLiveItem {
    type Error = ConvertError;

    fn try_from(value: HoldLiveItem) -> Result<Self, Self::Error> {
        Ok(Self {
            key: HoldKey::new(
                TitleId::parse(&value.title_id)?,
                ReleaseId::parse(&value.release_id)?,
            ),
            added_unix: value.added_unix,
            size: value.size,
            reason: value.reason,
            remote: value.remote.map(CoreRemoteRef::try_from).transpose()?,
            placement: value.placement.map(CorePlacement::try_from).transpose()?,
            output_path: None,
        })
    }
}

impl From<&HoldKey> for HoldRejectRequest {
    fn from(key: &HoldKey) -> Self {
        Self {
            title_id: key.title_id.render(),
            release_id: key.release_id.as_str().to_string(),
        }
    }
}

impl TryFrom<HoldRejectRequest> for HoldKey {
    type Error = ConvertError;

    fn try_from(value: HoldRejectRequest) -> Result<Self, Self::Error> {
        Ok(Self::new(
            TitleId::parse(&value.title_id)?,
            ReleaseId::parse(&value.release_id)?,
        ))
    }
}

impl From<&CorePlacement> for Placement {
    fn from(placement: &CorePlacement) -> Self {
        match placement {
            CorePlacement::Movie {
                title,
                year,
                extension,
            } => Self {
                kind: Some(placement::Kind::Movie(MoviePlacement {
                    title: title.clone(),
                    year: u32::from(*year),
                    extension: extension.clone(),
                })),
            },
            CorePlacement::Episode {
                title,
                year,
                season,
                episode,
                episode_end,
                episode_title,
                extension,
            } => Self {
                kind: Some(placement::Kind::Episode(EpisodePlacement {
                    title: title.clone(),
                    year: u32::from(*year),
                    season: u32::from(*season),
                    episode: u32::from(*episode),
                    extension: extension.clone(),
                    episode_end: episode_end.map(u32::from).unwrap_or(0),
                    episode_title: episode_title.clone().unwrap_or_default(),
                })),
            },
            CorePlacement::Track {
                artist,
                album,
                year,
                disc,
                track,
                title,
                extension,
            } => Self {
                kind: Some(placement::Kind::Track(TrackPlacement {
                    album: album.clone(),
                    year: u32::from(*year),
                    track: track.map(u32::from).unwrap_or(0),
                    title: title.clone(),
                    extension: extension.clone(),
                    artist: artist.clone(),
                    disc: disc.map(u32::from).unwrap_or(0),
                })),
            },
        }
    }
}

impl TryFrom<Placement> for CorePlacement {
    type Error = ConvertError;

    fn try_from(value: Placement) -> Result<Self, Self::Error> {
        match value.kind {
            Some(placement::Kind::Movie(movie)) => Ok(Self::movie(
                movie.title,
                fit_u16(movie.year, "year")?,
                movie.extension,
            )),
            Some(placement::Kind::Episode(ep)) => Ok(Self::episode_titled(
                ep.title,
                fit_u16(ep.year, "year")?,
                fit_u8(ep.season, "season")?,
                fit_u16(ep.episode, "episode")?,
                (ep.episode_end != 0)
                    .then(|| fit_u16(ep.episode_end, "episode_end"))
                    .transpose()?,
                (!ep.episode_title.is_empty()).then_some(ep.episode_title),
                ep.extension,
            )),
            Some(placement::Kind::Track(track)) => Ok(Self::track(
                track.artist,
                track.album,
                fit_u16(track.year, "year")?,
                (track.disc != 0)
                    .then(|| fit_u8(track.disc, "disc"))
                    .transpose()?,
                (track.track != 0)
                    .then(|| fit_u8(track.track, "track"))
                    .transpose()?,
                track.title,
                track.extension,
            )),
            None => Err(ConvertError::MissingField("placement.kind")),
        }
    }
}

fn fit_u16(n: u32, field: &'static str) -> Result<u16, ConvertError> {
    u16::try_from(n).map_err(|_| ConvertError::InvalidPlacement(field))
}

fn fit_u8(n: u32, field: &'static str) -> Result<u8, ConvertError> {
    u8::try_from(n).map_err(|_| ConvertError::InvalidPlacement(field))
}

impl TryFrom<HoldListResponse> for Vec<CoreHoldLiveItem> {
    type Error = ConvertError;

    fn try_from(value: HoldListResponse) -> Result<Self, Self::Error> {
        value.items.into_iter().map(TryFrom::try_from).collect()
    }
}

impl TryFrom<WantedMissingResponse> for Vec<TitleId> {
    type Error = ConvertError;

    fn try_from(value: WantedMissingResponse) -> Result<Self, Self::Error> {
        Ok(value
            .title_id
            .into_iter()
            .filter_map(|raw| TitleId::parse(&raw).ok())
            .collect())
    }
}

impl TryFrom<UnmonitorRequest> for TitleId {
    type Error = ConvertError;

    fn try_from(value: UnmonitorRequest) -> Result<Self, Self::Error> {
        Ok(TitleId::parse(&value.title_id)?)
    }
}

impl From<DfResponse> for Bytes {
    fn from(response: DfResponse) -> Self {
        Self::new(response.free_bytes)
    }
}

impl From<DfResponse> for DfSnapshot {
    fn from(response: DfResponse) -> Self {
        Self {
            free: Bytes::new(response.free_bytes),
            semver: response.semver,
            proto_package: response.proto_package,
        }
    }
}

impl From<GrabApplyResponse> for GrabApplyReport {
    fn from(response: GrabApplyResponse) -> Self {
        Self {
            noop: response.noop,
            diff: response.diff,
        }
    }
}

impl From<EdgeApplyResponse> for GrabApplyReport {
    fn from(response: EdgeApplyResponse) -> Self {
        Self {
            noop: response.noop,
            diff: response.diff,
        }
    }
}

impl From<EdgeCheckResponse> for EdgeApiReport {
    fn from(response: EdgeCheckResponse) -> Self {
        Self {
            fingerprint: response.fingerprint,
            invariant_ok: response.invariant_ok,
            drift: response.drift,
        }
    }
}

impl TryFrom<GuardPreviewItem> for CoreGuardPreviewItem {
    type Error = ConvertError;

    fn try_from(value: GuardPreviewItem) -> Result<Self, Self::Error> {
        Ok(Self {
            hash: value.hash,
            state: value.state,
            ratio: value.ratio,
            is_private: value.is_private,
            content_path: value.content_path,
            save_path: value.save_path,
            remote: value.r#ref.map(CoreRemoteRef::try_from).transpose()?,
        })
    }
}

impl TryFrom<&CoreGuardPreviewItem> for GuardPreviewItem {
    type Error = ConvertError;

    fn try_from(value: &CoreGuardPreviewItem) -> Result<Self, Self::Error> {
        Ok(Self {
            hash: value.hash.clone(),
            state: value.state.clone(),
            ratio: value.ratio,
            is_private: value.is_private,
            content_path: value.content_path.clone(),
            save_path: value.save_path.clone(),
            r#ref: value.remote.as_ref().map(RemoteRef::try_from).transpose()?,
        })
    }
}

impl From<KeyDiscoveryResponse> for KeyPresence {
    fn from(response: KeyDiscoveryResponse) -> Self {
        Self {
            sonarr_key_present: response.sonarr_key_present,
            radarr_key_present: response.radarr_key_present,
            lidarr_key_present: response.lidarr_key_present,
            prowlarr_key_present: response.prowlarr_key_present,
            sab_key_present: response.sab_key_present,
            qbit_key_present: response.qbit_key_present,
        }
    }
}

impl From<ControlError> for ErrorDetail {
    fn from(err: ControlError) -> Self {
        Self {
            exit_code: i32::from(err.exit_code),
            reason: err.exit_code.error_code().to_string(),
            message: err.message,
        }
    }
}

impl TryFrom<ErrorDetail> for ControlError {
    type Error = ConvertError;

    fn try_from(detail: ErrorDetail) -> Result<Self, Self::Error> {
        Ok(Self {
            exit_code: exit_code_from_i32(detail.exit_code)?,
            message: detail.message,
        })
    }
}

/// `ErrorDetail.reason` that maps to gRPC `ResourceExhausted` (AD-12 pool pin).
pub const REASON_RESOURCE_EXHAUSTED: &str = "resource_exhausted";

/// Pack a serialized [`ErrorDetail`] into `Status::details` (ADV-8 Construction A).
///
/// Pool exhaustion is the one case that is not `Code::Unknown`: it uses
/// `ResourceExhausted` so a naive gateway cannot silently queue onto a shared
/// channel. Every other domain error stays Unknown + details.
pub fn status_from_error_detail(detail: &ErrorDetail) -> tonic::Status {
    let code = if detail.reason == REASON_RESOURCE_EXHAUSTED {
        tonic::Code::ResourceExhausted
    } else {
        tonic::Code::Unknown
    };
    tonic::Status::with_details(code, detail.message.clone(), detail.encode_to_vec().into())
}

/// Domain detail for a refused N+1th concurrent Range stream.
pub fn resource_exhausted_detail(message: impl Into<String>) -> ErrorDetail {
    ErrorDetail {
        exit_code: i32::from(ExitCode::Runtime),
        reason: REASON_RESOURCE_EXHAUSTED.to_string(),
        message: message.into(),
    }
}

/// Parse a serialized [`ErrorDetail`] from `Status::details`. Missing or unknown fail.
pub fn error_detail_from_status(status: &tonic::Status) -> Result<ErrorDetail, ConvertError> {
    if status.details().is_empty() {
        return Err(ConvertError::MissingErrorDetail);
    }
    let detail =
        ErrorDetail::decode(status.details()).map_err(|_| ConvertError::InvalidErrorDetail)?;
    exit_code_from_i32(detail.exit_code)?;
    Ok(detail)
}

fn control_error_from_status(status: tonic::Status) -> ControlError {
    match error_detail_from_status(&status).and_then(ControlError::try_from) {
        Ok(err) => err,
        Err(_) => ControlError {
            exit_code: ExitCode::Runtime,
            message: status.message().to_string(),
        },
    }
}

/// Canonical [`ControlPort`] over the generated [`control_client::ControlClient`].
#[derive(Debug, Clone)]
pub struct ControlPortClient<T> {
    inner: control_client::ControlClient<T>,
}

impl<T> ControlPortClient<T> {
    pub fn new(inner: control_client::ControlClient<T>) -> Self {
        Self { inner }
    }
}

impl<T> ControlPort for ControlPortClient<T>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Clone + Send + Sync + 'static,
    T::Error: Into<tonic::codegen::StdError>,
    T::Future: Send,
    T::ResponseBody: tonic::codegen::Body<Data = tonic::codegen::Bytes> + Send + 'static,
    <T::ResponseBody as tonic::codegen::Body>::Error: Into<tonic::codegen::StdError> + Send,
{
    fn df(&self) -> BoxFuture<'_, Result<DfSnapshot, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .df(DfRequest {})
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(DfSnapshot::from(inner))
        })
    }

    fn unmonitor<'a>(&'a self, title_id: &'a TitleId) -> BoxFuture<'a, Result<(), ControlError>> {
        let mut client = self.inner.clone();
        let request = UnmonitorRequest::from(title_id);
        Box::pin(async move {
            let response = client
                .unmonitor(request)
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(())
        })
    }

    fn delete_remote<'a>(
        &'a self,
        remote: &'a CoreRemoteRef,
    ) -> BoxFuture<'a, Result<DeleteRemoteOutcome, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let request = DeleteRemoteRequest {
                r#ref: Some(RemoteRef::try_from(remote).map_err(convert_to_control)?),
            };
            let response = client
                .delete_remote(request)
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            DeleteRemoteOutcome::try_from(inner).map_err(convert_to_control)
        })
    }

    fn grab_apply<'a>(
        &'a self,
        desired_state_toml: &'a [u8],
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
        let mut client = self.inner.clone();
        let toml = desired_state_toml.to_vec();
        Box::pin(async move {
            let response = client
                .grab_apply(GrabApplyRequest {
                    desired_state_toml: toml.into(),
                })
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(GrabApplyReport::from(inner))
        })
    }

    fn edge_check(&self) -> BoxFuture<'_, Result<EdgeApiReport, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .edge_check(EdgeCheckRequest {})
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(EdgeApiReport::from(inner))
        })
    }

    fn edge_apply<'a>(
        &'a self,
        desired_state_toml: &'a [u8],
    ) -> BoxFuture<'a, Result<GrabApplyReport, ControlError>> {
        let mut client = self.inner.clone();
        let toml = desired_state_toml.to_vec();
        Box::pin(async move {
            let response = client
                .edge_apply(EdgeApplyRequest {
                    desired_state_toml: toml.into(),
                })
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(GrabApplyReport::from(inner))
        })
    }

    fn key_discovery(&self) -> BoxFuture<'_, Result<KeyPresence, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .key_discovery(KeyDiscoveryRequest {})
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(KeyPresence::from(inner))
        })
    }

    fn guard_preview(&self) -> BoxFuture<'_, Result<Vec<CoreGuardPreviewItem>, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .guard_preview(GuardPreviewRequest {})
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            inner
                .items
                .into_iter()
                .map(CoreGuardPreviewItem::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(convert_to_control)
        })
    }

    fn hold_list(&self) -> BoxFuture<'_, Result<Vec<CoreHoldLiveItem>, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .hold_list(HoldListRequest {})
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Vec::<CoreHoldLiveItem>::try_from(inner).map_err(convert_to_control)
        })
    }

    fn hold_reject<'a>(&'a self, key: &'a HoldKey) -> BoxFuture<'a, Result<(), ControlError>> {
        let mut client = self.inner.clone();
        let request = HoldRejectRequest::from(key);
        Box::pin(async move {
            let response = client
                .hold_reject(request)
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Ok(())
        })
    }

    fn wanted_missing(&self) -> BoxFuture<'_, Result<Vec<TitleId>, ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .wanted_missing(WantedMissingRequest {})
                .await
                .map_err(control_error_from_status)?;
            let inner = response.into_inner();
            accept_handshake(&inner.proto_package, &inner.semver)?;
            Vec::<TitleId>::try_from(inner).map_err(convert_to_control)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn wire_ref(root_id: &str, rel_path: &str) -> RemoteRef {
        RemoteRef {
            root_id: root_id.to_string(),
            rel_path: rel_path.to_string(),
        }
    }

    fn domain_ref(root_id: &str, rel_path: &str) -> CoreRemoteRef {
        CoreRemoteRef::from_wire_parts(root_id.to_string(), PathBuf::from(rel_path)).expect("ref")
    }

    #[test]
    fn remote_ref_happy() {
        let wire = wire_ref("seedbox", "a/b.bin");
        let domain = CoreRemoteRef::try_from(wire).expect("happy");
        assert_eq!(domain.root_id(), "seedbox");
        assert_eq!(domain.rel_path(), Path::new("a/b.bin"));
        let round_trip = RemoteRef::try_from(&domain).expect("to wire");
        assert_eq!(round_trip.root_id, "seedbox");
        assert_eq!(round_trip.rel_path, "a/b.bin");
    }

    #[test]
    fn empty_root_id_is_empty_root() {
        let err = CoreRemoteRef::try_from(wire_ref("", "a/b.bin")).unwrap_err();
        assert!(matches!(
            err,
            ConvertError::Walker(WalkerError::EmptyRootId)
        ));
    }

    #[test]
    fn escape_path_is_unknown_path() {
        for bad in ["/etc/passwd", "../..", ""] {
            let err = CoreRemoteRef::try_from(wire_ref("seedbox", bad)).unwrap_err();
            assert!(
                matches!(err, ConvertError::Walker(WalkerError::UnknownPath(_))),
                "{bad} => {err:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_rel_path_has_no_wire_ref() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let rel = PathBuf::from(OsString::from_vec(vec![0xff, b'a']));
        let domain = CoreRemoteRef::from_wire_parts("seedbox".into(), rel).expect("shape");
        assert!(matches!(
            RemoteRef::try_from(&domain),
            Err(ConvertError::NonUtf8Path)
        ));
    }

    #[test]
    fn remote_entry_mtime_round_trip() {
        let entry = CoreRemoteEntry::from_wire_parts(domain_ref("seedbox", "a/b.bin"), 5, -1, 2);
        let wire = RemoteEntry::try_from(&entry).expect("to wire");
        assert_eq!(wire.len, 5);
        assert_eq!(wire.mtime, -1);
        assert_eq!(wire.nlink, 2);
        let back = CoreRemoteEntry::try_from(wire).expect("from wire");
        assert_eq!(back.len(), 5);
        assert_eq!(back.mtime(), -1);
        assert_eq!(back.nlink(), 2);
        assert_eq!(back.r#ref().root_id(), "seedbox");
        assert_eq!(back.r#ref().rel_path(), Path::new("a/b.bin"));
    }

    #[test]
    fn error_detail_round_trips_through_status_details() {
        let codes = [
            ExitCode::Ok,
            ExitCode::Runtime,
            ExitCode::Usage,
            ExitCode::LockConflict,
            ExitCode::DriftVerify,
            ExitCode::PolicyRefusal,
        ];
        for exit_code in codes {
            let message = format!("msg-{}", exit_code.error_code());
            let err = ControlError {
                exit_code,
                message: message.clone(),
            };
            let detail = ErrorDetail::from(err);
            assert_eq!(detail.exit_code, i32::from(exit_code));
            assert_eq!(detail.reason, exit_code.error_code());
            let status = status_from_error_detail(&detail);
            assert_eq!(status.code(), tonic::Code::Unknown);
            assert_eq!(status.message(), message.as_str());
            let parsed = error_detail_from_status(&status).expect("parse");
            assert_eq!(parsed, detail);
            let back = ControlError::try_from(parsed).expect("control");
            assert_eq!(back.exit_code, exit_code);
            assert_eq!(back.message, message);
            assert_eq!(back.exit_code.error_code(), detail.reason);
        }
    }

    #[test]
    fn resource_exhausted_reason_is_the_grpc_code() {
        let detail = resource_exhausted_detail("channel pool exhausted");
        let status = status_from_error_detail(&detail);
        assert_eq!(status.code(), tonic::Code::ResourceExhausted);
        assert_eq!(status.message(), "channel pool exhausted");
        let parsed = error_detail_from_status(&status).expect("parse");
        assert_eq!(parsed, detail);
        assert_eq!(parsed.reason, REASON_RESOURCE_EXHAUSTED);
    }

    #[test]
    fn status_without_details_does_not_invent_error_detail() {
        let status = tonic::Status::unknown("no details");
        assert!(matches!(
            error_detail_from_status(&status),
            Err(ConvertError::MissingErrorDetail)
        ));
    }

    #[test]
    fn invalid_error_detail_encoding_does_not_invent_error_detail() {
        let status = tonic::Status::with_details(
            tonic::Code::Unknown,
            "x",
            tonic::codegen::Bytes::from_static(&[0xff, 0xff]),
        );
        assert!(matches!(
            error_detail_from_status(&status),
            Err(ConvertError::InvalidErrorDetail)
        ));
    }

    #[test]
    fn unparseable_status_maps_to_runtime_on_the_control_path() {
        let err = control_error_from_status(tonic::Status::unknown("no details"));
        assert_eq!(err.exit_code, ExitCode::Runtime);
        assert_eq!(err.message, "no details");

        let err = control_error_from_status(tonic::Status::with_details(
            tonic::Code::Unknown,
            "x",
            tonic::codegen::Bytes::from_static(&[0xff, 0xff]),
        ));
        assert_eq!(err.exit_code, ExitCode::Runtime);
        assert_eq!(err.message, "x");
    }

    #[test]
    fn bad_exit_code_does_not_map_to_a_default() {
        let detail = ErrorDetail {
            exit_code: 99,
            reason: "nope".to_string(),
            message: "mystery".to_string(),
        };
        let status = status_from_error_detail(&detail);
        assert!(matches!(
            error_detail_from_status(&status),
            Err(ConvertError::UnknownExitCode(99))
        ));
    }

    #[test]
    fn edge_check_and_apply_reports_from_wire() {
        let check = EdgeCheckResponse {
            semver: "0.1.0".into(),
            proto_package: PROTO_PACKAGE.into(),
            fingerprint: "abc".into(),
            invariant_ok: false,
            drift: "bind-to-star".into(),
        };
        let report = EdgeApiReport::from(check);
        assert!(!report.invariant_ok);
        assert_eq!(report.fingerprint, "abc");
        let apply = EdgeApplyResponse {
            semver: "0.1.0".into(),
            proto_package: PROTO_PACKAGE.into(),
            noop: true,
            diff: String::new(),
        };
        let grab = GrabApplyReport::from(apply);
        assert!(grab.noop);
    }

    #[test]
    fn df_free_bytes_become_domain_bytes() {
        let response = DfResponse {
            semver: "0.1.0".to_string(),
            proto_package: PROTO_PACKAGE.to_string(),
            free_bytes: 42,
        };
        let bytes = Bytes::from(response);
        assert_eq!(bytes.get(), 42);
    }

    #[test]
    fn guard_preview_item_round_trip() {
        let domain = CoreGuardPreviewItem {
            hash: "abc".into(),
            state: "uploading".into(),
            ratio: 1.25,
            is_private: true,
            content_path: "/data/media/a.mkv".into(),
            save_path: "/data/torrents".into(),
            remote: Some(domain_ref("seedbox", "movies/a.mkv")),
        };
        let wire = GuardPreviewItem::try_from(&domain).expect("to wire");
        assert_eq!(wire.hash, "abc");
        assert_eq!(wire.state, "uploading");
        assert!((wire.ratio - 1.25).abs() < f64::EPSILON);
        assert!(wire.is_private);
        assert_eq!(wire.content_path, "/data/media/a.mkv");
        assert_eq!(wire.save_path, "/data/torrents");
        let back = CoreGuardPreviewItem::try_from(wire).expect("from wire");
        assert_eq!(back.hash, domain.hash);
        assert_eq!(back.remote, domain.remote);
        let empty = GuardPreviewItem {
            hash: "h".into(),
            state: "pausedDL".into(),
            ratio: 0.0,
            is_private: false,
            content_path: String::new(),
            save_path: String::new(),
            r#ref: None,
        };
        let parsed = CoreGuardPreviewItem::try_from(empty).expect("optional ref");
        assert!(parsed.remote.is_none());
    }

    #[test]
    fn delete_remote_result_rejects_unspecified_and_unknown() {
        assert!(matches!(
            DeleteRemoteOutcome::try_from(DeleteRemoteResult::Unspecified),
            Err(ConvertError::UnknownDeleteRemoteResult(0))
        ));
        let unknown = DeleteRemoteResponse {
            semver: "0.1.0".to_string(),
            proto_package: PROTO_PACKAGE.to_string(),
            result: 99,
        };
        assert!(matches!(
            DeleteRemoteOutcome::try_from(unknown),
            Err(ConvertError::UnknownDeleteRemoteResult(99))
        ));
        assert_eq!(
            DeleteRemoteOutcome::try_from(DeleteRemoteResult::Deleted).expect("deleted"),
            DeleteRemoteOutcome::Deleted
        );
        assert_eq!(
            DeleteRemoteOutcome::try_from(DeleteRemoteResult::SkippedSeeding).expect("skip"),
            DeleteRemoteOutcome::SkippedSeeding
        );
        let deleted = DeleteRemoteResponse {
            semver: "0.1.0".to_string(),
            proto_package: PROTO_PACKAGE.to_string(),
            result: 1,
        };
        assert_eq!(
            DeleteRemoteOutcome::try_from(deleted).expect("deleted response"),
            DeleteRemoteOutcome::Deleted
        );
        let skipped = DeleteRemoteResponse {
            semver: "0.1.0".to_string(),
            proto_package: PROTO_PACKAGE.to_string(),
            result: 2,
        };
        assert_eq!(
            DeleteRemoteOutcome::try_from(skipped).expect("skipped response"),
            DeleteRemoteOutcome::SkippedSeeding
        );
        let omitted = DeleteRemoteResponse {
            semver: "0.1.0".to_string(),
            proto_package: PROTO_PACKAGE.to_string(),
            result: 0,
        };
        assert!(matches!(
            DeleteRemoteOutcome::try_from(omitted),
            Err(ConvertError::UnknownDeleteRemoteResult(0))
        ));
    }

    #[test]
    fn missing_nested_refs_are_convert_errors() {
        assert!(matches!(
            CoreRemoteEntry::try_from(RemoteEntry {
                r#ref: None,
                len: 1,
                mtime: 0,
                nlink: 1,
            }),
            Err(ConvertError::MissingField("ref"))
        ));
        assert!(matches!(
            CoreRemoteRef::try_from(StatRequest { r#ref: None }),
            Err(ConvertError::MissingField("ref"))
        ));
        assert!(matches!(
            CoreRemoteRef::try_from(DeleteRemoteRequest { r#ref: None }),
            Err(ConvertError::MissingField("ref"))
        ));
        assert!(matches!(
            CoreRemoteRef::try_from(GetRangeRequest {
                r#ref: None,
                offset: 0,
                len: 1,
            }),
            Err(ConvertError::MissingField("ref"))
        ));
        assert!(matches!(
            CoreRemoteEntry::try_from(StatResponse { entry: None }),
            Err(ConvertError::MissingField("entry"))
        ));
        let list = ListResponse {
            entries: vec![RemoteEntry {
                r#ref: None,
                len: 1,
                mtime: 0,
                nlink: 1,
            }],
        };
        assert!(matches!(
            Vec::<CoreRemoteEntry>::try_from(list),
            Err(ConvertError::MissingField("ref"))
        ));
    }

    #[test]
    fn handshake_rejects_unknown_package() {
        assert!(check_handshake("mediaops.v2").is_err());
        assert!(check_handshake(PROTO_PACKAGE).is_ok());
        assert!(minor_skew_warning("0.2.0", "0.1.0").is_some());
        assert!(minor_skew_warning("0.1.1", "0.1.0").is_none());
        assert!(minor_skew_warning("0.1.0", "0.1.0").is_none());
        assert!(minor_skew_warning("1.0.0", "0.1.0").is_none());
    }

    #[test]
    fn unmonitor_request_renders_title_id() {
        let title = TitleId::movie("603").expect("title");
        let request = UnmonitorRequest::from(&title);
        assert_eq!(request.title_id, "movie:tmdb:603");
        assert_eq!(request.title_id, title.render());
        assert_eq!(TitleId::try_from(request).expect("parse"), title);
    }

    #[test]
    fn wanted_missing_response_parses_title_ids() {
        let response = WantedMissingResponse {
            semver: "0.1.0".into(),
            proto_package: PROTO_PACKAGE.into(),
            title_id: vec!["movie:tmdb:603".into(), "series:tvdb:79126".into()],
        };
        let ids = Vec::<TitleId>::try_from(response).expect("ids");
        assert_eq!(ids[0].render(), "movie:tmdb:603");
        assert_eq!(ids[1].render(), "series:tvdb:79126");
        let mixed = WantedMissingResponse {
            semver: "0.1.0".into(),
            proto_package: PROTO_PACKAGE.into(),
            title_id: vec!["not-a-title".into(), "movie:tmdb:603".into()],
        };
        let ids = Vec::<TitleId>::try_from(mixed).expect("skip bad");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].render(), "movie:tmdb:603");
    }

    #[test]
    fn hold_live_item_carries_release_id_verbatim() {
        let domain = CoreHoldLiveItem {
            key: HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::usenet("The.Matrix.1999.nzb").expect("release"),
            ),
            added_unix: 1_577_836_800,
            size: 1234,
            reason: "No files found are eligible for import".into(),
            remote: Some(domain_ref("seedbox", "The.Matrix.1999.mkv")),
            placement: Some(CorePlacement::movie("The.Matrix", 1999, "mkv")),
            output_path: Some("/data/_incoming/The.Matrix.1999".into()),
        };
        let wire = HoldLiveItem::from(&domain);
        assert_eq!(wire.title_id, "movie:tmdb:603");
        assert_eq!(wire.release_id, domain.key.release_id.as_str());
        assert_eq!(wire.added_unix, 1_577_836_800);
        assert_eq!(wire.size, 1234);
        assert_eq!(
            wire.remote.as_ref().expect("remote").rel_path,
            "The.Matrix.1999.mkv"
        );
        let back = CoreHoldLiveItem::try_from(wire).expect("from wire");
        assert_eq!(back.key, domain.key);
        assert_eq!(back.remote, domain.remote);
        assert_eq!(back.placement, domain.placement);
        assert!(
            back.output_path.is_none(),
            "outputPath stays off the wire; seedbox maps it to RemoteRef"
        );
        let key = HoldKey::new(
            TitleId::movie("603").expect("title"),
            ReleaseId::parse("deadbeef").expect("id"),
        );
        let reject = HoldRejectRequest::from(&key);
        assert_eq!(reject.title_id, "movie:tmdb:603");
        assert_eq!(reject.release_id, "deadbeef");
        assert_eq!(HoldKey::try_from(reject).expect("key"), key);
    }

    #[test]
    fn hold_live_item_remote_ref_round_trips() {
        let remote = domain_ref("seedbox", "downloads/The.Matrix.1999.mkv");
        let domain = CoreHoldLiveItem {
            key: HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::parse("deadbeef").expect("id"),
            ),
            added_unix: 0,
            size: 1,
            reason: "blocked".into(),
            remote: Some(remote.clone()),
            placement: Some(CorePlacement::movie("The.Matrix", 1999, "mkv")),
            output_path: None,
        };
        let wire = HoldLiveItem::from(&domain);
        assert!(
            wire.remote.is_some(),
            "mapped RemoteRef must not be dropped on the wire"
        );
        let back = CoreHoldLiveItem::try_from(wire).expect("from wire");
        assert_eq!(back.remote.as_ref().expect("remote"), &remote);
    }

    #[cfg(unix)]
    #[test]
    fn hold_live_item_non_utf8_remote_is_not_dropped() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let rel = PathBuf::from(OsString::from_vec(vec![0xff, b'a']));
        let domain_remote = CoreRemoteRef::from_wire_parts("seedbox".into(), rel).expect("shape");
        let domain = CoreHoldLiveItem {
            key: HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::parse("deadbeef").expect("id"),
            ),
            added_unix: 0,
            size: 1,
            reason: "blocked".into(),
            remote: Some(domain_remote),
            placement: None,
            output_path: None,
        };
        let wire = HoldLiveItem::from(&domain);
        assert!(
            wire.remote.is_some(),
            "lossy RemoteRef must still be emitted"
        );
        assert_eq!(wire.remote.as_ref().expect("remote").root_id, "seedbox");
        assert!(!wire.remote.as_ref().expect("remote").rel_path.is_empty());
    }

    #[test]
    fn bad_wire_title_id_is_convert_error() {
        let err = CoreHoldLiveItem::try_from(HoldLiveItem {
            title_id: "not-a-title".into(),
            release_id: "abc".into(),
            added_unix: 0,
            size: 0,
            reason: String::new(),
            remote: None,
            placement: None,
        })
        .unwrap_err();
        assert!(matches!(err, ConvertError::TitleId(_)), "{err}");
        let err = CoreHoldLiveItem::try_from(HoldLiveItem {
            title_id: "movie:tmdb:603".into(),
            release_id: String::new(),
            added_unix: 0,
            size: 0,
            reason: String::new(),
            remote: None,
            placement: None,
        })
        .unwrap_err();
        assert!(matches!(err, ConvertError::Hold(_)), "{err}");
        let runtime = convert_to_control(err);
        assert_eq!(runtime.exit_code, ExitCode::Runtime);
    }
}
