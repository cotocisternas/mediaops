//! A separate test executable keeps subprocess creation away from unit-test
//! locks. Each child runs one test and sets umask before starting its runtime.
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mediaops_core::{Blake3Hex, Placement, RemoteRef, TitleId, VerifiedStagingHandle};
use mediaops_transfer::{PullSpec, RangeSource, TransferError, pull_file_with_progress};

struct Memory;
impl RangeSource for Memory {
    async fn get_range(&self, _: &RemoteRef, _: u64, len: u64) -> Result<Vec<u8>, TransferError> {
        Ok(vec![7; len as usize])
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").mode() & 0o7777
}

#[test]
fn publication_and_staging_modes_under_isolated_umasks() {
    const CHILD: &str = "MEDIAOPS_PUBLICATION_UMASK_CHILD";
    let Ok(mask) = std::env::var(CHILD) else {
        for mask in ["077", "000"] {
            let output =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args([
                        "--exact",
                        "publication_and_staging_modes_under_isolated_umasks",
                        "--nocapture",
                    ])
                    .env(CHILD, mask)
                    .output()
                    .expect("isolated child");
            assert!(
                output.status.success(),
                "umask {mask}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    };
    let mask = u32::from_str_radix(&mask, 8).expect("umask");
    assert!(matches!(mask, 0o077 | 0o000));
    unsafe {
        libc::umask(mask);
    }
    let root = Scratch(std::env::temp_dir().join(format!("mediaops-umask-{}", std::process::id())));
    fs::create_dir(&root.0).expect("root");
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).expect("operator root policy");
    let title = TitleId::movie("603").expect("id");
    let placement = Placement::movie("The.Matrix", 1999, "mkv");
    let spec = PullSpec {
        library_root: root.0.clone(),
        title_id: title.clone(),
        final_name: "The.Matrix.(1999).mkv".into(),
        remote: RemoteRef::from_wire_parts("seedbox".into(), "a.bin".into()).expect("remote"),
        file_len: 16,
        range_len: 4,
        concurrency: 1,
    };
    let staged = root
        .0
        .join(mediaops_core::staging_path(&title, &spec.final_name).expect("staging"));
    let partial = PathBuf::from(format!("{}.partial", staged.display()));
    let mut partial_seen = false;
    let outcome = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(pull_file_with_progress(Arc::new(Memory), &spec, |_, _| {
            if partial.exists() {
                partial_seen = true;
                assert_eq!(mode(&partial), 0o600);
            }
        }))
        .expect("pull");
    assert!(partial_seen);
    assert_eq!(mode(&outcome.staged), 0o600);
    assert_eq!(mode(&root.0.join("_incoming")), 0o700);
    assert_eq!(mode(staged.parent().expect("parent")), 0o700);
    assert_eq!(
        mode(&PathBuf::from(format!("{}.partial.b3", staged.display()))),
        0o600
    );
    assert_eq!(mode(&staged.with_extension("pull.lock")), 0o600);
    let handle =
        VerifiedStagingHandle::verify(&root.0, &title, staged, &placement).expect("handle");
    let result =
        mediaops_core::install_verified(&root.0, &title, &handle, &Blake3Hex::of_bytes(&[7; 16]))
            .expect("install");
    assert_eq!(mode(&result.path), 0o644);
    assert_eq!(fs::read(&result.path).expect("media"), [7; 16]);
    let parent = result.path.parent().expect("schema parent");
    assert_eq!(mode(parent), 0o755);
    assert_eq!(mode(parent.parent().expect("schema kind")), 0o755);
    assert_eq!(mode(&root.0), 0o700);
}
