//! PullFile: `.partial` + sidecar, resume, whole-file BLAKE3.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mediaops_core::{Blake3Hex, RemoteRef, TitleId, staging_path};
use tokio::task::JoinSet;

use crate::TransferError;
use crate::schedule::{PendingFile, plan_ranges, remaining, take_slots};
use crate::sidecar::{self, Sidecar};

pub trait RangeSource: Send + Sync {
    fn get_range(
        &self,
        remote: &RemoteRef,
        offset: u64,
        len: u64,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, TransferError>> + Send;
}

pub struct PullSpec {
    pub library_root: PathBuf,
    pub title_id: TitleId,
    pub final_name: String,
    pub remote: RemoteRef,
    pub file_len: u64,
    pub range_len: u64,
    pub concurrency: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullOutcome {
    pub staged: PathBuf,
    /// Ranges the sidecar already had; empty for a fresh pull. A pull that
    /// found the file fully staged (a previous run died between the final
    /// rename and its job write) reports `already_staged`.
    pub resumed_ranges: Vec<(u64, u64)>,
    pub already_staged: bool,
}

/// Pull `spec.remote` into `_incoming/<token>/<final-name>`.
///
/// The staged file is **not** hashed here: the install gate hashes it once
/// when it places the file, and that digest is what the index records.
pub async fn pull_file<S: RangeSource + 'static>(
    src: Arc<S>,
    spec: &PullSpec,
) -> Result<PullOutcome, TransferError> {
    pull_file_with_progress(src, spec, |_, _| {}).await
}

/// Same as [`pull_file`], reporting `(done, total)` bytes after each range.
pub async fn pull_file_with_progress<S, F>(
    src: Arc<S>,
    spec: &PullSpec,
    mut on_progress: F,
) -> Result<PullOutcome, TransferError>
where
    S: RangeSource + 'static,
    F: FnMut(u64, u64) + Send,
{
    if spec.file_len == 0 {
        return Err(TransferError::Sidecar("file_len must be > 0".into()));
    }
    let rel = staging_path(&spec.title_id, &spec.final_name)
        .map_err(|err| TransferError::Path(err.to_string()))?;
    let staged = spec.library_root.join(&rel);
    let partial = {
        let mut p = staged.clone();
        p.as_mut_os_string().push(".partial");
        p
    };
    let sidecar_path = {
        let mut p = staged.clone();
        p.as_mut_os_string().push(".partial.b3");
        p
    };
    if let Some(parent) = partial.parent() {
        fs::create_dir_all(parent).map_err(|err| TransferError::io(parent, err))?;
    }
    if let Ok(meta) = fs::symlink_metadata(&staged) {
        // A complete staged file with no `.partial` beside it is the crash
        // window after the final rename: the bytes are all here, only the job
        // row was not advanced. Hand it to the install gate instead of
        // refusing forever.
        if meta.is_file() && !partial.exists() && meta.len() == spec.file_len {
            let _ = fs::remove_file(&sidecar_path);
            on_progress(spec.file_len, spec.file_len);
            return Ok(PullOutcome {
                staged,
                resumed_ranges: Vec::new(),
                already_staged: true,
            });
        }
        return Err(TransferError::Path(format!(
            "staged file already exists with unexpected shape: {}",
            staged.display()
        )));
    }

    let mut sidecar = match sidecar::load(&sidecar_path)? {
        Some(existing) => {
            if existing.file_len != spec.file_len {
                return Err(TransferError::Sidecar(format!(
                    "sidecar file_len {} != {}",
                    existing.file_len, spec.file_len
                )));
            }
            verify_recorded_ranges(existing, &partial)?
        }
        None => {
            if spec.range_len == 0 {
                return Err(TransferError::Sidecar("range_len must be > 0".into()));
            }
            Sidecar::new(spec.file_len, spec.range_len)
        }
    };
    let range_len = sidecar.range_len;
    let planned = plan_ranges(spec.file_len, range_len);
    let mut todo = remaining(&planned, &sidecar);
    let resumed_ranges: Vec<(u64, u64)> =
        sidecar.ranges.iter().map(|r| (r.offset, r.len)).collect();
    on_progress(sidecar_done(&sidecar), spec.file_len);

    {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&partial)
            .map_err(|err| TransferError::io(&partial, err))?;
        file.set_len(spec.file_len)
            .map_err(|err| TransferError::io(&partial, err))?;
    }

    let concurrency = spec.concurrency.max(1);
    while !todo.is_empty() {
        let mut files = [PendingFile {
            index: 0,
            remaining: todo.clone(),
        }];
        let batch = take_slots(&mut files, concurrency);
        todo = std::mem::take(&mut files[0].remaining);

        let mut set = JoinSet::new();
        for (_, offset, len) in batch {
            let src = src.clone();
            let remote = spec.remote.clone();
            set.spawn(async move {
                let bytes = src.get_range(&remote, offset, len).await?;
                Ok::<_, TransferError>((offset, len, bytes))
            });
        }
        while let Some(joined) = set.join_next().await {
            let (offset, len, bytes) =
                joined.map_err(|err| TransferError::Join(err.to_string()))??;
            if bytes.len() as u64 != len {
                return Err(TransferError::ShortRange {
                    offset,
                    want: len,
                    got: bytes.len() as u64,
                });
            }
            let digest = Blake3Hex::of_bytes(&bytes);
            write_range(&partial, offset, &bytes)?;
            sidecar.record(offset, len, digest.to_string());
            sidecar::save(&sidecar_path, &sidecar)?;
            on_progress(sidecar_done(&sidecar), spec.file_len);
        }
    }

    // Every range is on disk and recorded: this is where the file becomes a
    // whole. Rename first; a crash before the sidecar is removed leaves
    // (staged, sidecar, no partial), which the check at the top recognises.
    fs::rename(&partial, &staged).map_err(|err| TransferError::io(&staged, err))?;
    fs::remove_file(&sidecar_path).map_err(|err| TransferError::io(&sidecar_path, err))?;
    Ok(PullOutcome {
        staged,
        resumed_ranges,
        already_staged: false,
    })
}

fn sidecar_done(sidecar: &Sidecar) -> u64 {
    sidecar.ranges.iter().map(|r| r.len).sum()
}

fn verify_recorded_ranges(mut sidecar: Sidecar, partial: &Path) -> Result<Sidecar, TransferError> {
    if !partial.exists() {
        sidecar.ranges.clear();
        return Ok(sidecar);
    }
    let mut file = File::open(partial).map_err(|err| TransferError::io(partial, err))?;
    let mut kept = Vec::new();
    for range in sidecar.ranges.drain(..) {
        let n = sidecar::range_buf_len(sidecar.file_len, range.offset, range.len)?;
        let mut buf = vec![0_u8; n];
        if file.seek(SeekFrom::Start(range.offset)).is_err() {
            continue;
        }
        if file.read_exact(&mut buf).is_err() {
            continue;
        }
        if Blake3Hex::of_bytes(&buf).to_string() == range.blake3 {
            kept.push(range);
        }
    }
    sidecar.ranges = kept;
    Ok(sidecar)
}

fn write_range(path: &Path, offset: u64, bytes: &[u8]) -> Result<(), TransferError> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|err| TransferError::io(path, err))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| TransferError::io(path, err))?;
    file.write_all(bytes)
        .map_err(|err| TransferError::io(path, err))?;
    file.sync_all()
        .map_err(|err| TransferError::io(path, err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::TitleId;
    use std::sync::Mutex;

    struct Mem {
        body: Vec<u8>,
        hits: Mutex<Vec<(u64, u64)>>,
    }

    impl RangeSource for Mem {
        async fn get_range(
            &self,
            _remote: &RemoteRef,
            offset: u64,
            len: u64,
        ) -> Result<Vec<u8>, TransferError> {
            self.hits.lock().expect("hits").push((offset, len));
            let start = offset as usize;
            let end = start + len as usize;
            Ok(self.body[start..end].to_vec())
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-pull-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn remote() -> RemoteRef {
        RemoteRef::from_wire_parts("seedbox".into(), PathBuf::from("a.bin")).expect("ref")
    }

    #[tokio::test]
    async fn pull_writes_staging_and_hashes() {
        let root = scratch("full");
        let body = b"abcdefghij".to_vec();
        let src = Arc::new(Mem {
            body: body.clone(),
            hits: Mutex::new(Vec::new()),
        });
        let spec = PullSpec {
            library_root: root.clone(),
            title_id: TitleId::movie("603").expect("id"),
            final_name: "The.Matrix.(1999).mkv".into(),
            remote: remote(),
            file_len: body.len() as u64,
            range_len: 4,
            concurrency: 2,
        };
        let hits_progress = std::sync::Arc::new(Mutex::new(Vec::new()));
        let tracked = hits_progress.clone();
        let out = pull_file_with_progress(src.clone(), &spec, move |done, total| {
            tracked.lock().expect("prog").push((done, total));
        })
        .await
        .expect("pull");
        let progress = hits_progress.lock().expect("prog").clone();
        assert!(
            progress
                .iter()
                .any(|(done, total)| *done == body.len() as u64 && *total == body.len() as u64),
            "progress must reach the whole file: {progress:?}"
        );
        assert_eq!(fs::read(&out.staged).expect("read"), body);
        assert!(out.resumed_ranges.is_empty());
        assert!(!out.already_staged);
        let mut leftover = out.staged.clone();
        leftover.as_mut_os_string().push(".partial");
        assert!(!leftover.exists());
        let mut sidecar = out.staged.clone();
        sidecar.as_mut_os_string().push(".partial.b3");
        assert!(!sidecar.exists());

        // Crash window: the file is fully staged but the job row never moved.
        // The next pull must hand it on, not refuse, and must not fetch.
        let hits_before = src.hits.lock().expect("hits").len();
        fs::write(&sidecar, b"{}").expect("stale sidecar");
        let again = pull_file(src.clone(), &spec).await.expect("already staged");
        assert!(again.already_staged);
        assert_eq!(again.staged, out.staged);
        assert_eq!(
            src.hits.lock().expect("hits").len(),
            hits_before,
            "no refetch"
        );
        assert!(!sidecar.exists(), "stale sidecar is cleaned up");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn staged_file_of_the_wrong_length_is_refused() {
        let root = scratch("wrong-len");
        let title = TitleId::movie_key("The.Matrix", 1999).expect("id");
        let rel = staging_path(&title, "The.Matrix.(1999).mkv").expect("staging");
        let staged = root.join(&rel);
        fs::create_dir_all(staged.parent().expect("parent")).expect("mkdir");
        fs::write(&staged, b"short").expect("staged");
        let src = Arc::new(Mem {
            body: b"abcdefghij".to_vec(),
            hits: Mutex::new(Vec::new()),
        });
        let spec = PullSpec {
            library_root: root.clone(),
            title_id: title,
            final_name: "The.Matrix.(1999).mkv".into(),
            remote: remote(),
            file_len: 10,
            range_len: 4,
            concurrency: 1,
        };
        let err = pull_file(src, &spec).await.expect_err("wrong shape");
        assert!(matches!(err, TransferError::Path(_)), "{err}");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resume_oob_sidecar_range_is_sidecar_error() {
        let root = scratch("resume-oob");
        let title = TitleId::movie("603").expect("id");
        let rel = staging_path(&title, "The.Matrix.(1999).mkv").expect("staging");
        let staged = root.join(&rel);
        let mut partial = staged.clone();
        partial.as_mut_os_string().push(".partial");
        let mut sidecar_path = staged.clone();
        sidecar_path.as_mut_os_string().push(".partial.b3");
        fs::create_dir_all(partial.parent().expect("parent")).expect("mkdir");
        let body = b"abcdefghij".to_vec();
        {
            let mut f = File::create(&partial).expect("partial");
            f.set_len(body.len() as u64).expect("len");
            f.write_all(&body).expect("write");
            f.sync_all().expect("sync");
        }
        fs::write(
            &sidecar_path,
            r#"{"version":1,"file_len":10,"range_len":4,"ranges":[{"offset":0,"len":999,"blake3":"abc"}]}"#,
        )
        .expect("sidecar");

        let src = Arc::new(Mem {
            body: body.clone(),
            hits: Mutex::new(Vec::new()),
        });
        let spec = PullSpec {
            library_root: root.clone(),
            title_id: title,
            final_name: "The.Matrix.(1999).mkv".into(),
            remote: remote(),
            file_len: body.len() as u64,
            range_len: 4,
            concurrency: 1,
        };
        let err = match pull_file(src.clone(), &spec).await {
            Err(err) => err,
            Ok(_) => panic!("OOB sidecar must fail"),
        };
        assert!(
            matches!(err, TransferError::Sidecar(_)),
            "OOB sidecar must be Sidecar, got {err}"
        );
        assert!(
            src.hits.lock().expect("hits").is_empty(),
            "must not allocate/fetch after OOB sidecar"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resume_skips_recorded_ranges_and_uses_sidecar_range_len() {
        let root = scratch("resume");
        let title = TitleId::movie("603").expect("id");
        let rel = staging_path(&title, "The.Matrix.(1999).mkv").expect("staging");
        let staged = root.join(&rel);
        let mut partial = staged.clone();
        partial.as_mut_os_string().push(".partial");
        let mut sidecar_path = staged.clone();
        sidecar_path.as_mut_os_string().push(".partial.b3");
        fs::create_dir_all(partial.parent().expect("parent")).expect("mkdir");
        let body = b"abcdefghij".to_vec();
        {
            let mut f = File::create(&partial).expect("partial");
            f.set_len(body.len() as u64).expect("len");
            f.write_all(&body[..4]).expect("write");
            f.sync_all().expect("sync");
        }
        let mut sc = Sidecar::new(body.len() as u64, 4);
        sc.record(0, 4, Blake3Hex::of_bytes(&body[..4]).to_string());
        sidecar::save(&sidecar_path, &sc).expect("sidecar");

        let src = Arc::new(Mem {
            body: body.clone(),
            hits: Mutex::new(Vec::new()),
        });
        let spec = PullSpec {
            library_root: root.clone(),
            title_id: title,
            final_name: "The.Matrix.(1999).mkv".into(),
            remote: remote(),
            file_len: body.len() as u64,
            range_len: 64, // must be ignored; sidecar says 4
            concurrency: 1,
        };
        let out = pull_file(src.clone(), &spec).await.expect("resume");
        assert_eq!(fs::read(&out.staged).expect("read"), body);
        assert_eq!(out.resumed_ranges, vec![(0, 4)]);
        let hits = src.hits.lock().expect("hits").clone();
        assert!(
            !hits.iter().any(|(off, _)| *off == 0),
            "completed range 0 must not be fetched again, hits={hits:?}"
        );
        assert!(hits.iter().any(|(off, len)| *off == 4 && *len == 4));
        assert!(hits.iter().any(|(off, len)| *off == 8 && *len == 2));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resume_refetches_when_sidecar_hash_does_not_match_partial() {
        let root = scratch("corrupt");
        let title = TitleId::movie("603").expect("id");
        let rel = staging_path(&title, "The.Matrix.(1999).mkv").expect("staging");
        let staged = root.join(&rel);
        let mut partial = staged.clone();
        partial.as_mut_os_string().push(".partial");
        let mut sidecar_path = staged.clone();
        sidecar_path.as_mut_os_string().push(".partial.b3");
        fs::create_dir_all(partial.parent().expect("parent")).expect("mkdir");
        let body = b"abcdefghij".to_vec();
        {
            let mut f = File::create(&partial).expect("partial");
            f.set_len(body.len() as u64).expect("len");
            f.write_all(&[0, 0, 0, 0]).expect("zeros");
            f.sync_all().expect("sync");
        }
        let mut sc = Sidecar::new(body.len() as u64, 4);
        sc.record(0, 4, Blake3Hex::of_bytes(&body[..4]).to_string());
        sidecar::save(&sidecar_path, &sc).expect("sidecar");

        let src = Arc::new(Mem {
            body: body.clone(),
            hits: Mutex::new(Vec::new()),
        });
        let spec = PullSpec {
            library_root: root.clone(),
            title_id: title,
            final_name: "The.Matrix.(1999).mkv".into(),
            remote: remote(),
            file_len: body.len() as u64,
            range_len: 4,
            concurrency: 1,
        };
        let out = pull_file(src.clone(), &spec).await.expect("refetch");
        assert_eq!(fs::read(&out.staged).expect("read"), body);
        let hits = src.hits.lock().expect("hits").clone();
        assert!(
            hits.iter().any(|(off, _)| *off == 0),
            "corrupt recorded range must be fetched again, hits={hits:?}"
        );
        let _ = fs::remove_dir_all(root);
    }
}
