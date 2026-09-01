//! NVENC transcode via ExecPort, then reversible [`mediaops_core::replace`].

use std::fs;
use std::path::{Path, PathBuf};

use mediaops_core::{
    Blake3Hex, ExecCommand, ExecPort, Placement, TitleId, VerifiedConvertingHandle, render,
    replace, staging_path,
};

use crate::EncodeError;

pub fn ffmpeg_nvenc_args(input: &Path, converting: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-i".into(),
        input.display().to_string(),
        "-c:v".into(),
        "h264_nvenc".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "copy".into(),
        converting.display().to_string(),
    ]
}

pub fn converting_path(
    library_root: &Path,
    title_id: &TitleId,
    filename: &str,
) -> Result<PathBuf, EncodeError> {
    let name = format!("{filename}.converting");
    Ok(library_root
        .join(staging_path(title_id, &name).map_err(|err| EncodeError::Path(err.to_string()))?))
}

pub fn backup_path(library_root: &Path, title_id: &TitleId, filename: &str) -> PathBuf {
    library_root
        .join("_ops")
        .join("backup-hevc-originals")
        .join(title_id.staging_token())
        .join(filename)
}

#[derive(Clone, Copy)]
pub struct TranscodeSpec<'a> {
    pub library_root: &'a Path,
    pub title_id: &'a TitleId,
    pub placement: &'a Placement,
    pub ffmpeg: &'a str,
}

/// Write `.converting` and run ffmpeg. Does **not** call `replace`.
pub async fn encode_to_converting(
    exec: &impl ExecPort,
    spec: TranscodeSpec<'_>,
) -> Result<PathBuf, EncodeError> {
    let dest_rel =
        render(spec.title_id, spec.placement).map_err(|err| EncodeError::Path(err.to_string()))?;
    let filename = dest_rel
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| EncodeError::Path("destination file name is not utf-8".into()))?;
    let live = spec.library_root.join(&dest_rel);
    let converting = converting_path(spec.library_root, spec.title_id, filename)?;
    if let Some(parent) = converting.parent() {
        fs::create_dir_all(parent).map_err(|err| EncodeError::Io(err.to_string()))?;
    }
    let args = ffmpeg_nvenc_args(&live, &converting);
    let cmd = ExecCommand::new(spec.ffmpeg, args);
    let out = exec
        .run(&cmd)
        .await
        .map_err(|err| EncodeError::Exec(err.to_string()))?;
    if out.status != 0 {
        let _ = fs::remove_file(&converting);
        return Err(EncodeError::Exec(format!("ffmpeg exited {}", out.status)));
    }
    Ok(converting)
}

pub fn replace_converting(
    spec: TranscodeSpec<'_>,
    converting: PathBuf,
) -> Result<(PathBuf, Blake3Hex), EncodeError> {
    let dest_rel =
        render(spec.title_id, spec.placement).map_err(|err| EncodeError::Path(err.to_string()))?;
    let filename = dest_rel
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| EncodeError::Path("destination file name is not utf-8".into()))?;
    let handle = VerifiedConvertingHandle::verify(spec.title_id, converting, spec.placement)
        .map_err(|err| EncodeError::Install(err.to_string()))?;
    let backup = backup_path(spec.library_root, spec.title_id, filename);
    let dest = replace(spec.library_root, spec.title_id, &handle, &backup)
        .map_err(|err| EncodeError::Install(err.to_string()))?;
    let file = fs::File::open(&dest).map_err(|err| EncodeError::Io(err.to_string()))?;
    let digest = Blake3Hex::of_reader(file).map_err(|err| EncodeError::Io(err.to_string()))?;
    Ok((dest, digest))
}

/// Write `.converting`, run ffmpeg, `replace` into the schema path.
/// On ffmpeg failure the live original is left in place (no `replace`).
pub async fn transcode_nvenc(
    exec: &impl ExecPort,
    spec: TranscodeSpec<'_>,
) -> Result<(PathBuf, Blake3Hex), EncodeError> {
    let converting = encode_to_converting(exec, spec).await?;
    replace_converting(spec, converting)
}

pub fn should_start_next(paused: bool, cap: u32) -> bool {
    !paused && cap > 0
}

/// `min(desired.max_nvenc, max(stored nvenc_cap, 1))` when HEVC NVENC is
/// present; 0 otherwise.
pub fn session_cap(max_nvenc: u32, stored_nvenc_cap: u32, hevc: bool) -> u32 {
    if !hevc {
        return 0;
    }
    max_nvenc.min(stored_nvenc_cap.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{
        ExecCommand, ExecError, ExecOutput, ExecPort, Placement, TitleId, VerifiedStagingHandle,
        install, staging_path,
    };
    use std::io::Write;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mediaops-encode-{}-{}-{}",
                std::process::id(),
                n,
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("mkdir");
            Self { path }
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct Transcript {
        calls: Mutex<Vec<ExecCommand>>,
        status: i32,
        write_converting: bool,
    }

    impl ExecPort for Transcript {
        async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
            self.calls.lock().expect("calls").push(command.clone());
            if self.write_converting {
                if let Some(out) = command.args.last() {
                    let path = PathBuf::from(out);
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).expect("mkdir");
                    }
                    fs::write(&path, b"encoded-h264").expect("write converting");
                }
            }
            Ok(ExecOutput {
                status: self.status,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    fn seed_library(lib: &Path) -> (TitleId, Placement, PathBuf) {
        let title_id = TitleId::movie("603").expect("id");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let name = "The.Matrix.(1999).mkv";
        let staged = lib.join(staging_path(&title_id, name).expect("staging"));
        fs::create_dir_all(staged.parent().expect("parent")).expect("mkdir");
        let mut f = fs::File::create(&staged).expect("create");
        f.write_all(b"original-hevc").expect("write");
        let handle =
            VerifiedStagingHandle::verify(lib, &title_id, staged, &placement).expect("verify");
        let installed = install(lib, &title_id, &handle).expect("install");
        (title_id, placement, installed.path)
    }

    #[tokio::test]
    async fn ffmpeg_argv_is_h264_nvenc_yuv420p_audio_copy() {
        let tmp = TempTree::new();
        let lib = tmp.path.join("library");
        let (title_id, placement, live) = seed_library(&lib);
        let exec = Transcript {
            calls: Mutex::new(Vec::new()),
            status: 0,
            write_converting: true,
        };
        transcode_nvenc(
            &exec,
            TranscodeSpec {
                library_root: &lib,
                title_id: &title_id,
                placement: &placement,
                ffmpeg: "ffmpeg",
            },
        )
        .await
        .expect("transcode");
        let calls = exec.calls.lock().expect("calls").clone();
        assert_eq!(calls[0].program, "ffmpeg");
        assert_eq!(calls[0].args[0], "-y");
        assert_eq!(calls[0].args[1], "-i");
        assert_eq!(calls[0].args[2], live.display().to_string());
        assert_eq!(calls[0].args[3], "-c:v");
        assert_eq!(calls[0].args[4], "h264_nvenc");
        assert_eq!(calls[0].args[5], "-pix_fmt");
        assert_eq!(calls[0].args[6], "yuv420p");
        assert_eq!(calls[0].args[7], "-c:a");
        assert_eq!(calls[0].args[8], "copy");
        assert!(calls[0].args[9].ends_with(".converting"));
    }

    #[tokio::test]
    async fn failed_ffmpeg_leaves_original_in_place_no_delete_before_replace() {
        let tmp = TempTree::new();
        let lib = tmp.path.join("library");
        let (title_id, placement, live) = seed_library(&lib);
        let exec = Transcript {
            calls: Mutex::new(Vec::new()),
            status: 1,
            write_converting: false,
        };
        let err = transcode_nvenc(
            &exec,
            TranscodeSpec {
                library_root: &lib,
                title_id: &title_id,
                placement: &placement,
                ffmpeg: "ffmpeg",
            },
        )
        .await
        .expect_err("ffmpeg fail");
        assert!(err.to_string().contains("ffmpeg exited"), "{err}");
        assert_eq!(fs::read(&live).expect("original"), b"original-hevc");
        let backup = backup_path(&lib, &title_id, "The.Matrix.(1999).mkv");
        assert!(!backup.exists(), "replace must not run on ffmpeg failure");
    }

    #[test]
    fn session_cap_is_zero_without_hevc() {
        assert_eq!(session_cap(8, 0, false), 0);
        assert_eq!(session_cap(8, 1, true), 1);
        assert_eq!(session_cap(1, 1, true), 1);
    }

    #[test]
    fn pause_skips_starting_the_next_encode() {
        assert!(!should_start_next(true, 1));
        assert!(!should_start_next(false, 0));
        assert!(should_start_next(false, 1));
    }
}
