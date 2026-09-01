//! Home-side library layout, grabber=None planner, and apply.

use std::fs;
use std::path::{Path, PathBuf};

use mediaops_core::{Bytes, Placement, TitleId, free_bytes, parse_placement};

mod apply;
mod plan;

pub use apply::{ApplyCtx, ApplyError, ApplyReport, InstalledCopy, apply};
pub use plan::{PlanRequest, Planned, plan_actions};

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

pub fn run_service_unit(exec_start: &str) -> String {
    format!(
        "[Unit]\nDescription=mediaops run\n\n[Service]\nType=oneshot\nTimeoutStartSec=infinity\nExecStart={exec_start}\n"
    )
}

pub fn run_timer_unit() -> String {
    "[Unit]\nDescription=mediaops run timer\n\n[Timer]\nOnBootSec=1min\nOnUnitInactiveSec=5min\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
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

/// Walk `movies`/`series`/`music` for TitleId → schema path. Used when a v4
/// title-index row has an empty path.
pub fn scan_schema_files(root: &Path) -> Result<Vec<(TitleId, PathBuf, Placement)>, LibraryError> {
    let mut out = Vec::new();
    for kind in ["movies", "series", "music"] {
        let dir = root.join(kind);
        if !dir.is_dir() {
            continue;
        }
        scan_kind_dir(root, &dir, &mut out)?;
    }
    Ok(out)
}

fn scan_kind_dir(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(TitleId, PathBuf, Placement)>,
) -> Result<(), LibraryError> {
    let reader = fs::read_dir(dir).map_err(|err| LibraryError::io(dir, err))?;
    for entry in reader {
        let entry = entry.map_err(|err| LibraryError::io(dir, err))?;
        let path = entry.path();
        if path.is_dir() {
            let inner = fs::read_dir(&path).map_err(|err| LibraryError::io(&path, err))?;
            for file in inner {
                let file = file.map_err(|err| LibraryError::io(&path, err))?;
                let file_path = file.path();
                if !file_path.is_file() {
                    continue;
                }
                let Ok(rel) = file_path.strip_prefix(root) else {
                    continue;
                };
                if let Ok((title_id, placement)) = parse_placement(rel) {
                    out.push((title_id, rel.to_path_buf(), placement));
                }
            }
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
}
