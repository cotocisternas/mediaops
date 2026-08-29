//! Domain types for mediaops.
//!
//! [`TitleId`] and [`pathschema`] are pure functions. [`walker`] and [`install`]
//! may use caller-supplied filesystem roots (tempdir fixtures). There is still
//! no tokio runtime and no network.

mod install;
pub mod pathschema;
mod title_id;
pub mod walker;

pub use install::{
    InstallError, VerifiedConvertingHandle, VerifiedStagingHandle, install, replace,
};
pub use pathschema::{
    GRAMMAR_VERSION, PathSchemaError, Placement, RejectBin, parse, render, staging_path,
    strip_scene_tags,
};
pub use title_id::{TitleId, TitleIdError, TitleKind, TitleSource};
pub use walker::{Allowlist, RemoteEntry, RemoteRef, WalkerError};

use std::process::{ExitCode as ProcessExitCode, Termination};

use serde::{Deserialize, Serialize};

/// Process exit taxonomy owned by `core` (AD-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    Ok = 0,
    Runtime = 1,
    Usage = 2,
    LockConflict = 3,
    DriftVerify = 4,
    PolicyRefusal = 5,
}

impl ExitCode {
    /// Stable `error.code` string for the JSON envelope.
    pub const fn error_code(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Runtime => "runtime",
            Self::Usage => "usage",
            Self::LockConflict => "lock_conflict",
            Self::DriftVerify => "drift_verify",
            Self::PolicyRefusal => "policy_refusal",
        }
    }
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> Self {
        code as i32
    }
}

impl Termination for ExitCode {
    fn report(self) -> ProcessExitCode {
        ProcessExitCode::from(i32::from(self) as u8)
    }
}

/// `{ok, data, error:{code, message}}` result envelope (AD-18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<EnvelopeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct EnvelopeError {
    pub code: String,
    pub message: String,
}

impl<T> Envelope<T> {
    pub fn ok(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(code: ExitCode, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(EnvelopeError {
                code: code.error_code().to_string(),
                message: message.into(),
            }),
        }
    }
}

/// Skeleton identity payload for the composition-root binaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub version: String,
}

impl Identity {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

pub fn identity_line(name: &str, version: &str) -> String {
    format!("{name} {version}")
}

pub fn render_success_json(name: &str, version: &str) -> Result<String, serde_json::Error> {
    serde_json::to_string(&Envelope::ok(Identity::new(name, version)))
}

pub fn render_error_json(code: ExitCode, message: &str) -> Result<String, serde_json::Error> {
    serde_json::to_string(&Envelope::<Identity>::err(code, message))
}

/// Reserved capability tokens (CAP-11). No LLM runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityToken {
    ReadFs,
    ProbeMedia,
    ArrGet,
    ArrPost,
    SshExecAllowlist,
}

pub fn capability_label(token: CapabilityToken) -> &'static str {
    match token {
        CapabilityToken::ReadFs => "read_fs",
        CapabilityToken::ProbeMedia => "probe_media",
        CapabilityToken::ArrGet => "arr_get",
        CapabilityToken::ArrPost => "arr_post",
        CapabilityToken::SshExecAllowlist => "ssh_exec_allowlist",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_discriminants() {
        assert_eq!(i32::from(ExitCode::Ok), 0);
        assert_eq!(i32::from(ExitCode::Runtime), 1);
        assert_eq!(i32::from(ExitCode::Usage), 2);
        assert_eq!(i32::from(ExitCode::LockConflict), 3);
        assert_eq!(i32::from(ExitCode::DriftVerify), 4);
        assert_eq!(i32::from(ExitCode::PolicyRefusal), 5);
    }

    #[test]
    fn exit_code_match_is_exhaustive() {
        let codes = [
            ExitCode::Ok,
            ExitCode::Runtime,
            ExitCode::Usage,
            ExitCode::LockConflict,
            ExitCode::DriftVerify,
            ExitCode::PolicyRefusal,
        ];
        for code in codes {
            let n = match code {
                ExitCode::Ok => 0,
                ExitCode::Runtime => 1,
                ExitCode::Usage => 2,
                ExitCode::LockConflict => 3,
                ExitCode::DriftVerify => 4,
                ExitCode::PolicyRefusal => 5,
            };
            assert_eq!(n, i32::from(code));
        }
    }

    #[test]
    fn envelope_ok_round_trip() {
        let envelope = Envelope::ok(Identity::new("mediaops", "0.1.0"));
        let encoded = serde_json::to_string(&envelope).expect("serialize");
        let decoded: Envelope<Identity> = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, envelope);

        let value: serde_json::Value = serde_json::from_str(&encoded).expect("value");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["name"], "mediaops");
        assert_eq!(value["data"]["version"], "0.1.0");
        assert_eq!(value.get("error"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn envelope_usage_error_round_trip() {
        let envelope = Envelope::<Identity>::err(ExitCode::Usage, "unexpected argument");
        let encoded = serde_json::to_string(&envelope).expect("serialize");
        let decoded: Envelope<Identity> = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, envelope);

        let value: serde_json::Value = serde_json::from_str(&encoded).expect("value");
        assert_eq!(value["ok"], false);
        assert_eq!(value.get("data"), Some(&serde_json::Value::Null));
        assert_eq!(value["error"]["code"], "usage");
        assert_eq!(value["error"]["message"], "unexpected argument");
    }

    #[test]
    fn capability_token_match_is_exhaustive() {
        let tokens = [
            CapabilityToken::ReadFs,
            CapabilityToken::ProbeMedia,
            CapabilityToken::ArrGet,
            CapabilityToken::ArrPost,
            CapabilityToken::SshExecAllowlist,
        ];
        for token in tokens {
            let _ = match token {
                CapabilityToken::ReadFs
                | CapabilityToken::ProbeMedia
                | CapabilityToken::ArrGet
                | CapabilityToken::ArrPost
                | CapabilityToken::SshExecAllowlist => capability_label(token),
            };
        }
        assert_eq!(tokens.len(), 5);
    }
}
