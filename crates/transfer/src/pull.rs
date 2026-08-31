//! PullFile: `.partial` + sidecar, resume, whole-file BLAKE3.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mediaops_core::{Blake3Hex, RemoteRef, TitleId, staging_path};
use tokio::task::JoinSet;

use crate::schedule::{plan_ranges, remaining, PendingFile, take_slots};
use crate::sidecar::{self, Sidecar};
use crate::TransferError;

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

pub struct PullOutcome {
    pub staged: PathBuf,
    pub whole_file_b3: Blake3Hex,
    pub resumed_ranges: Vec<(u64, u64)>,
}

pub async fn pull_file<S: RangeSource + 'static>(
    src: Arc<S>,
    spec: &PullSpec,
) -> Result<PullOutcome, TransferError> {
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
    if staged.exists() {
        return Err(TransferError::Path(format!(
            "staged file already exists: {}",
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
    let resumed_ranges: Vec<(u64, u64)> = sidecar
        .ranges
        .iter()
        .map(|r| (r.offset, r.len))
        .collect();

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
            let (offset, len, bytes) = joined.map_err(|err| TransferError::Join(err.to_string()))??;
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
        }
    }

    let whole = hash_file(&partial)?;
    if staged.exists() {
        return Err(TransferError::Path(format!(
            "staged file already exists: {}",
            staged.display()
        )));
    }
    fs::rename(&partial, &staged).map_err(|err| TransferError::io(&staged, err))?;
    fs::remove_file(&sidecar_path).map_err(|err| TransferError::io(&sidecar_path, err))?;
    let incoming = spec.library_root.join("_incoming");
    let _ = crate::prune_empty_incoming(&incoming);
    Ok(PullOutcome {
        staged,
        whole_file_b3: whole,
        resumed_ranges,
    })
}

fn verify_recorded_ranges(mut sidecar: Sidecar, partial: &Path) -> Result<Sidecar, TransferError> {
    if !partial.exists() {
        sidecar.ranges.clear();
        return Ok(sidecar);
    }
    let mut file = File::open(partial).map_err(|err| TransferError::io(partial, err))?;
    let mut kept = Vec::new();
    for range in sidecar.ranges.drain(..) {
        let mut buf = vec![0_u8; range.len as usize];
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
    file.sync_all().map_err(|err| TransferError::io(path, err))?;
    Ok(())
}

fn hash_file(path: &Path) -> Result<Blake3Hex, TransferError> {
    let file = File::open(path).map_err(|err| TransferError::io(path, err))?;
    Blake3Hex::of_reader(file).map_err(|err| TransferError::io(path, err))
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
        let out = pull_file(src, &spec).await.expect("pull");
        assert_eq!(fs::read(&out.staged).expect("read"), body);
        assert_eq!(out.whole_file_b3, Blake3Hex::of_bytes(&body));
        assert!(out.resumed_ranges.is_empty());
        let mut leftover = out.staged.clone();
        leftover.as_mut_os_string().push(".partial");
        assert!(!leftover.exists());
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
