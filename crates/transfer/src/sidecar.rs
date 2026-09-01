//! `.partial.b3` sidecar. All lengths and offsets are bytes.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::TransferError;

pub const SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    pub file_len: u64,
    pub range_len: u64,
    pub ranges: Vec<SidecarRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarRange {
    pub offset: u64,
    pub len: u64,
    pub blake3: String,
}

impl Sidecar {
    pub fn new(file_len: u64, range_len: u64) -> Self {
        Self {
            version: SIDECAR_VERSION,
            file_len,
            range_len,
            ranges: Vec::new(),
        }
    }

    pub fn has(&self, offset: u64, len: u64) -> bool {
        self.ranges
            .iter()
            .any(|r| r.offset == offset && r.len == len)
    }

    pub fn record(&mut self, offset: u64, len: u64, blake3: String) {
        if !self.has(offset, len) {
            self.ranges.push(SidecarRange {
                offset,
                len,
                blake3,
            });
        }
    }
}

pub fn load(path: &Path) -> Result<Option<Sidecar>, TransferError> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|err| TransferError::io(path, err))?;
    let sidecar: Sidecar =
        serde_json::from_str(&text).map_err(|err| TransferError::Sidecar(err.to_string()))?;
    if sidecar.version != SIDECAR_VERSION {
        return Err(TransferError::Sidecar(format!(
            "unsupported sidecar version {}",
            sidecar.version
        )));
    }
    if sidecar.range_len == 0 {
        return Err(TransferError::Sidecar("range_len must be > 0".into()));
    }
    Ok(Some(sidecar))
}

pub fn save(path: &Path, sidecar: &Sidecar) -> Result<(), TransferError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| TransferError::io(parent, err))?;
    }
    let tmp = path.with_extension("b3.tmp");
    let json = serde_json::to_vec_pretty(sidecar)
        .map_err(|err| TransferError::Sidecar(err.to_string()))?;
    let mut file = File::create(&tmp).map_err(|err| TransferError::io(&tmp, err))?;
    file.write_all(&json)
        .map_err(|err| TransferError::io(&tmp, err))?;
    file.sync_all()
        .map_err(|err| TransferError::io(&tmp, err))?;
    fs::rename(&tmp, path).map_err(|err| TransferError::io(path, err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-sidecar-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn load_missing_path_is_none() {
        let dir = scratch("missing");
        let path = dir.join("a.partial.b3");
        assert!(load(&path).expect("load").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_bad_json_is_sidecar_error() {
        let dir = scratch("bad");
        let path = dir.join("a.partial.b3");
        fs::write(&path, "not-json").expect("write");
        let err = load(&path).expect_err("bad json");
        assert!(matches!(err, TransferError::Sidecar(_)), "{err:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_unsupported_version_is_sidecar_error() {
        let dir = scratch("ver");
        let path = dir.join("a.partial.b3");
        fs::write(
            &path,
            r#"{"version":99,"file_len":10,"range_len":4,"ranges":[]}"#,
        )
        .expect("write");
        let err = load(&path).expect_err("version");
        match err {
            TransferError::Sidecar(message) => {
                assert!(
                    message.contains("unsupported sidecar version 99"),
                    "{message}"
                );
            }
            other => panic!("expected Sidecar, got {other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_zero_range_len_is_sidecar_error() {
        let dir = scratch("zero");
        let path = dir.join("a.partial.b3");
        fs::write(
            &path,
            r#"{"version":1,"file_len":10,"range_len":0,"ranges":[]}"#,
        )
        .expect("write");
        let err = load(&path).expect_err("range_len");
        match err {
            TransferError::Sidecar(message) => {
                assert!(message.contains("range_len must be > 0"), "{message}");
            }
            other => panic!("expected Sidecar, got {other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_load_round_trip_and_record_is_idempotent() {
        let dir = scratch("round");
        let path = dir.join("nested").join("a.partial.b3");
        let mut sidecar = Sidecar::new(10, 4);
        sidecar.record(0, 4, "abc".into());
        sidecar.record(0, 4, "abc".into());
        assert_eq!(sidecar.ranges.len(), 1);
        assert!(sidecar.has(0, 4));
        assert!(!sidecar.has(4, 4));
        save(&path, &sidecar).expect("save");
        let loaded = load(&path).expect("load").expect("some");
        assert_eq!(loaded, sidecar);
        let _ = fs::remove_dir_all(dir);
    }
}
