//! Home API objects (`mediaops.home.v1`). Pure: no tokio, no tonic, no sqlite.

use serde::{Deserialize, Serialize};

use crate::bytes::Bytes;
use crate::desired_state::{Grabber, PathRoot};
use crate::digest::Blake3Hex;
use crate::title_id::{TitleId, TitleKind};

/// Wire `apiVersion` for every Home object.
pub const HOME_API_VERSION: &str = "mediaops.home.v1";

/// Singleton Cluster name.
pub const CLUSTER_NAME: &str = "home";

/// Singleton Secret name that holds the seedbox endpoint.
pub const SECRET_NAME: &str = "seedbox";

/// gRPC metadata key identifying the caller. Hobby UDS has no mTLS.
pub const ACTOR_HEADER: &str = "x-mediaops-actor";

/// Pull Job retry budget.
pub const PULL_MAX_ATTEMPTS: u32 = 3;

/// Pull Job wall-clock deadline in seconds.
pub const PULL_DEADLINE_SECS: u64 = 30 * 60;

/// Node heartbeat interval in seconds.
pub const NODE_HEARTBEAT_SECS: u64 = 10;

/// Node is NotReady after this many seconds without a heartbeat.
pub const NODE_NOTREADY_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HomeError {
    #[error("{kind} `{name}` not found")]
    NotFound { kind: Kind, name: String },
    #[error("{kind} `{name}` already exists")]
    AlreadyExists { kind: Kind, name: String },
    #[error("{kind} `{name}` resourceVersion conflict")]
    Conflict { kind: Kind, name: String },
    #[error(
        "watch resourceVersion {requested} expired (oldest retained {oldest}); relist and watch from zero"
    )]
    Expired { requested: i64, oldest: i64 },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Denied(String),
}

/// Who is calling the Home API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Actor {
    Cli,
    Import,
    Controller,
    Scheduler,
    Inventory,
    Pull,
    Gateway,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Import => "import",
            Self::Controller => "controller",
            Self::Scheduler => "scheduler",
            Self::Inventory => "inventory",
            Self::Pull => "pull",
            Self::Gateway => "gateway",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "cli" => Ok(Self::Cli),
            "import" => Ok(Self::Import),
            "controller" => Ok(Self::Controller),
            "scheduler" => Ok(Self::Scheduler),
            "inventory" => Ok(Self::Inventory),
            "pull" => Ok(Self::Pull),
            "gateway" => Ok(Self::Gateway),
            other => Err(HomeError::Invalid(format!("unknown actor `{other}`"))),
        }
    }
}

/// API object kind. Match every variant; do not add a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Cluster,
    Secret,
    Title,
    Want,
    Job,
    Hold,
    RemoteFile,
    Node,
    Event,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cluster => "Cluster",
            Self::Secret => "Secret",
            Self::Title => "Title",
            Self::Want => "Want",
            Self::Job => "Job",
            Self::Hold => "Hold",
            Self::RemoteFile => "RemoteFile",
            Self::Node => "Node",
            Self::Event => "Event",
        }
    }

    pub fn store_key(self) -> &'static str {
        match self {
            Self::Cluster => "cluster",
            Self::Secret => "secret",
            Self::Title => "title",
            Self::Want => "want",
            Self::Job => "job",
            Self::Hold => "hold",
            Self::RemoteFile => "remotefile",
            Self::Node => "node",
            Self::Event => "event",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "Cluster" | "cluster" => Ok(Self::Cluster),
            "Secret" | "secret" => Ok(Self::Secret),
            "Title" | "title" => Ok(Self::Title),
            "Want" | "want" => Ok(Self::Want),
            "Job" | "job" => Ok(Self::Job),
            "Hold" | "hold" => Ok(Self::Hold),
            "RemoteFile" | "remotefile" | "Remote_file" => Ok(Self::RemoteFile),
            "Node" | "node" => Ok(Self::Node),
            "Event" | "event" => Ok(Self::Event),
            other => Err(HomeError::Invalid(format!("unknown kind `{other}`"))),
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Object identity and concurrency token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectMeta {
    pub name: String,
    pub uid: String,
    pub generation: i64,
    pub resource_version: i64,
}

/// One Home API object. `-o json` prints this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HomeObject {
    pub api_version: String,
    pub kind: Kind,
    pub metadata: ObjectMeta,
    pub spec: Spec,
    pub status: StatusBody,
}

impl HomeObject {
    pub fn new(kind: Kind, name: impl Into<String>, spec: Spec, status: StatusBody) -> Self {
        Self {
            api_version: HOME_API_VERSION.to_string(),
            kind,
            metadata: ObjectMeta {
                name: name.into(),
                uid: String::new(),
                generation: 0,
                resource_version: 0,
            },
            spec,
            status,
        }
    }

    /// Zero Secret fields that must never appear on CLI get.
    pub fn redact(&mut self) {
        if let Spec::Secret(secret) = &mut self.spec {
            secret.seedbox_address.clear();
        }
    }

    /// Parse a kubectl-style document. `kind` selects how `spec` / `status` decode.
    pub fn from_document(value: &serde_json::Value) -> Result<Self, HomeError> {
        reject_unknown(
            value,
            &[
                "apiVersion",
                "api_version",
                "kind",
                "metadata",
                "spec",
                "status",
            ],
        )?;
        let kind = Kind::parse(
            value
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| HomeError::Invalid("kind is required".into()))?,
        )?;
        let api_version = value
            .get("apiVersion")
            .or_else(|| value.get("api_version"))
            .and_then(|v| v.as_str())
            .unwrap_or(HOME_API_VERSION)
            .to_string();
        let metadata = value
            .get("metadata")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        reject_unknown(
            &metadata,
            &[
                "name",
                "uid",
                "generation",
                "resourceVersion",
                "resource_version",
            ],
        )?;
        let name = metadata
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let resource_version = metadata
            .get("resourceVersion")
            .or_else(|| metadata.get("resource_version"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let generation = metadata
            .get("generation")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let uid = metadata
            .get("uid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let spec_val = value.get("spec").cloned().unwrap_or(serde_json::json!({}));
        let status_val = value
            .get("status")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let spec = spec_from_value(kind, spec_val)?;
        let status = status_from_value(kind, status_val)?;
        Ok(Self {
            api_version,
            kind,
            metadata: ObjectMeta {
                name,
                uid,
                generation,
                resource_version,
            },
            spec,
            status,
        })
    }

    /// Admission validation. This is separate from decoding because reads may
    /// contain redacted Secrets or legacy observations requiring migration.
    pub fn validate(&self) -> Result<(), HomeError> {
        let invalid = |message: &str| HomeError::Invalid(message.into());
        if self.api_version != HOME_API_VERSION
            || self.spec.kind() != self.kind
            || self.status.kind() != self.kind
        {
            return Err(invalid("apiVersion, kind and body must agree"));
        }
        if self.metadata.name.is_empty()
            || self.metadata.name.contains('\0')
            || self.metadata.resource_version < 0
            || self.metadata.generation < 0
        {
            return Err(invalid(
                "valid object name and nonnegative metadata are required",
            ));
        }
        let title = |id: &str| TitleId::parse(id).map_err(|e| HomeError::Invalid(e.to_string()));
        let range = |len: u64, concurrency: u32| {
            if len == 0
                || len > crate::MAX_RANGE_LEN_MIB * Bytes::MIB
                || concurrency == 0
                || concurrency > 64
            {
                Err(invalid("rangeLen must be 1..64 MiB and concurrency 1..64"))
            } else {
                Ok(())
            }
        };
        match &self.spec {
            Spec::Cluster(s) => {
                if self.metadata.name != CLUSTER_NAME {
                    return Err(invalid("Cluster name must be home"));
                }
                range(s.range_len.get(), s.range_concurrency.unwrap_or(1))?;
                if !s.library_root.is_empty() {
                    absolute_path(&s.library_root)?;
                }
                let mut roots = std::collections::HashSet::new();
                for root in &s.roots {
                    absolute_path(&root.path)?;
                    if root.id.is_empty() || root.id.contains('/') || !roots.insert(&root.id) {
                        return Err(invalid(
                            "remote root ids must be unique nonempty components",
                        ));
                    }
                }
            }
            Spec::Secret(s) => {
                if self.metadata.name != SECRET_NAME {
                    return Err(invalid("Secret name must be seedbox"));
                }
                let (host, port) = s
                    .seedbox_address
                    .rsplit_once(':')
                    .ok_or_else(|| invalid("seedboxAddress must be host:port"))?;
                if host.is_empty()
                    || host.chars().any(char::is_whitespace)
                    || port.parse::<u16>().ok().filter(|p| *p > 0).is_none()
                {
                    return Err(invalid("seedboxAddress must be host:port"));
                }
                for pin in [&s.ca_sha256, &s.server_sha256, &s.client_sha256] {
                    if !pin.is_empty()
                        && (pin.len() != 64 || !pin.bytes().all(|c| c.is_ascii_hexdigit()))
                    {
                        return Err(invalid("TLS fingerprints must be SHA-256 hex"));
                    }
                }
            }
            Spec::Title(s) => {
                title(&s.title_id)?;
                if self.metadata.name != s.title_id {
                    return Err(invalid("Title name must equal titleId"));
                }
            }
            Spec::Want(s) => {
                title(&s.title_id)?;
                if self.metadata.name != s.title_id {
                    return Err(invalid("Want name must equal titleId"));
                }
            }
            Spec::Job(s) => {
                let id = title(&s.title_id)?;
                range(s.range_len, s.range_concurrency)?;
                absolute_path(&s.library_root)?;
                crate::RemoteRef::from_wire_parts(
                    s.remote_root.clone(),
                    s.remote_path.clone().into(),
                )
                .map_err(|e| HomeError::Invalid(e.to_string()))?;
                validate_placement(&id, &s.dest_rel)?;
                if s.file_len == 0
                    || (!s.node_name.is_empty() && s.node_name != WorkerKind::Pull.node_name())
                    || s.worker_kind != WorkerKind::Pull.as_str()
                {
                    return Err(invalid(
                        "Pull requires a file, pull worker kind and optional pull binding",
                    ));
                }
            }
            Spec::Hold(s) => {
                title(&s.title_id)?;
                if s.release_id.is_empty()
                    || self.metadata.name != format!("{}-{}", s.title_id, s.release_id)
                {
                    return Err(invalid("Hold name must identify its title and release"));
                }
            }
            Spec::Node(s) => {
                if self.metadata.name != s.worker_kind.node_name() {
                    return Err(invalid("Node name must equal workerKind"));
                }
            }
            Spec::RemoteFile | Spec::Event => {}
        }
        match &self.status {
            StatusBody::Title(s) => {
                let Spec::Title(spec) = &self.spec else {
                    unreachable!()
                };
                let id = title(&spec.title_id)?;
                let mut keys = std::collections::HashSet::new();
                for file in &s.observed_files() {
                    validate_placement(&id, &file.path)?;
                    let (_, placement) = crate::parse_placement(std::path::Path::new(&file.path))
                        .map_err(|e| HomeError::Invalid(e.to_string()))?;
                    if !keys.insert(placement.file_key()) {
                        return Err(invalid("duplicate Title file placement"));
                    }
                }
            }
            StatusBody::Job(s) => {
                let Spec::Job(spec) = &self.spec else {
                    unreachable!()
                };
                if s.attempts > PULL_MAX_ATTEMPTS
                    || s.bytes_done > spec.file_len
                    || s.started_unix < 0
                {
                    return Err(invalid("invalid Job progress"));
                }
            }
            StatusBody::RemoteFile(s) => {
                crate::RemoteRef::from_wire_parts(s.root_id.clone(), s.rel_path.clone().into())
                    .map_err(|e| HomeError::Invalid(e.to_string()))?;
                if s.list_generation <= 0 || (s.parse_ok && title(&s.title_id).is_err()) {
                    return Err(invalid(
                        "RemoteFile requires a positive listGeneration and valid parsed title",
                    ));
                }
            }
            StatusBody::Node(s) => {
                if s.last_heartbeat_unix < 0 || s.list_generation < 0 || s.list_completed_unix < 0 {
                    return Err(invalid("invalid Node observation"));
                }
            }
            StatusBody::Hold(s) => {
                if s.list_generation < 0 {
                    return Err(HomeError::Invalid(
                        "Hold inventory generation must not be negative".into(),
                    ));
                }
                if !s.remote_root.is_empty() || !s.remote_path.is_empty() {
                    crate::RemoteRef::from_wire_parts(
                        s.remote_root.clone(),
                        s.remote_path.clone().into(),
                    )
                    .map_err(|e| HomeError::Invalid(e.to_string()))?;
                }
                if let Some(placement) = &s.placement {
                    let Spec::Hold(spec) = &self.spec else {
                        unreachable!()
                    };
                    crate::render(&title(&spec.title_id)?, placement)
                        .map_err(|e| HomeError::Invalid(e.to_string()))?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// TOML or JSON document. YAML is accepted when it is JSON.
    pub fn from_bytes(raw: &[u8]) -> Result<Self, HomeError> {
        let text = std::str::from_utf8(raw)
            .map_err(|_| HomeError::Invalid("object document is not utf-8".into()))?;
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            let value: serde_json::Value =
                serde_json::from_str(trimmed).map_err(|e| HomeError::Invalid(e.to_string()))?;
            return Self::from_document(&value);
        }
        let value: toml::Value =
            toml::from_str(text).map_err(|e| HomeError::Invalid(e.to_string()))?;
        let json = snake_keys_to_camel(toml_to_json(value)?);
        Self::from_document(&json)
    }
}

fn reject_unknown(value: &serde_json::Value, allowed: &[&str]) -> Result<(), HomeError> {
    let fields = value
        .as_object()
        .ok_or_else(|| HomeError::Invalid("object expected".into()))?;
    if let Some(key) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(HomeError::Invalid(format!("unknown field `{key}`")));
    }
    Ok(())
}

fn absolute_path(raw: &str) -> Result<(), HomeError> {
    let path = std::path::Path::new(raw);
    if !path.is_absolute()
        || raw.contains('\0')
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(HomeError::Invalid(
            "absolute path without parent traversal required".into(),
        ));
    }
    Ok(())
}

fn validate_placement(id: &TitleId, raw: &str) -> Result<(), HomeError> {
    let path = std::path::Path::new(raw);
    let (parsed, placement) =
        crate::parse_placement(path).map_err(|e| HomeError::Invalid(e.to_string()))?;
    if (id.is_key() && parsed != *id)
        || id.kind() != parsed.kind()
        || crate::render(id, &placement).map_err(|e| HomeError::Invalid(e.to_string()))? != path
    {
        return Err(HomeError::Invalid(
            "destination must be a canonical PathSchema path for the TitleId".into(),
        ));
    }
    Ok(())
}

fn toml_to_json(value: toml::Value) -> Result<serde_json::Value, HomeError> {
    serde_json::to_value(value).map_err(|e| HomeError::Invalid(e.to_string()))
}

fn snake_keys_to_camel(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, val) in map {
                out.insert(to_camel(&key), snake_keys_to_camel(val));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(snake_keys_to_camel).collect())
        }
        other => other,
    }
}

fn to_camel(key: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for ch in key.chars() {
        if ch == '_' {
            upper = true;
            continue;
        }
        if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn spec_from_value(kind: Kind, value: serde_json::Value) -> Result<Spec, HomeError> {
    let err = |e: serde_json::Error| HomeError::Invalid(e.to_string());
    Ok(match kind {
        Kind::Cluster => Spec::Cluster(serde_json::from_value(value).map_err(err)?),
        Kind::Secret => Spec::Secret(serde_json::from_value(value).map_err(err)?),
        Kind::Title => Spec::Title(serde_json::from_value(value).map_err(err)?),
        Kind::Want => Spec::Want(serde_json::from_value(value).map_err(err)?),
        Kind::Job => Spec::Job(serde_json::from_value(value).map_err(err)?),
        Kind::Hold => Spec::Hold(serde_json::from_value(value).map_err(err)?),
        Kind::RemoteFile => Spec::RemoteFile,
        Kind::Node => Spec::Node(serde_json::from_value(value).map_err(err)?),
        Kind::Event => Spec::Event,
    })
}

fn status_from_value(kind: Kind, value: serde_json::Value) -> Result<StatusBody, HomeError> {
    let err = |e: serde_json::Error| HomeError::Invalid(e.to_string());
    Ok(match kind {
        Kind::Cluster => StatusBody::Cluster(serde_json::from_value(value).map_err(err)?),
        Kind::Secret => StatusBody::Secret,
        Kind::Title => StatusBody::Title(serde_json::from_value(value).map_err(err)?),
        Kind::Want => StatusBody::Want(serde_json::from_value(value).map_err(err)?),
        Kind::Job => StatusBody::Job(serde_json::from_value(value).map_err(err)?),
        Kind::Hold => StatusBody::Hold(serde_json::from_value(value).map_err(err)?),
        Kind::RemoteFile => StatusBody::RemoteFile(serde_json::from_value(value).map_err(err)?),
        Kind::Node => StatusBody::Node(serde_json::from_value(value).map_err(err)?),
        Kind::Event => StatusBody::Event(serde_json::from_value(value).map_err(err)?),
    })
}

/// Desired state. Match every variant with [`Kind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Spec {
    Cluster(ClusterSpec),
    Secret(SecretSpec),
    Title(TitleSpec),
    Want(WantSpec),
    Job(JobSpec),
    Hold(HoldSpec),
    RemoteFile,
    Node(NodeSpec),
    Event,
}

/// Observed state. Match every variant with [`Kind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusBody {
    Cluster(ClusterStatus),
    Secret,
    Title(TitleStatus),
    Want(WantStatus),
    Job(JobStatus),
    Hold(HoldStatus),
    RemoteFile(RemoteFileStatus),
    Node(NodeStatus),
    Event(EventStatus),
}

impl Spec {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Cluster(_) => Kind::Cluster,
            Self::Secret(_) => Kind::Secret,
            Self::Title(_) => Kind::Title,
            Self::Want(_) => Kind::Want,
            Self::Job(_) => Kind::Job,
            Self::Hold(_) => Kind::Hold,
            Self::RemoteFile => Kind::RemoteFile,
            Self::Node(_) => Kind::Node,
            Self::Event => Kind::Event,
        }
    }

    pub fn empty(kind: Kind) -> Self {
        match kind {
            Kind::Cluster => Self::Cluster(ClusterSpec::default()),
            Kind::Secret => Self::Secret(SecretSpec::default()),
            Kind::Title => Self::Title(TitleSpec::default()),
            Kind::Want => Self::Want(WantSpec::default()),
            Kind::Job => Self::Job(JobSpec::default()),
            Kind::Hold => Self::Hold(HoldSpec::default()),
            Kind::RemoteFile => Self::RemoteFile,
            Kind::Node => Self::Node(NodeSpec::default()),
            Kind::Event => Self::Event,
        }
    }

    /// True for a kind whose whole payload is status. Apply preserves the
    /// stored status for every other kind; these have no spec to replace, so
    /// refusing the incoming status would make them write-once.
    pub fn is_status_only(&self) -> bool {
        matches!(self, Self::RemoteFile | Self::Event)
    }
}

impl StatusBody {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Cluster(_) => Kind::Cluster,
            Self::Secret => Kind::Secret,
            Self::Title(_) => Kind::Title,
            Self::Want(_) => Kind::Want,
            Self::Job(_) => Kind::Job,
            Self::Hold(_) => Kind::Hold,
            Self::RemoteFile(_) => Kind::RemoteFile,
            Self::Node(_) => Kind::Node,
            Self::Event(_) => Kind::Event,
        }
    }
    pub fn empty(kind: Kind) -> Self {
        match kind {
            Kind::Cluster => Self::Cluster(ClusterStatus::default()),
            Kind::Secret => Self::Secret,
            Kind::Title => Self::Title(TitleStatus::default()),
            Kind::Want => Self::Want(WantStatus::default()),
            Kind::Job => Self::Job(JobStatus::default()),
            Kind::Hold => Self::Hold(HoldStatus::default()),
            Kind::RemoteFile => Self::RemoteFile(RemoteFileStatus::default()),
            Kind::Node => Self::Node(NodeStatus::default()),
            Kind::Event => Self::Event(EventStatus::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterSpec {
    pub max_copy: Bytes,
    pub min_free: Bytes,
    pub range_len: Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_concurrency: Option<u32>,
    pub grabber: Grabber,
    #[serde(default)]
    pub lock: bool,
    #[serde(default)]
    pub encode_pause: bool,
    #[serde(default)]
    pub library_root: String,
    #[serde(default)]
    pub roots: Vec<PathRoot>,
}

impl Default for ClusterSpec {
    fn default() -> Self {
        Self {
            max_copy: Bytes::new(0),
            min_free: Bytes::new(0),
            range_len: Bytes::new(Bytes::MIB),
            range_concurrency: None,
            grabber: Grabber::None,
            lock: false,
            encode_pause: false,
            library_root: String::new(),
            roots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterStatus {
    #[serde(default)]
    pub accepted_generation: i64,
    #[serde(default)]
    pub reconcile_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSpec {
    pub seedbox_address: String,
    #[serde(default)]
    pub ca_sha256: String,
    #[serde(default)]
    pub server_sha256: String,
    #[serde(default)]
    pub client_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleSpec {
    #[serde(default)]
    pub title_id: String,
    #[serde(default)]
    pub desired_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleStatus {
    /// One durable proof per placement. Legacy single-file fields remain readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<TitleFileStatus>,
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_b3: Option<Blake3Hex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_b3: Option<Blake3Hex>,
    #[serde(default)]
    pub drifted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleFileStatus {
    pub path: String,
    pub install_b3: Blake3Hex,
    pub current_b3: Blake3Hex,
    #[serde(default)]
    pub drifted: bool,
}

impl TitleStatus {
    pub fn observed_files(&self) -> Vec<TitleFileStatus> {
        let mut files = self.files.clone();
        if !self.path.is_empty()
            && !files.iter().any(|f| f.path == self.path)
            && let (Some(install_b3), Some(current_b3)) = (&self.install_b3, &self.current_b3)
        {
            files.push(TitleFileStatus {
                path: self.path.clone(),
                install_b3: install_b3.clone(),
                current_b3: current_b3.clone(),
                drifted: self.drifted,
            });
        }
        files
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WantSpec {
    pub title_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum WantPhase {
    #[default]
    Open,
    Satisfied,
    Dropped,
}

impl WantPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Satisfied => "satisfied",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "open" => Ok(Self::Open),
            "satisfied" => Ok(Self::Satisfied),
            "dropped" => Ok(Self::Dropped),
            other => Err(HomeError::Invalid(format!("unknown Want phase `{other}`"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WantStatus {
    #[serde(default)]
    pub phase: WantPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum HomeJobKind {
    #[default]
    Pull,
}

impl HomeJobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pull => "pull",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "pull" => Ok(Self::Pull),
            other => Err(HomeError::Invalid(format!("unknown Job kind `{other}`"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobSpec {
    /// Exact Hold object that authorized this Job; empty means its Title's Want.
    #[serde(default)]
    pub hold_name: String,
    #[serde(default)]
    pub library_root: String,
    #[serde(default = "one")]
    pub range_concurrency: u32,
    pub kind: HomeJobKind,
    #[serde(default)]
    pub title_id: String,
    #[serde(default)]
    pub remote_root: String,
    #[serde(default)]
    pub remote_path: String,
    #[serde(default)]
    pub dest_rel: String,
    #[serde(default)]
    pub file_len: u64,
    #[serde(default)]
    pub range_len: u64,
    #[serde(default)]
    pub max_copy: u64,
    #[serde(default)]
    pub min_free: u64,
    #[serde(default)]
    pub node_name: String,
    #[serde(default)]
    pub worker_kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum JobPhase {
    #[default]
    Pending,
    Pulling,
    Verifying,
    Installed,
    Refused,
    Failed,
}

impl JobPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Pulling => "pulling",
            Self::Verifying => "verifying",
            Self::Installed => "installed",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "pending" => Ok(Self::Pending),
            "pulling" => Ok(Self::Pulling),
            "verifying" => Ok(Self::Verifying),
            "installed" => Ok(Self::Installed),
            "refused" => Ok(Self::Refused),
            "failed" => Ok(Self::Failed),
            other => Err(HomeError::Invalid(format!("unknown Job phase `{other}`"))),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Installed | Self::Refused | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobStatus {
    /// Persisted across retries and process restarts.
    #[serde(default)]
    pub started_unix: i64,
    /// Saved before installation so recovery can prove an already placed file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_b3: Option<Blake3Hex>,
    pub phase: JobPhase,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum HoldDecisionSpec {
    #[default]
    Empty,
    Approved,
    Rejected,
}

impl HoldDecisionSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "" | "empty" => Ok(Self::Empty),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            other => Err(HomeError::Invalid(format!(
                "unknown Hold decision `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HoldSpec {
    #[serde(default)]
    pub title_id: String,
    #[serde(default)]
    pub release_id: String,
    #[serde(default)]
    pub decision: HoldDecisionSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HoldStatus {
    /// Only observations in the inventory Node's committed generation are live.
    #[serde(default)]
    pub list_generation: i64,
    #[serde(default)]
    pub rejection_observed: bool,
    #[serde(default)]
    pub remote_root: String,
    #[serde(default)]
    pub remote_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<crate::Placement>,
    #[serde(default)]
    pub added_unix: i64,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub release: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteFileStatus {
    #[serde(default)]
    pub root_id: String,
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub len: u64,
    #[serde(default)]
    pub parse_ok: bool,
    #[serde(default)]
    pub title_id: String,
    #[serde(default)]
    pub list_generation: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkerKind {
    #[default]
    Pull,
    Scheduler,
    Inventory,
}

impl WorkerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Scheduler => "scheduler",
            Self::Inventory => "inventory",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "pull" => Ok(Self::Pull),
            "scheduler" => Ok(Self::Scheduler),
            "inventory" => Ok(Self::Inventory),
            other => Err(HomeError::Invalid(format!("unknown workerKind `{other}`"))),
        }
    }

    pub fn node_name(self) -> &'static str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeSpec {
    pub worker_kind: WorkerKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeStatus {
    /// Last completely published listing; zero means no successful listing yet.
    #[serde(default)]
    pub list_generation: i64,
    #[serde(default)]
    pub list_completed_unix: i64,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub last_heartbeat_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventStatus {
    #[serde(default)]
    pub involved_kind: String,
    #[serde(default)]
    pub involved_name: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub ts: i64,
}

/// Write operation used by admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeOp {
    Get,
    List,
    Watch,
    Apply,
    PatchSpec,
    PatchStatus,
    PatchBind,
    Delete,
    Reconcile,
}

/// Who may write which kind. Controllers writing through the store skip this.
pub fn admit(actor: Actor, op: HomeOp, kind: Kind) -> Result<(), HomeError> {
    let ok = matches!(
        (op, kind, actor),
        (HomeOp::Get | HomeOp::List | HomeOp::Watch, _, _)
            | (
                HomeOp::Reconcile,
                Kind::Cluster,
                Actor::Cli | Actor::Import | Actor::Controller
            )
            | (
                HomeOp::Apply,
                Kind::Cluster | Kind::Secret | Kind::Want | Kind::Title,
                Actor::Cli | Actor::Import | Actor::Controller,
            )
            | (HomeOp::Apply, Kind::Hold, Actor::Inventory)
            | (HomeOp::Apply, Kind::Hold, Actor::Import)
            | (HomeOp::Apply, Kind::RemoteFile, Actor::Inventory)
            | (
                HomeOp::Apply,
                Kind::Node,
                Actor::Scheduler | Actor::Inventory | Actor::Pull
            )
            | (HomeOp::Apply, Kind::Event, _)
            | (HomeOp::Apply, Kind::Job, Actor::Controller)
            | (HomeOp::PatchSpec, Kind::Hold, Actor::Cli | Actor::Import)
            | (
                HomeOp::PatchSpec,
                Kind::Want | Kind::Title | Kind::Cluster | Kind::Secret,
                Actor::Cli | Actor::Import,
            )
            | (HomeOp::PatchBind, Kind::Job, Actor::Scheduler)
            | (
                HomeOp::PatchStatus,
                Kind::Job | Kind::Title,
                Actor::Pull | Actor::Controller
            )
            | (HomeOp::PatchStatus, Kind::Title, Actor::Import)
            | (
                HomeOp::PatchStatus,
                Kind::Hold | Kind::RemoteFile,
                Actor::Inventory | Actor::Controller,
            )
            | (
                HomeOp::PatchStatus,
                Kind::Node,
                Actor::Scheduler | Actor::Inventory | Actor::Pull
            )
            | (
                HomeOp::PatchStatus,
                Kind::Want | Kind::Cluster,
                Actor::Controller
            )
            | (
                HomeOp::Delete,
                Kind::Want | Kind::Job | Kind::Hold | Kind::Event,
                Actor::Cli | Actor::Import,
            )
            | (HomeOp::Delete, Kind::RemoteFile, Actor::Inventory)
            | (
                HomeOp::Delete,
                Kind::Node,
                Actor::Scheduler | Actor::Inventory | Actor::Pull | Actor::Cli,
            )
    );
    if ok {
        Ok(())
    } else {
        Err(HomeError::Denied(format!(
            "{} may not {} {}",
            actor.as_str(),
            match op {
                HomeOp::Get => "get",
                HomeOp::List => "list",
                HomeOp::Watch => "watch",
                HomeOp::Apply => "apply",
                HomeOp::PatchSpec => "patch spec of",
                HomeOp::PatchStatus => "patch status of",
                HomeOp::PatchBind => "bind",
                HomeOp::Delete => "delete",
                HomeOp::Reconcile => "reconcile",
            },
            kind
        )))
    }
}

/// Album Jobs bind before movie, then series.
pub fn bind_priority(title_id: &TitleId) -> u8 {
    match title_id.kind() {
        TitleKind::Album => 0,
        TitleKind::Movie => 1,
        TitleKind::Series => 2,
    }
}

/// Whether a new Pull of `file_len` fits remaining budget after already-bound bytes.
pub fn pull_fits(
    free: u64,
    min_free: u64,
    max_copy: u64,
    already_bound: u64,
    file_len: u64,
) -> bool {
    let Some(required) = already_bound.checked_add(file_len) else {
        return false;
    };
    if max_copy > 0 && required > max_copy {
        return false;
    }
    free.checked_sub(required)
        .is_some_and(|remaining| remaining >= min_free)
}

fn one() -> u32 {
    1
}

/// A Node is usable only when it says ready *and* its heartbeat is fresh.
/// Every binder and every controller must agree on this, or a killed worker
/// keeps collecting work it will never do.
pub fn node_is_ready(ready: bool, last_heartbeat_unix: i64, now_unix: i64) -> bool {
    ready
        && now_unix >= last_heartbeat_unix
        && now_unix.saturating_sub(last_heartbeat_unix) < NODE_NOTREADY_SECS as i64
}

/// Stable RemoteFile object name: `root_id` + `/` + rel path.
pub fn remote_file_name(root_id: &str, rel_path: &str) -> String {
    format!("{root_id}/{rel_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_watermark_and_overflow_never_admit_more_bytes_than_free() {
        assert!(!pull_fits(5, 0, 0, 0, 6));
        assert!(!pull_fits(u64::MAX, 0, 0, u64::MAX, 1));
        assert!(pull_fits(5, 0, 0, 1, 4));
    }

    #[test]
    fn documents_reject_unknown_fields_and_invalid_write_shapes() {
        let document = serde_json::json!({"apiVersion": HOME_API_VERSION, "kind":"Want",
            "metadata":{"name":"movie:tmdb:603"}, "spec":{"titleId":"movie:tmdb:603"}});
        HomeObject::from_document(&document)
            .expect("decode")
            .validate()
            .expect("valid");
        for part in ["top", "metadata", "spec", "status"] {
            let mut bad = document.clone();
            if part == "top" {
                bad["typo"] = true.into();
            } else {
                bad[part]["typo"] = true.into();
            }
            assert!(
                HomeObject::from_document(&bad).is_err(),
                "unknown field in {part}"
            );
        }
        let mut wrong_version = HomeObject::from_document(&document).expect("decode");
        wrong_version.api_version = "future".into();
        assert!(wrong_version.validate().is_err());
        let mut wrong_name = HomeObject::from_document(&document).expect("decode");
        wrong_name.metadata.name = "movie:tmdb:604".into();
        assert!(wrong_name.validate().is_err());
        let mut cluster = HomeObject::new(
            Kind::Cluster,
            CLUSTER_NAME,
            Spec::Cluster(ClusterSpec::default()),
            StatusBody::empty(Kind::Cluster),
        );
        if let Spec::Cluster(s) = &mut cluster.spec {
            s.range_len = Bytes::new(0);
        }
        assert!(cluster.validate().is_err());
        if let Spec::Cluster(s) = &mut cluster.spec {
            s.range_len = Bytes::new(1);
            s.library_root = "/library/../elsewhere".into();
        }
        assert!(cluster.validate().is_err());
    }

    #[test]
    fn cli_cannot_apply_remotefile_or_job() {
        assert!(admit(Actor::Cli, HomeOp::Apply, Kind::RemoteFile).is_err());
        assert!(admit(Actor::Cli, HomeOp::Apply, Kind::Job).is_err());
        assert!(admit(Actor::Inventory, HomeOp::Apply, Kind::RemoteFile).is_ok());
        assert!(admit(Actor::Controller, HomeOp::Apply, Kind::Job).is_ok());
    }

    #[test]
    fn scheduler_binds_jobs_only() {
        assert!(admit(Actor::Scheduler, HomeOp::PatchBind, Kind::Job).is_ok());
        assert!(admit(Actor::Cli, HomeOp::PatchBind, Kind::Job).is_err());
        assert!(admit(Actor::Pull, HomeOp::PatchStatus, Kind::Job).is_ok());
    }

    #[test]
    fn secret_redact_clears_address() {
        let mut obj = HomeObject::new(
            Kind::Secret,
            SECRET_NAME,
            Spec::Secret(SecretSpec {
                seedbox_address: "seedbox:50051".into(),
                ..SecretSpec::default()
            }),
            StatusBody::Secret,
        );
        obj.redact();
        match obj.spec {
            Spec::Secret(s) => assert!(s.seedbox_address.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn music_first_priority() {
        let album = TitleId::parse("album:key:tool.lateralus").expect("a");
        let movie = TitleId::parse("movie:key:thematrix.1999").expect("m");
        let series = TitleId::parse("series:key:mrrobot.2015").expect("s");
        assert!(bind_priority(&album) < bind_priority(&movie));
        assert!(bind_priority(&movie) < bind_priority(&series));
    }

    #[test]
    fn pull_fits_respects_max_copy_and_min_free() {
        assert!(pull_fits(100, 10, 50, 0, 40));
        assert!(!pull_fits(100, 10, 50, 0, 51));
        assert!(!pull_fits(20, 10, 100, 0, 15));
        assert!(!pull_fits(100, 10, 50, 20, 40));
    }

    #[test]
    fn document_toml_want_round_trip() {
        let obj = HomeObject::from_bytes(
            b"kind = \"Want\"\n[metadata]\nname = \"movie:tmdb:603\"\n[spec]\ntitle_id = \"movie:tmdb:603\"\n",
        )
        .expect("toml");
        assert_eq!(obj.kind, Kind::Want);
        match obj.spec {
            Spec::Want(s) => assert_eq!(s.title_id, "movie:tmdb:603"),
            other => panic!("{other:?}"),
        }
    }
}
