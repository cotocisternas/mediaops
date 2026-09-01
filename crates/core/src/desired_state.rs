//! Desired-state document. Serde TOML, `deny_unknown_fields`, `schema_version` 1.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;

use crate::bytes::Bytes;
use crate::probe::UnderlayMode;
use crate::provider::ProviderKind;

const SCHEMA_VERSION: u32 = 1;
const SHA256_HEX_LEN: usize = 64;

/// First-pass peek: extra fields are ignored so a future version can be
/// diagnosed as `UnsupportedVersion` instead of an unknown-field parse error.
#[derive(Debug, Deserialize)]
struct SchemaVersionToml {
    schema_version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesiredStateToml {
    schema_version: u32,
    max_copy_gib: u64,
    min_free_gib: u64,
    range_len_mib: u64,
    max_nvenc: u32,
    lock: bool,
    #[serde(default)]
    grabber: Grabber,
    #[serde(default)]
    provider: Option<ProviderKind>,
    #[serde(default)]
    seedbox_address: Option<String>,
    #[serde(default)]
    underlay: UnderlayMode,
    #[serde(default)]
    tls: Option<TlsIdentity>,
    #[serde(default)]
    paths: PathsToml,
    #[serde(default)]
    grab: GrabToml,
    #[serde(default)]
    edge: Option<Edge>,
    #[serde(default)]
    pins: Pins,
}

/// *arr is optional. `none` means no live grabber HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grabber {
    #[default]
    None,
    Servarr,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsToml {
    #[serde(default)]
    roots: Vec<PathRoot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathRoot {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrabToml {
    #[serde(default)]
    indexers: Vec<GrabIndexer>,
    #[serde(default)]
    download_clients: Vec<GrabDownloadClient>,
    #[serde(default)]
    custom_format_packs: Vec<CustomFormatPack>,
    #[serde(default)]
    policy: GrabPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrabIndexer {
    pub name: String,
    pub priority: i32,
    pub app: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadClientKind {
    Sabnzbd,
    Qbittorrent,
}

impl DownloadClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sabnzbd => "sabnzbd",
            Self::Qbittorrent => "qbittorrent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrabDownloadClient {
    pub name: String,
    pub priority: i32,
    pub kind: DownloadClientKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomFormatPack {
    pub name: String,
    pub scores: BTreeMap<String, i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrabPolicy {
    #[serde(default)]
    pub delay_minutes: Option<u32>,
    #[serde(default)]
    pub quality_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grab {
    pub indexers: Vec<GrabIndexer>,
    pub download_clients: Vec<GrabDownloadClient>,
    pub custom_format_packs: Vec<CustomFormatPack>,
    pub policy: GrabPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    #[serde(default)]
    pub url_bases: BTreeMap<String, String>,
    pub bind: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    #[serde(default)]
    pub lidarr: Option<String>,
    #[serde(default)]
    pub matrix: Vec<PinMatrixRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinMatrixRow {
    pub package: String,
    pub os: String,
    pub glibc_min: String,
    pub refuse_above: String,
}

/// Compare `major.minor.patch` (missing patch = 0).
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some((major, minor, patch))
}

/// Exit 5 when a pin is above `refuse_above` (Lidarr glibc trap).
pub fn pin_matrix_refuse(pins: &Pins) -> Option<String> {
    for row in &pins.matrix {
        let current = match row.package.as_str() {
            "lidarr" => match pins.lidarr.as_deref() {
                Some(v) => v,
                None => continue,
            },
            _ => continue,
        };
        let Some(cur) = parse_semver(current) else {
            continue;
        };
        let Some(limit) = parse_semver(&row.refuse_above) else {
            continue;
        };
        if cur > limit {
            return Some(format!(
                "Lidarr glibc trap: refusing {} {} above {} on {} (glibc_min {})",
                row.package, current, row.refuse_above, row.os, row.glibc_min
            ));
        }
    }
    None
}

/// Paths + SHA-256-of-DER fingerprints. Never PEMs (AD-14).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsIdentity {
    pub ca_path: String,
    pub server_cert_path: String,
    pub server_key_path: String,
    pub client_cert_path: String,
    pub client_key_path: String,
    pub ca_sha256: String,
    pub server_sha256: String,
    pub client_sha256: String,
}

/// Parsed desired-state. Size fields exist only as [`Bytes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredState {
    schema_version: u32,
    max_copy: Bytes,
    min_free: Bytes,
    range_len: Bytes,
    max_nvenc: u32,
    lock: bool,
    grabber: Grabber,
    provider: Option<ProviderKind>,
    seedbox_address: Option<String>,
    underlay: UnderlayMode,
    tls: Option<TlsIdentity>,
    paths: Vec<PathRoot>,
    grab: Grab,
    edge: Option<Edge>,
    pins: Pins,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DesiredStateError {
    #[error("desired-state is not valid UTF-8")]
    InvalidUtf8,
    #[error("invalid desired-state TOML: {0}")]
    Parse(String),
    #[error("unsupported schema_version {0}; expected {SCHEMA_VERSION}")]
    UnsupportedVersion(u32),
    #[error("{field} must be greater than zero")]
    MustBeNonZero { field: &'static str },
    #[error("{field} = {value} overflows Bytes")]
    SizeOverflow { field: &'static str, value: u64 },
    #[error("{field} must be 64 lowercase hex characters")]
    InvalidFingerprint { field: &'static str },
    #[error("{field} must not contain a PEM body")]
    PemInDesiredState { field: &'static str },
    #[error("duplicate {field} `{name}`")]
    DuplicateName { field: &'static str, name: String },
}

impl DesiredState {
    pub fn from_toml(text: &str) -> Result<Self, DesiredStateError> {
        let version: SchemaVersionToml =
            toml::from_str(text).map_err(|err| DesiredStateError::Parse(err.to_string()))?;
        if version.schema_version != SCHEMA_VERSION {
            return Err(DesiredStateError::UnsupportedVersion(
                version.schema_version,
            ));
        }
        let raw: DesiredStateToml =
            toml::from_str(text).map_err(|err| DesiredStateError::Parse(err.to_string()))?;
        // Representable but nonsensical. A zero range length divides by zero in
        // the first consumer that counts chunks and never terminates a range
        // loop; a zero encoder cap stalls encode forever with no diagnostic.
        // There is no "disable" semantic on either field, so refuse at parse.
        if raw.range_len_mib == 0 {
            return Err(DesiredStateError::MustBeNonZero {
                field: "range_len_mib",
            });
        }
        if raw.max_nvenc == 0 {
            return Err(DesiredStateError::MustBeNonZero { field: "max_nvenc" });
        }
        if let Some(tls) = raw.tls.as_ref() {
            validate_tls(tls)?;
        }
        reject_duplicate_names(
            raw.paths.roots.iter().map(|r| r.id.as_str()),
            "paths.roots.id",
        )?;
        reject_duplicate_names(
            raw.grab.indexers.iter().map(|i| i.name.as_str()),
            "grab.indexers.name",
        )?;
        reject_duplicate_names(
            raw.grab.download_clients.iter().map(|c| c.name.as_str()),
            "grab.download_clients.name",
        )?;
        reject_duplicate_names(
            raw.grab.custom_format_packs.iter().map(|p| p.name.as_str()),
            "grab.custom_format_packs.name",
        )?;
        Ok(Self {
            schema_version: raw.schema_version,
            max_copy: gib(raw.max_copy_gib, "max_copy_gib")?,
            min_free: gib(raw.min_free_gib, "min_free_gib")?,
            range_len: mib(raw.range_len_mib, "range_len_mib")?,
            max_nvenc: raw.max_nvenc,
            lock: raw.lock,
            grabber: raw.grabber,
            provider: raw.provider,
            seedbox_address: raw.seedbox_address,
            underlay: raw.underlay,
            tls: raw.tls,
            paths: raw.paths.roots,
            grab: Grab {
                indexers: raw.grab.indexers,
                download_clients: raw.grab.download_clients,
                custom_format_packs: raw.grab.custom_format_packs,
                policy: raw.grab.policy,
            },
            edge: raw.edge,
            pins: raw.pins,
        })
    }

    pub fn from_toml_bytes(bytes: &[u8]) -> Result<Self, DesiredStateError> {
        let text = std::str::from_utf8(bytes).map_err(|_| DesiredStateError::InvalidUtf8)?;
        Self::from_toml(text)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Converted from `max_copy_gib` (1 GiB = 2^30).
    pub fn max_copy(&self) -> Bytes {
        self.max_copy
    }

    /// Converted from `min_free_gib` (1 GiB = 2^30).
    pub fn min_free(&self) -> Bytes {
        self.min_free
    }

    /// Converted from `range_len_mib` (1 MiB = 2^20).
    pub fn range_len(&self) -> Bytes {
        self.range_len
    }

    pub fn max_nvenc(&self) -> u32 {
        self.max_nvenc
    }

    pub fn lock(&self) -> bool {
        self.lock
    }

    pub fn grabber(&self) -> Grabber {
        self.grabber
    }

    pub fn provider(&self) -> Option<ProviderKind> {
        self.provider
    }

    pub fn seedbox_address(&self) -> Option<&str> {
        self.seedbox_address.as_deref()
    }

    pub fn underlay(&self) -> UnderlayMode {
        self.underlay
    }

    pub fn tls(&self) -> Option<&TlsIdentity> {
        self.tls.as_ref()
    }

    pub fn paths(&self) -> &[PathRoot] {
        &self.paths
    }

    pub fn grab(&self) -> &Grab {
        &self.grab
    }

    pub fn edge(&self) -> Option<&Edge> {
        self.edge.as_ref()
    }

    pub fn pins(&self) -> &Pins {
        &self.pins
    }
}

fn reject_duplicate_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    field: &'static str,
) -> Result<(), DesiredStateError> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(DesiredStateError::DuplicateName {
                field,
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == SHA256_HEX_LEN && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn looks_like_pem(value: &str) -> bool {
    value.contains("BEGIN ") || value.contains("-----")
}

fn validate_tls(tls: &TlsIdentity) -> Result<(), DesiredStateError> {
    for (field, value) in [
        ("ca_path", tls.ca_path.as_str()),
        ("server_cert_path", tls.server_cert_path.as_str()),
        ("server_key_path", tls.server_key_path.as_str()),
        ("client_cert_path", tls.client_cert_path.as_str()),
        ("client_key_path", tls.client_key_path.as_str()),
        ("ca_sha256", tls.ca_sha256.as_str()),
        ("server_sha256", tls.server_sha256.as_str()),
        ("client_sha256", tls.client_sha256.as_str()),
    ] {
        if looks_like_pem(value) {
            return Err(DesiredStateError::PemInDesiredState { field });
        }
    }
    for (field, value) in [
        ("ca_sha256", tls.ca_sha256.as_str()),
        ("server_sha256", tls.server_sha256.as_str()),
        ("client_sha256", tls.client_sha256.as_str()),
    ] {
        if !is_lowercase_sha256(value) {
            return Err(DesiredStateError::InvalidFingerprint { field });
        }
    }
    Ok(())
}

/// Insert or replace the `[tls]` table in a desired-state document. Paths and
/// fingerprints only — callers must never pass PEM bodies.
pub fn upsert_tls_table(toml_text: &str, tls: &TlsIdentity) -> Result<String, DesiredStateError> {
    validate_tls(tls)?;
    let mut table: toml::Table =
        toml::from_str(toml_text).map_err(|err| DesiredStateError::Parse(err.to_string()))?;
    let mut tls_table = toml::Table::new();
    tls_table.insert("ca_path".into(), toml::Value::String(tls.ca_path.clone()));
    tls_table.insert(
        "server_cert_path".into(),
        toml::Value::String(tls.server_cert_path.clone()),
    );
    tls_table.insert(
        "server_key_path".into(),
        toml::Value::String(tls.server_key_path.clone()),
    );
    tls_table.insert(
        "client_cert_path".into(),
        toml::Value::String(tls.client_cert_path.clone()),
    );
    tls_table.insert(
        "client_key_path".into(),
        toml::Value::String(tls.client_key_path.clone()),
    );
    tls_table.insert(
        "ca_sha256".into(),
        toml::Value::String(tls.ca_sha256.clone()),
    );
    tls_table.insert(
        "server_sha256".into(),
        toml::Value::String(tls.server_sha256.clone()),
    );
    tls_table.insert(
        "client_sha256".into(),
        toml::Value::String(tls.client_sha256.clone()),
    );
    table.insert("tls".into(), toml::Value::Table(tls_table));
    let encoded =
        toml::to_string(&table).map_err(|err| DesiredStateError::Parse(err.to_string()))?;
    DesiredState::from_toml(&encoded)?;
    Ok(encoded)
}

fn gib(n: u64, field: &'static str) -> Result<Bytes, DesiredStateError> {
    n.checked_mul(Bytes::GIB)
        .map(Bytes::new)
        .ok_or(DesiredStateError::SizeOverflow { field, value: n })
}

fn mib(n: u64, field: &'static str) -> Result<Bytes, DesiredStateError> {
    n.checked_mul(Bytes::MIB)
        .map(Bytes::new)
        .ok_or(DesiredStateError::SizeOverflow { field, value: n })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::probe::UnderlayMode;

    pub(crate) const HAPPY_TOML: &str = r#"
schema_version = 1
max_copy_gib = 256
min_free_gib = 256
range_len_mib = 8
max_nvenc = 2
lock = true
"#;

    fn assert_is_bytes(_: Bytes) {}

    #[test]
    fn ds_happy_sizes_are_bytes() {
        let ds = DesiredState::from_toml(HAPPY_TOML).expect("parse");
        assert_eq!(ds.schema_version(), 1);
        assert_is_bytes(ds.max_copy());
        assert_is_bytes(ds.min_free());
        assert_is_bytes(ds.range_len());
        assert_eq!(ds.max_copy(), Bytes::new(256 * Bytes::GIB));
        assert_eq!(ds.min_free(), Bytes::new(256 * Bytes::GIB));
        assert_eq!(ds.range_len(), Bytes::new(8 * Bytes::MIB));
        assert_eq!(ds.max_nvenc(), 2);
        assert!(ds.lock());
        assert_eq!(ds.max_copy().get(), 274877906944);
        assert_eq!(ds.range_len().get(), 8388608);
    }

    #[test]
    fn unknown_field_is_denied() {
        let toml = r#"
schema_version = 1
max_copy_gib = 256
min_free_gib = 256
range_len_mib = 8
max_nvenc = 2
lock = true
policies = {}
"#;
        let err = DesiredState::from_toml(toml).expect_err("unknown field");
        assert!(
            matches!(err, DesiredStateError::Parse(ref msg) if msg.contains("unknown field")),
            "expected deny_unknown_fields, got {err:?}"
        );
    }

    #[test]
    fn missing_schema_version_fails() {
        let toml = r#"
max_copy_gib = 256
min_free_gib = 256
range_len_mib = 8
max_nvenc = 2
lock = true
"#;
        let err = DesiredState::from_toml(toml).expect_err("missing version");
        assert!(
            matches!(err, DesiredStateError::Parse(ref msg) if msg.contains("schema_version")),
            "expected required schema_version, got {err:?}"
        );
    }

    #[test]
    fn schema_version_other_than_1_fails() {
        let toml = r#"
schema_version = 2
max_copy_gib = 256
min_free_gib = 256
range_len_mib = 8
max_nvenc = 2
lock = true
"#;
        assert_eq!(
            DesiredState::from_toml(toml),
            Err(DesiredStateError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn schema_version_2_with_extra_fields_is_unsupported_version() {
        let toml = r#"
schema_version = 2
max_copy_gib = 256
min_free_gib = 256
range_len_mib = 8
max_nvenc = 2
lock = true
policies = {}
"#;
        assert_eq!(
            DesiredState::from_toml(toml),
            Err(DesiredStateError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert_eq!(
            DesiredState::from_toml_bytes(&[0xff]),
            Err(DesiredStateError::InvalidUtf8)
        );
    }

    #[test]
    fn size_overflow_is_an_error() {
        let toml = r#"
schema_version = 1
max_copy_gib = 17179869184
min_free_gib = 256
range_len_mib = 8
max_nvenc = 2
lock = false
"#;
        assert!(matches!(
            DesiredState::from_toml(toml),
            Err(DesiredStateError::SizeOverflow {
                field: "max_copy_gib",
                value: 17179869184
            })
        ));
    }

    #[test]
    fn zero_range_len_is_refused() {
        let toml = HAPPY_TOML.replace("range_len_mib = 8", "range_len_mib = 0");
        assert_eq!(
            DesiredState::from_toml(&toml),
            Err(DesiredStateError::MustBeNonZero {
                field: "range_len_mib"
            })
        );
    }

    #[test]
    fn zero_max_nvenc_is_refused() {
        let toml = HAPPY_TOML.replace("max_nvenc = 2", "max_nvenc = 0");
        assert_eq!(
            DesiredState::from_toml(&toml),
            Err(DesiredStateError::MustBeNonZero { field: "max_nvenc" })
        );
    }

    #[test]
    fn grabber_defaults_to_none() {
        let ds = DesiredState::from_toml(HAPPY_TOML).expect("parse");
        assert_eq!(ds.grabber(), Grabber::None);
        assert_eq!(ds.underlay(), UnderlayMode::Direct);
        assert!(ds.tls().is_none());
    }

    #[test]
    fn pem_body_in_tls_is_refused() {
        let toml = format!(
            "{HAPPY_TOML}\n[tls]\nca_path = \"-----BEGIN CERTIFICATE-----\"\n\
             server_cert_path = \"/a\"\nserver_key_path = \"/b\"\n\
             client_cert_path = \"/c\"\nclient_key_path = \"/d\"\n\
             ca_sha256 = \"{fp}\"\nserver_sha256 = \"{fp}\"\nclient_sha256 = \"{fp}\"\n",
            fp = "a".repeat(64)
        );
        assert!(matches!(
            DesiredState::from_toml(&toml),
            Err(DesiredStateError::PemInDesiredState { field: "ca_path" })
        ));
    }

    #[test]
    fn upsert_tls_table_never_writes_pem() {
        let fp = "ab".repeat(32);
        let tls = TlsIdentity {
            ca_path: "/cfg/tls/ca.pem".into(),
            server_cert_path: "/cfg/tls/server.pem".into(),
            server_key_path: "/cfg/tls/server.key".into(),
            client_cert_path: "/cfg/tls/client.pem".into(),
            client_key_path: "/cfg/tls/client.key".into(),
            ca_sha256: fp.clone(),
            server_sha256: fp.clone(),
            client_sha256: fp,
        };
        let encoded = upsert_tls_table(HAPPY_TOML, &tls).expect("upsert");
        assert!(!encoded.contains("BEGIN "));
        let ds = DesiredState::from_toml(&encoded).expect("reparse");
        assert_eq!(ds.tls().expect("tls").ca_path, "/cfg/tls/ca.pem");
    }

    #[test]
    fn happy_toml_still_parses_with_empty_grab_tables() {
        let ds = DesiredState::from_toml(HAPPY_TOML).expect("parse");
        assert!(ds.paths().is_empty());
        assert!(ds.grab().indexers.is_empty());
        assert!(ds.edge().is_none());
        assert!(ds.pins().lidarr.is_none());
        assert_eq!(ds.grabber(), Grabber::None);
    }

    #[test]
    fn servarr_tables_parse_and_duplicate_nzbgeek_is_conflict() {
        let toml = format!(
            r#"
{HAPPY_TOML}
grabber = "servarr"

[[paths.roots]]
id = "complete"
path = "/data/complete"

[[grab.indexers]]
name = "NZBgeek"
priority = 25
app = "prowlarr"

[[grab.download_clients]]
name = "SABnzbd"
priority = 1
kind = "sabnzbd"

[[grab.custom_format_packs]]
name = "prefer-h264"
scores = {{ "x264" = 100, "x265" = -10000 }}

[grab.policy]
delay_minutes = 0

[edge]
url_bases = {{ sonarr = "/sonarr", radarr = "/radarr" }}
bind = "127.0.0.1"
auth = "forms"

[pins]
lidarr = "2.14.5"

[[pins.matrix]]
package = "lidarr"
os = "ubuntu-20.04"
glibc_min = "2.31"
refuse_above = "2.14.5"
"#
        );
        let ds = DesiredState::from_toml(&toml).expect("parse");
        assert_eq!(ds.grabber(), Grabber::Servarr);
        assert_eq!(ds.paths()[0].id, "complete");
        assert_eq!(ds.grab().indexers[0].name, "NZBgeek");
        assert_eq!(
            ds.grab().download_clients[0].kind,
            DownloadClientKind::Sabnzbd
        );
        assert_eq!(ds.edge().expect("edge").bind, "127.0.0.1");
        assert_eq!(ds.pins().lidarr.as_deref(), Some("2.14.5"));

        let dup = format!(
            r#"
{HAPPY_TOML}
[[grab.indexers]]
name = "NZBgeek"
priority = 25
app = "prowlarr"
[[grab.indexers]]
name = "NZBgeek"
priority = 50
app = "prowlarr"
"#
        );
        assert!(matches!(
            DesiredState::from_toml(&dup),
            Err(DesiredStateError::DuplicateName {
                field: "grab.indexers.name",
                name
            }) if name == "NZBgeek"
        ));
    }

    #[test]
    fn lidarr_glibc_trap_refuses_above_pin() {
        let ok = Pins {
            lidarr: Some("2.14.5".into()),
            matrix: vec![PinMatrixRow {
                package: "lidarr".into(),
                os: "ubuntu-20.04".into(),
                glibc_min: "2.31".into(),
                refuse_above: "2.14.5".into(),
            }],
        };
        assert!(pin_matrix_refuse(&ok).is_none());
        let trap = Pins {
            lidarr: Some("2.15.0".into()),
            matrix: ok.matrix.clone(),
        };
        let msg = pin_matrix_refuse(&trap).expect("refuse");
        assert!(msg.contains("Lidarr glibc trap"), "{msg}");
        assert!(msg.contains("2.15.0"), "{msg}");
    }
}
