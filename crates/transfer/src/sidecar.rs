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
