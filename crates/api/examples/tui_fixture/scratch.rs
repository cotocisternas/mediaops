use std::path::{Path, PathBuf};

use super::errors::FixtureError;

const FIXTURE_ENTRIES: &[&str] = &["api.db", "api.db-wal", "api.db-shm", "api.sock", "library"];

#[derive(Debug, Clone)]
pub struct Scratch {
    pub socket: PathBuf,
    pub api_db: PathBuf,
    pub library: PathBuf,
}

pub fn prepare_scratch(dir: &Path) -> Result<Scratch, FixtureError> {
    if !dir.is_absolute() || is_shared_root(dir) || has_git_ancestor(dir) {
        return Err(FixtureError::ScratchNotDedicated);
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if !meta.is_dir() => return Err(FixtureError::ScratchNotDedicated),
        Ok(_) => ensure_only_fixture_entries(dir)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    std::fs::create_dir_all(dir)?;
    let library = dir.join("library");
    std::fs::create_dir_all(&library)?;
    Ok(Scratch {
        socket: dir.join("api.sock"),
        api_db: dir.join("api.db"),
        library,
    })
}

fn is_shared_root(dir: &Path) -> bool {
    dir == Path::new("/") || dir == std::env::temp_dir()
}

fn has_git_ancestor(dir: &Path) -> bool {
    dir.ancestors().any(|p| p.join(".git").exists())
}

fn ensure_only_fixture_entries(dir: &Path) -> Result<(), FixtureError> {
    for entry in std::fs::read_dir(dir)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else {
            return Err(FixtureError::ScratchNotDedicated);
        };
        if !FIXTURE_ENTRIES.contains(&name) {
            return Err(FixtureError::ScratchNotDedicated);
        }
    }
    Ok(())
}
