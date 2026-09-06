//! Bind only our own stale socket; an unrelated file must survive startup.

use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};

pub async fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let parent_meta = std::fs::metadata(parent)?;
    if parent_meta.uid() != unsafe { libc::geteuid() } || parent_meta.mode() & 0o022 != 0 {
        anyhow::bail!(
            "gateway socket directory must be owned by this user and not writable by other users: {}",
            parent.display()
        );
    }
    match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
        Ok(meta) => {
            if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
                anyhow::bail!(
                    "refusing to remove an unrelated entry at {}",
                    path.display()
                );
            }
            match tokio::time::timeout(Duration::from_secs(1), UnixStream::connect(path)).await {
                Ok(Err(err)) if err.kind() == io::ErrorKind::ConnectionRefused => {}
                Ok(Err(err)) => return Err(err.into()),
                Ok(Ok(_)) | Err(_) => {
                    anyhow::bail!("gateway socket {} is live or busy", path.display())
                }
            }
            let current = std::fs::symlink_metadata(path)?;
            if current.dev() != meta.dev() || current.ino() != meta.ino() {
                anyhow::bail!("gateway socket changed during startup: {}", path.display());
            }
            std::fs::remove_file(path)?;
        }
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mediaops-gateway-socket-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("dir");
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn preserves_regular_files_symlinks_and_live_sockets() {
        let dir = Scratch::new();
        let path = dir.0.join("gateway.sock");
        std::fs::write(&path, "operator data").expect("file");
        assert!(bind(&path).await.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("preserved"),
            "operator data"
        );
        let link = dir.0.join("link.sock");
        std::os::unix::fs::symlink(&path, &link).expect("link");
        assert!(bind(&link).await.is_err());
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("preserved")
                .file_type()
                .is_symlink()
        );
        let live = dir.0.join("live.sock");
        let _listener = bind(&live).await.expect("listen");
        assert!(bind(&live).await.is_err());
    }

    #[tokio::test]
    async fn replaces_own_stale_socket_and_sets_private_permissions() {
        let dir = Scratch::new();
        let path = dir.0.join("gateway.sock");
        let listener = bind(&path).await.expect("listen");
        drop(listener);
        let _listener = bind(&path).await.expect("stale socket");
        assert_eq!(
            std::fs::metadata(&path).expect("metadata").mode() & 0o777,
            0o600
        );
    }
}
