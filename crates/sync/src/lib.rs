//! Home-side library layout, grabber=None planner, and apply.

use std::fs;
use std::path::{Path, PathBuf};

use mediaops_core::{Blake3Hex, Bytes, InstalledFile, TitleIndexRepo, free_bytes};

mod apply;
mod hold;
mod plan;
mod reclaim;

pub use apply::{
    ApplyCtx, ApplyError, ApplyReport, CopyFailure, InstalledCopy, UnmonitorFailure, apply,
};
pub use hold::inbox;
pub use plan::{
    AUDIO_EXTENSIONS, PlanRequest, Planned, VIDEO_EXTENSIONS, is_media_file, plan_actions,
};
pub use reclaim::{
    ReclaimCandidate, ReclaimReport, apply_reclaim, preview_actions, reclaim_preview,
};

pub const SCHEMA_DIRS: &[&str] = &["movies", "series", "music", "_ops", "_incoming"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LibraryError {
    #[error("io at `{path}`: {message}")]
    Io { path: String, message: String },
    #[error("free space {free} is below min_free {min_free}")]
    Watermark { free: u64, min_free: u64 },
}

impl LibraryError {
    fn io(path: &Path, err: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        }
    }
}

pub fn ensure_layout(root: &Path) -> Result<(), LibraryError> {
    fs::create_dir_all(root).map_err(|err| LibraryError::io(root, err))?;
    for name in SCHEMA_DIRS {
        let dir = root.join(name);
        fs::create_dir_all(&dir).map_err(|err| LibraryError::io(&dir, err))?;
    }
    Ok(())
}

pub fn refuse_below_watermark(root: &Path, min_free: Bytes) -> Result<u64, LibraryError> {
    let free = free_bytes(root).map_err(|err| LibraryError::Io {
        path: root.display().to_string(),
        message: err.to_string(),
    })?;
    if free < min_free.get() {
        return Err(LibraryError::Watermark {
            free,
            min_free: min_free.get(),
        });
    }
    Ok(free)
}

/// The run service. Niced and best-effort I/O like the `media-sync` unit it
/// replaces: the library is a spinning disk shared with playback.
pub fn run_service_unit(exec_start: &str) -> String {
    format!(
        "[Unit]\nDescription=mediaops run (seedbox -> library)\nAfter=network-online.target mediaopsd-home.service\nWants=mediaopsd-home.service\n\n[Service]\nType=oneshot\nTimeoutStartSec=infinity\nNice=10\nIOSchedulingClass=best-effort\nIOSchedulingPriority=6\nExecStart={exec_start}\n"
    )
}

/// Hourly after the previous run *finishes* (`OnUnitInactiveSec`), never a
/// calendar that can overlap a long copy. Same cadence as the old timer.
pub fn run_timer_unit() -> String {
    "[Unit]\nDescription=mediaops run timer\n\n[Timer]\nOnBootSec=5min\nOnUnitInactiveSec=1h\n\n[Install]\nWantedBy=timers.target\n"
        .to_string()
}

pub fn write_user_units(unit_dir: &Path, exec_start: &str) -> Result<(), LibraryError> {
    fs::create_dir_all(unit_dir).map_err(|err| LibraryError::io(unit_dir, err))?;
    let service = unit_dir.join("mediaops-run.service");
    let timer = unit_dir.join("mediaops-run.timer");
    fs::write(&service, run_service_unit(exec_start))
        .map_err(|err| LibraryError::io(&service, err))?;
    fs::write(&timer, run_timer_unit()).map_err(|err| LibraryError::io(&timer, err))?;
    Ok(())
}

pub fn home_service_unit(exec_start: &str) -> String {
    format!(
        "[Unit]\nDescription=mediaopsd home gateway\n\n[Service]\nType=simple\nRestart=on-failure\nExecStart={exec_start}\n\n[Install]\nWantedBy=default.target\n"
    )
}

pub fn write_home_unit(unit_dir: &Path, exec_start: &str) -> Result<(), LibraryError> {
    fs::create_dir_all(unit_dir).map_err(|err| LibraryError::io(unit_dir, err))?;
    let service = unit_dir.join("mediaopsd-home.service");
    fs::write(&service, home_service_unit(exec_start))
        .map_err(|err| LibraryError::io(&service, err))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReindexReport {
    pub indexed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReindexError {
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error("title-index: {0}")]
    TitleIndex(String),
}

/// Hash on-disk schema files and [`TitleIndexRepo::record_install`].
/// Matching digest may backfill path; clash never overwrites `install_b3`.
pub async fn reindex_schema<T>(root: &Path, titles: &T) -> Result<ReindexReport, ReindexError>
where
    T: TitleIndexRepo,
    T::Error: std::fmt::Display,
{
    if !root.is_dir() {
        let err = if root.exists() {
            std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory")
        } else {
            std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory")
        };
        return Err(LibraryError::io(root, err).into());
    }
    let scanned = scan_schema_files(root)?;
    let mut indexed = 0;
    for file in scanned {
        let abs = root.join(&file.path);
        let handle = fs::File::open(&abs).map_err(|err| LibraryError::io(&abs, err))?;
        let digest = Blake3Hex::of_reader(handle).map_err(|err| LibraryError::io(&abs, err))?;
        titles
            .record_install(&file.title_id, &digest, &file.path)
            .await
            .map_err(|err| ReindexError::TitleIndex(err.to_string()))?;
        indexed += 1;
    }
    Ok(ReindexReport { indexed })
}

/// Media extensions the library scan counts as a schema file.
const LIBRARY_MEDIA: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "ts", "mov", "webm", "wmv", "flac", "mp3", "m4a", "ogg", "opus",
    "wav", "aac", "aiff",
];

/// Walk `movies`/`series`/`music` for every schema media file on disk.
///
/// Depth follows the grammar: `movies/T/*`, `series/T/[Season.NN/]*`,
/// `music/Artist/Album/[Disc.NN/]*`. `music` may be a symlink to another
/// disk (it is on this operator's library); `read_dir` follows it. Files the
/// grammar cannot place (`.converting`, `.partial`, loose scene files) are
/// skipped, never errors. An unreadable directory is an error naming it.
pub fn scan_schema_files(root: &Path) -> Result<Vec<InstalledFile>, LibraryError> {
    let mut out = Vec::new();
    for kind in ["movies", "series", "music"] {
        let dir = root.join(kind);
        if !dir.is_dir() {
            continue;
        }
        let max_depth = if kind == "music" { 4 } else { 3 };
        scan_tree(root, &dir, 1, max_depth, &mut out)?;
    }
    Ok(out)
}

fn scan_tree(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<InstalledFile>,
) -> Result<(), LibraryError> {
    let reader = fs::read_dir(dir).map_err(|err| LibraryError::io(dir, err))?;
    for entry in reader {
        let entry = entry.map_err(|err| LibraryError::io(dir, err))?;
        let path = entry.path();
        if path.is_dir() {
            if depth < max_depth {
                scan_tree(root, &path, depth + 1, max_depth, out)?;
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !LIBRARY_MEDIA.contains(&ext.as_str()) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        if let Ok(file) = InstalledFile::from_rel_path(rel) {
            out.push(file);
        }
    }
    Ok(())
}

/// Quote a systemd `ExecStart=` argv so spaces in the binary or `--state-db` path survive.
pub fn systemd_exec_start(exe: &Path, extra_args: &[&str]) -> String {
    let mut parts = vec![quote_systemd_arg(&exe.display().to_string())];
    for arg in extra_args {
        parts.push(quote_systemd_arg(arg));
    }
    parts.join(" ")
}

fn quote_systemd_arg(s: &str) -> String {
    if s.chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Warn if a well-known media-server config mentions `_incoming` or `_ops`.
pub fn media_server_warnings(search_roots: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for root in search_roots {
        if !root.exists() {
            continue;
        }
        scan_dir(root, 0, &mut out);
    }
    out
}

fn scan_dir(dir: &Path, depth: u8, out: &mut Vec<String>) {
    if depth > 3 {
        return;
    }
    let Ok(reader) = fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, depth + 1, out);
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(name.ends_with(".xml")
            || name.ends_with(".json")
            || name.ends_with(".conf")
            || name.ends_with(".yml")
            || name.ends_with(".yaml"))
        {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.len() > 1_000_000 {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if text.contains("_incoming") || text.contains("_ops") {
            out.push(format!(
                "{} mentions _incoming or _ops; not a media-server library",
                path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-sync-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn layout_creates_schema_dirs() {
        let root = scratch("layout");
        ensure_layout(&root).expect("layout");
        for name in SCHEMA_DIRS {
            assert!(root.join(name).is_dir(), "{name}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn watermark_refuses_when_min_free_is_impossible() {
        let root = scratch("water");
        ensure_layout(&root).expect("layout");
        let err = refuse_below_watermark(&root, Bytes::new(u64::MAX)).expect_err("refuse");
        assert!(matches!(err, LibraryError::Watermark { .. }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn timer_unit_has_on_unit_inactive_not_calendar() {
        let text = run_timer_unit();
        assert!(text.contains("OnUnitInactiveSec="));
        assert!(text.contains("OnBootSec="));
        assert!(!text.contains("OnCalendar"));
        assert!(
            !text.contains("reclaim"),
            "no leftover mediaops-reclaim.timer"
        );
    }

    #[test]
    fn no_reclaim_timer_unit_is_written() {
        let dir = scratch("no-reclaim-timer");
        write_user_units(&dir, "/opt/mediaops run").expect("units");
        assert!(dir.join("mediaops-run.timer").is_file());
        assert!(!dir.join("mediaops-reclaim.timer").exists());
        assert!(!dir.join("mediaops-reclaim.service").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn home_service_is_simple_restart_on_failure() {
        let text = home_service_unit("/opt/mediaopsd serve --role home");
        assert!(text.contains("Type=simple"));
        assert!(text.contains("Restart=on-failure"));
        assert!(text.contains("serve --role home"));
        assert!(!text.contains("OnCalendar"));
    }

    #[test]
    fn systemd_exec_start_quotes_spaces() {
        let line = systemd_exec_start(
            Path::new("/opt/my bin/mediaops"),
            &["--state-db", "/tmp/a b/state.db", "run"],
        );
        assert_eq!(
            line,
            "\"/opt/my bin/mediaops\" --state-db \"/tmp/a b/state.db\" run"
        );
    }

    #[test]
    fn media_server_config_mentioning_incoming_warns() {
        let root = scratch("jelly");
        fs::create_dir_all(root.join("jellyfin")).expect("mkdir");
        fs::write(
            root.join("jellyfin/system.xml"),
            "<Library>_incoming</Library>",
        )
        .expect("write");
        let warns = media_server_warnings(&[root.join("jellyfin")]);
        assert_eq!(warns.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn media_server_warnings_skip_missing_wrong_ext_large_and_deep() {
        let missing = PathBuf::from("/no/such/mediaops-jelly-root");
        assert!(media_server_warnings(&[missing]).is_empty());

        let root = scratch("warn-skip");
        fs::write(root.join("notes.txt"), "_incoming").expect("txt");
        fs::write(root.join("tiny.conf"), "ok").expect("conf");
        let big = vec![b'x'; 1_000_001];
        let mut big_xml = b"<Library>_ops</Library>".to_vec();
        big_xml.extend(big);
        fs::write(root.join("huge.xml"), big_xml).expect("huge");
        let deep = root.join("a").join("b").join("c").join("d");
        fs::create_dir_all(&deep).expect("deep");
        fs::write(deep.join("system.xml"), "<Library>_incoming</Library>").expect("deep xml");
        assert!(
            media_server_warnings(std::slice::from_ref(&root)).is_empty(),
            "txt/huge/deep must be skipped"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_user_units_and_home_unit_use_expected_names() {
        let dir = scratch("units");
        write_user_units(&dir, "/opt/mediaops run").expect("run units");
        write_home_unit(&dir, "/opt/mediaopsd serve --role home").expect("home unit");
        let service = fs::read_to_string(dir.join("mediaops-run.service")).expect("service");
        let timer = fs::read_to_string(dir.join("mediaops-run.timer")).expect("timer");
        let home = fs::read_to_string(dir.join("mediaopsd-home.service")).expect("home");
        assert!(service.contains("ExecStart=/opt/mediaops run"));
        assert!(timer.contains("OnUnitInactiveSec="));
        assert!(!timer.contains("OnCalendar"));
        assert!(home.contains("ExecStart=/opt/mediaopsd serve --role home"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scan_schema_files_finds_movie_episodes_tracks_and_skips_junk() {
        let root = scratch("scan");
        ensure_layout(&root).expect("layout");
        let files = [
            "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
            "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.The.Target.mkv",
            "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E02.mkv",
            "music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac",
            "music/Radiohead/OK.Computer.(1997)/Disc.02/OK.Computer.(1997).01.Airbag.flac",
        ];
        for rel in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(&path, b"x").expect("write");
        }
        fs::write(root.join("movies/not-schema.mkv"), b"x").expect("loose file");
        fs::write(
            root.join("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv.converting"),
            b"x",
        )
        .expect("converting");
        fs::write(root.join("movies/The.Matrix.(1999)/poster.jpg"), b"x").expect("poster");
        fs::create_dir_all(root.join("movies/The.Matrix.(1999)/extra")).expect("nested");
        fs::write(root.join("movies/The.Matrix.(1999)/extra/note.txt"), b"x").expect("nested file");
        let scanned = scan_schema_files(&root).expect("scan");
        let mut paths: Vec<&str> = scanned.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        let mut expected: Vec<&str> = files.to_vec();
        expected.sort_unstable();
        assert_eq!(paths, expected);
        let ids: Vec<String> = scanned.iter().map(|f| f.title_id.render()).collect();
        assert!(
            ids.contains(&"movie:key:thematrix.1999".to_string()),
            "{ids:?}"
        );
        assert_eq!(
            ids.iter()
                .filter(|i| *i == "series:key:thewire.2002")
                .count(),
            2,
            "two episodes, one show identity, two files"
        );
        assert!(
            ids.contains(&"album:key:radiohead.okcomputer".to_string()),
            "{ids:?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scan_follows_a_music_symlink_like_this_library_has() {
        let root = scratch("scan-symlink");
        ensure_layout(&root).expect("layout");
        fs::remove_dir(root.join("music")).expect("rm music");
        let elsewhere = scratch("scan-symlink-target");
        let track = elsewhere.join("Yes/Relayer.(1974)/Relayer.(1974).01.Gates.flac");
        fs::create_dir_all(track.parent().expect("parent")).expect("mkdir");
        fs::write(&track, b"x").expect("write");
        std::os::unix::fs::symlink(&elsewhere, root.join("music")).expect("symlink");
        let scanned = scan_schema_files(&root).expect("scan");
        assert_eq!(scanned.len(), 1, "{scanned:?}");
        assert_eq!(
            scanned[0].path,
            "music/Yes/Relayer.(1974)/Relayer.(1974).01.Gates.flac"
        );
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(elsewhere);
    }

    /// Path-keyed in-memory index, mirroring the store contract.
    struct MemTitles {
        rows: std::sync::Mutex<Vec<mediaops_core::TitleIndexEntry>>,
    }

    impl MemTitles {
        fn new() -> Self {
            Self {
                rows: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl TitleIndexRepo for MemTitles {
        type Error = mediaops_core::TitleIndexError;

        async fn get(
            &self,
            title_id: &mediaops_core::TitleId,
        ) -> Result<Vec<mediaops_core::TitleIndexEntry>, Self::Error> {
            Ok(self
                .rows
                .lock()
                .expect("lock")
                .iter()
                .filter(|r| r.title_id() == title_id)
                .cloned()
                .collect())
        }

        async fn get_path(
            &self,
            path: &str,
        ) -> Result<Option<mediaops_core::TitleIndexEntry>, Self::Error> {
            Ok(self
                .rows
                .lock()
                .expect("lock")
                .iter()
                .find(|r| r.path() == path)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<mediaops_core::TitleIndexEntry>, Self::Error> {
            Ok(self.rows.lock().expect("lock").clone())
        }

        async fn record_install(
            &self,
            title_id: &mediaops_core::TitleId,
            digest: &Blake3Hex,
            path: &str,
        ) -> Result<(), Self::Error> {
            let mut rows = self.rows.lock().expect("lock");
            if let Some(existing) = rows.iter().find(|r| r.path() == path) {
                if existing.install_b3() != digest {
                    return Err(mediaops_core::TitleIndexError::InstallDigestImmutable);
                }
                return Ok(());
            }
            if let Some(blank) = rows
                .iter_mut()
                .find(|r| r.title_id() == title_id && r.path_missing() && r.install_b3() == digest)
            {
                *blank = mediaops_core::TitleIndexEntry::new(
                    blank.title_id().clone(),
                    path.to_string(),
                    blank.install_b3().clone(),
                    blank.current_b3().clone(),
                );
                return Ok(());
            }
            rows.push(mediaops_core::TitleIndexEntry::new(
                title_id.clone(),
                path.to_string(),
                digest.clone(),
                digest.clone(),
            ));
            Ok(())
        }

        async fn record_replace(
            &self,
            path: &str,
            current_b3: &Blake3Hex,
        ) -> Result<(), Self::Error> {
            let mut rows = self.rows.lock().expect("lock");
            let existing = rows
                .iter_mut()
                .find(|r| r.path() == path)
                .ok_or(mediaops_core::TitleIndexError::NotInstalled)?;
            *existing = mediaops_core::TitleIndexEntry::new(
                existing.title_id().clone(),
                existing.path().to_string(),
                existing.install_b3().clone(),
                current_b3.clone(),
            );
            Ok(())
        }

        async fn import_rows(
            &self,
            rows: &[mediaops_core::TitleIndexEntry],
        ) -> Result<(), Self::Error> {
            let mut vec = self.rows.lock().expect("lock");
            if !vec.is_empty() {
                return Err(mediaops_core::TitleIndexError::NotEmpty);
            }
            vec.extend(rows.iter().cloned());
            Ok(())
        }

        async fn rewrite_absolute_prefix(
            &self,
            old_root: &str,
            new_root: &str,
        ) -> Result<u64, Self::Error> {
            let mut rows = self.rows.lock().expect("lock");
            let mut rewritten = 0_u64;
            for row in rows.iter_mut() {
                let Some(new_path) =
                    mediaops_core::rewrite_absolute_under(row.path(), old_root, new_root)
                else {
                    continue;
                };
                *row = mediaops_core::TitleIndexEntry::new(
                    row.title_id().clone(),
                    new_path,
                    row.install_b3().clone(),
                    row.current_b3().clone(),
                );
                rewritten += 1;
            }
            Ok(rewritten)
        }
    }

    const MATRIX: &str = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";

    #[tokio::test]
    async fn reindex_records_install_b3_and_backfills_path() {
        let root = scratch("reindex");
        ensure_layout(&root).expect("layout");
        let rel = PathBuf::from(MATRIX);
        let path = root.join(&rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, b"orig").expect("write");
        let titles = MemTitles::new();
        let title = mediaops_core::TitleId::movie_key("The.Matrix", 1999).expect("title");
        let digest = Blake3Hex::of_bytes(b"orig");
        titles
            .import_rows(&[mediaops_core::TitleIndexEntry::new(
                title.clone(),
                "",
                digest.clone(),
                digest.clone(),
            )])
            .await
            .expect("seed empty path");
        let report = reindex_schema(&root, &titles).await.expect("reindex");
        assert_eq!(report.indexed, 1);
        let rows = titles.get(&title).await.expect("get");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].install_b3(), &digest);
        assert_eq!(rows[0].path(), MATRIX);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reindex_digest_clash_does_not_overwrite_install_b3() {
        let root = scratch("reindex-clash");
        ensure_layout(&root).expect("layout");
        let rel = PathBuf::from(MATRIX);
        let path = root.join(&rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, b"new-bytes").expect("write");
        let titles = MemTitles::new();
        let title = mediaops_core::TitleId::movie_key("The.Matrix", 1999).expect("title");
        let old = Blake3Hex::of_bytes(b"old-bytes");
        titles
            .record_install(&title, &old, MATRIX)
            .await
            .expect("seed");
        let err = reindex_schema(&root, &titles).await.expect_err("clash");
        assert!(err.to_string().contains("immutable"), "{err}");
        let rows = titles.get(&title).await.expect("get");
        assert_eq!(rows[0].install_b3(), &old);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reindex_unreadable_schema_file_is_io() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("reindex-io");
        ensure_layout(&root).expect("layout");
        let rel = PathBuf::from(MATRIX);
        let path = root.join(&rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, b"orig").expect("write");
        let movies = root.join("movies");
        let restore = || {
            let _ = fs::set_permissions(&movies, fs::Permissions::from_mode(0o755));
        };
        fs::set_permissions(&movies, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read_dir(&movies).is_ok() {
            restore();
            let _ = fs::remove_dir_all(root);
            return;
        }
        let titles = MemTitles::new();
        let err = reindex_schema(&root, &titles).await.expect_err("io");
        restore();
        assert!(
            matches!(err, ReindexError::Library(LibraryError::Io { .. })),
            "{err}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reindex_missing_root_is_io() {
        let root = scratch("reindex-missing").join("no-such-library");
        let titles = MemTitles::new();
        let err = reindex_schema(&root, &titles)
            .await
            .expect_err("missing root");
        assert!(
            matches!(err, ReindexError::Library(LibraryError::Io { .. })),
            "{err}"
        );
        let _ = fs::remove_dir_all(root.parent().expect("parent"));
    }

    #[tokio::test]
    async fn reindex_after_empty_index_restores_reclaim_proof() {
        let root = scratch("reindex-proof");
        ensure_layout(&root).expect("layout");
        let rel = PathBuf::from(MATRIX);
        let path = root.join(&rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, b"orig").expect("write");
        let titles = MemTitles::new();
        let title = mediaops_core::TitleId::movie_key("The.Matrix", 1999).expect("title");
        // The remote is root-relative under a `movies` root.
        let listing = mediaops_core::RemoteEntry::from_wire_parts(
            mediaops_core::RemoteRef::from_wire_parts(
                "movies".into(),
                PathBuf::from("The.Matrix.(1999)/The.Matrix.(1999).mkv"),
            )
            .expect("ref"),
            4,
            1,
            1,
        );
        let listings = [listing];
        let kinds = mediaops_core::RootKinds::from([(
            "movies".to_string(),
            Some(mediaops_core::TitleKind::Movie),
        )]);
        let on_disk = scan_schema_files(&root).expect("scan");
        assert_eq!(on_disk.len(), 1);
        assert!(
            mediaops_core::reclaim_proved(&listings, &kinds, &[], &on_disk).is_empty(),
            "empty title_index is not proof"
        );
        let report = reindex_schema(&root, &titles).await.expect("reindex");
        assert_eq!(report.indexed, 1);
        let rows = titles.list().await.expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title_id(), &title);
        assert_eq!(rows[0].install_b3(), &Blake3Hex::of_bytes(b"orig"));
        assert_eq!(
            mediaops_core::reclaim_proved(&listings, &kinds, &rows, &on_disk).len(),
            1,
            "reindex row is reclaim proof"
        );
        let _ = fs::remove_dir_all(root);
    }
}
