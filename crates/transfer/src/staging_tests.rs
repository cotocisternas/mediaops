use crate::PullSpec;
use crate::cleanup_verified_staging;
use crate::sidecar::{self, Sidecar};
use mediaops_core::{RemoteRef, TitleId, staging_path};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mediaops-staging-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir(&path).expect("scratch");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn spec(root: &Path) -> PullSpec {
    PullSpec {
        library_root: root.to_path_buf(),
        title_id: TitleId::movie("603").expect("id"),
        final_name: "The.Matrix.(1999).mkv".into(),
        remote: RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from("a.bin")).expect("ref"),
        file_len: 8,
        range_len: 4,
        concurrency: 1,
    }
}

fn paths(spec: &PullSpec) -> (PathBuf, PathBuf, PathBuf) {
    let staged = spec
        .library_root
        .join(staging_path(&spec.title_id, &spec.final_name).expect("staging"));
    let mut sidecar = staged.clone();
    sidecar.as_mut_os_string().push(".partial.b3");
    let lock = staged.with_extension("pull.lock");
    (staged, sidecar, lock)
}

fn write_owned_staging(
    spec: &PullSpec,
    extra: Option<(&str, &[u8])>,
) -> (PathBuf, PathBuf, PathBuf) {
    let (staged, sidecar_path, lock_path) = paths(spec);
    fs::create_dir_all(staged.parent().expect("parent")).expect("mkdir");
    fs::write(&staged, b"abcdefgh").expect("staged");
    let mut proof = Sidecar::new(spec.file_len, spec.range_len);
    proof.remote_root = spec.remote.root_id().to_string();
    proof.remote_path = spec.remote.rel_path().to_string_lossy().into_owned();
    sidecar::save(&sidecar_path, &proof).expect("sidecar");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .expect("lock");
    if let Some((name, bytes)) = extra {
        fs::write(staged.parent().expect("parent").join(name), bytes).expect("extra");
    }
    (staged, sidecar_path, lock_path)
}

#[test]
fn cleanup_removes_staged_source_and_sidecar_and_retains_lock() {
    let dir = Scratch::new("owned");
    let spec = spec(&dir.0);
    let (staged, sidecar, lock) = write_owned_staging(&spec, Some(("stray.bin", b"keep")));
    cleanup_verified_staging(&spec).expect("cleanup");
    assert!(fs::symlink_metadata(&staged).is_err());
    assert!(fs::symlink_metadata(&sidecar).is_err());
    assert!(lock.is_file());
    assert_eq!(
        fs::read(staged.parent().expect("parent").join("stray.bin")).expect("stray"),
        b"keep"
    );
}

#[test]
fn cleanup_is_idempotent_when_staged_files_are_absent() {
    let dir = Scratch::new("absent");
    let spec = spec(&dir.0);
    let (staged, sidecar, lock) = write_owned_staging(&spec, None);
    fs::remove_file(&staged).expect("staged");
    fs::remove_file(&sidecar).expect("sidecar");
    cleanup_verified_staging(&spec).expect("first");
    cleanup_verified_staging(&spec).expect("second");
    assert!(lock.is_file());
    fs::remove_file(&lock).expect("lock");
    cleanup_verified_staging(&spec).expect("no lock and no files");
}

#[test]
fn cleanup_rejects_symlink_staged_source() {
    let dir = Scratch::new("symlink");
    let spec = spec(&dir.0);
    let (staged, sidecar, lock) = write_owned_staging(&spec, None);
    let target = dir.0.join("precious");
    fs::write(&target, b"keep").expect("target");
    fs::remove_file(&staged).expect("staged");
    std::os::unix::fs::symlink(&target, &staged).expect("symlink");
    assert!(cleanup_verified_staging(&spec).is_err());
    assert_eq!(fs::read(&target).expect("preserved"), b"keep");
    assert!(sidecar.is_file());
    assert!(lock.is_file());
}

#[test]
fn cleanup_rejects_sidecar_for_another_remote() {
    let dir = Scratch::new("foreign");
    let spec = spec(&dir.0);
    let (staged, sidecar, lock) = write_owned_staging(&spec, None);
    let mut proof = sidecar::load(&sidecar).expect("load").expect("present");
    proof.remote_root = "other-root".into();
    sidecar::save(&sidecar, &proof).expect("foreign");
    assert!(cleanup_verified_staging(&spec).is_err());
    assert_eq!(fs::read(&staged).expect("staged"), b"abcdefgh");
    assert!(sidecar.is_file());
    assert!(lock.is_file());
}

#[test]
fn cleanup_fails_when_writer_lock_is_held() {
    let dir = Scratch::new("held");
    let spec = spec(&dir.0);
    let (staged, sidecar, lock_path) = write_owned_staging(&spec, None);
    let lock = File::open(&lock_path).expect("open");
    lock.try_lock().expect("hold");
    assert!(cleanup_verified_staging(&spec).is_err());
    assert_eq!(fs::read(&staged).expect("staged"), b"abcdefgh");
    assert!(sidecar.is_file());
    drop(lock);
}

#[test]
fn cleanup_fails_when_lock_is_missing_and_staging_remains() {
    let dir = Scratch::new("nolock");
    let spec = spec(&dir.0);
    let (staged, sidecar, lock) = write_owned_staging(&spec, None);
    fs::remove_file(&lock).expect("lock");
    assert!(cleanup_verified_staging(&spec).is_err());
    assert_eq!(fs::read(&staged).expect("staged"), b"abcdefgh");
    assert!(sidecar.is_file());
}
