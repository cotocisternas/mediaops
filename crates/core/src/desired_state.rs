//! Desired-state document. Serde TOML, `deny_unknown_fields`, `schema_version` 1.

use serde::Deserialize;

use crate::bytes::Bytes;

const SCHEMA_VERSION: u32 = 1;

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
        Ok(Self {
            schema_version: raw.schema_version,
            max_copy: gib(raw.max_copy_gib, "max_copy_gib")?,
            min_free: gib(raw.min_free_gib, "min_free_gib")?,
            range_len: mib(raw.range_len_mib, "range_len_mib")?,
            max_nvenc: raw.max_nvenc,
            lock: raw.lock,
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
}
