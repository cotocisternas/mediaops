use std::path::{Component, Path};

use mediaops_core::{HomeObject, JobSpec, Kind, Spec, StatusBody, SyncDisposition};

pub(crate) fn placement_state(
    objects: &[HomeObject],
    job: &JobSpec,
) -> Option<(SyncDisposition, String)> {
    let Ok((identity, placement)) = mediaops_core::parse_placement(Path::new(&job.dest_rel)) else {
        return blocked("invalid destination placement");
    };
    let matches = |path: &str, title_id: &str| {
        mediaops_core::parse_placement(Path::new(path)).is_ok_and(|(id, p)| {
            p.file_key() == placement.file_key() && (id == identity || title_id == job.title_id)
        })
    };
    let root = Path::new(&job.library_root);
    if !std::fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir()) {
        return blocked("library root is missing, unreadable, or a symlink");
    }
    let mut present = false;
    for title in objects.iter().filter(|obj| obj.kind == Kind::Title) {
        let (Spec::Title(spec), StatusBody::Title(status)) = (&title.spec, &title.status) else {
            continue;
        };
        let files = status.observed_files();
        for file in &files {
            if !matches(&file.path, &spec.title_id) {
                continue;
            }
            let path = root.join(&file.path);
            if file.drifted || !regular_proof_path(root, &path) {
                return blocked(
                    "recorded placement is drifted, missing, unreadable, or traverses a symlink; explicit repair is required",
                );
            }
            present = true;
        }
        if !status.path.is_empty()
            && matches(&status.path, &spec.title_id)
            && !files.iter().any(|file| file.path == status.path)
        {
            return blocked("recorded placement lacks verified file proofs");
        }
    }
    if present {
        return Some((
            SyncDisposition::Present,
            "verified library placement already present".into(),
        ));
    }
    let mut path = root.to_path_buf();
    let components: Vec<_> = Path::new(&job.dest_rel).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(segment) = component else {
            return blocked("unsafe destination path");
        };
        path.push(segment);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if index + 1 < components.len() && meta.is_dir() => {}
            Ok(_) => {
                return blocked(
                    "destination or unsafe parent already exists without a matching proof",
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Err(_) => return blocked("destination cannot be inspected safely"),
        }
    }
    None
}

fn blocked(reason: &str) -> Option<(SyncDisposition, String)> {
    Some((SyncDisposition::Blocked, reason.into()))
}

fn regular_proof_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return false;
    }
    let mut current = root.to_path_buf();
    for (index, part) in components.iter().enumerate() {
        let Component::Normal(segment) = part else {
            return false;
        };
        current.push(segment);
        let Ok(meta) = std::fs::symlink_metadata(&current) else {
            return false;
        };
        if index + 1 == components.len() {
            if !meta.is_file() {
                return false;
            }
        } else if !meta.is_dir() {
            return false;
        }
    }
    true
}
