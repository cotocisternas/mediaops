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
    ExitCode, GrabApplyReport, KeyPresence, RemoteEntry as CoreRemoteEntry,
    RemoteRef as CoreRemoteRef, TitleId, WalkerError,
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
        let rel_path = value
            .rel_path()
            .to_str()
            .ok_or(ConvertError::NonUtf8Path)?
            .to_owned();
        Ok(Self {
            root_id: value.root_id().to_owned(),
            rel_path,
        })
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
            DeleteRemoteResult::Unspecified => {
                Err(ConvertError::UnknownDeleteRemoteResult(value as i32))
            }
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
            check_handshake(&inner.proto_package)?;
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
            check_handshake(&response.into_inner().proto_package)?;
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
            check_handshake(&inner.proto_package)?;
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
            check_handshake(&inner.proto_package)?;
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
            check_handshake(&inner.proto_package)?;
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
            check_handshake(&inner.proto_package)?;
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
            check_handshake(&inner.proto_package)?;
            Ok(KeyPresence::from(inner))
        })
    }

    fn guard_preview(&self) -> BoxFuture<'_, Result<(), ControlError>> {
        let mut client = self.inner.clone();
        Box::pin(async move {
            let response = client
                .guard_preview(GuardPreviewRequest {})
                .await
                .map_err(control_error_from_status)?;
            check_handshake(&response.into_inner().proto_package)?;
            Ok(())
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
    }

    #[test]
    fn unmonitor_request_renders_title_id() {
        let title = TitleId::movie("603").expect("title");
        let request = UnmonitorRequest::from(&title);
        assert_eq!(request.title_id, "movie:tmdb:603");
        assert_eq!(request.title_id, title.render());
    }
}
