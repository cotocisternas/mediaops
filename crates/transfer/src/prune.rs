//! Empty-dir prune under `_incoming/`. `*.partial*` dirs are sacred.

use std::fs;
use std::path::{Path, PathBuf};

use crate::TransferError;

pub fn dir_is_sacred(dir: &Path) -> Result<bool, TransferError> {
    let reader = match fs::read_dir(dir) {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(TransferError::io(dir, err)),
    };
    for entry in reader {
        let entry = entry.map_err(|err| TransferError::io(dir, err))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.contains(".partial") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Remove empty directories under `incoming` that are not sacred.
/// Does not delete `incoming` itself.
pub fn prune_empty_incoming(incoming: &Path) -> Result<Vec<PathBuf>, TransferError> {
    let mut removed = Vec::new();
    prune_dir(incoming, incoming, &mut removed)?;
    Ok(removed)
}

fn prune_dir(incoming: &Path, dir: &Path, removed: &mut Vec<PathBuf>) -> Result<bool, TransferError> {
    if dir_is_sacred(dir)? {
        return Ok(false);
    }
    let reader = match fs::read_dir(dir) {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(TransferError::io(dir, err)),
    };
    let entries: Vec<_> = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| TransferError::io(dir, err))?;
    let mut empty = true;
    for entry in entries {
        let path = entry.path();
        let ft = entry
            .file_type()
            .map_err(|err| TransferError::io(&path, err))?;
        if ft.is_dir() {
            if !prune_dir(incoming, &path, removed)? {
                empty = false;
            }
        } else {
            empty = false;
        }
    }
    if empty && dir != incoming {
        fs::remove_dir(dir).map_err(|err| TransferError::io(dir, err))?;
        removed.push(dir.to_path_buf());
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-prune-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        let mut f = fs::File::create(path).expect("create");
        f.write_all(bytes).expect("write");
    }

    #[test]
    fn sidecar_only_dir_is_sacred_and_not_pruned() {
        let incoming = scratch("sacred");
        let title = incoming.join("movie-tmdb-603");
        write(&title.join("The.Matrix.(1999).mkv.partial.b3"), b"{}");
        assert!(dir_is_sacred(&title).expect("sacred"));
        let removed = prune_empty_incoming(&incoming).expect("prune");
        assert!(removed.is_empty());
        assert!(title.join("The.Matrix.(1999).mkv.partial.b3").is_file());
        let _ = fs::remove_dir_all(incoming);
    }

    #[test]
    fn empty_title_dir_is_pruned() {
        let incoming = scratch("empty");
        let title = incoming.join("movie-tmdb-603");
        fs::create_dir_all(&title).expect("mkdir");
        let removed = prune_empty_incoming(&incoming).expect("prune");
        assert_eq!(removed, vec![title.clone()]);
        assert!(!title.exists());
        assert!(incoming.is_dir());
        let _ = fs::remove_dir_all(incoming);
    }
}
