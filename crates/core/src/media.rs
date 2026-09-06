//! Media-file predicates the planner and Home controllers share.

use crate::walker::RemoteRef;

/// Extensions treated as library video.
pub const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "m4v", "avi", "ts", "mov", "webm", "wmv"];

/// Extensions treated as library audio.
pub const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "ogg", "opus", "wav", "aac", "aiff"];

/// True for a file the planner would consider library media.
pub fn is_media_file(remote: &RemoteRef) -> bool {
    let name = remote
        .rel_path()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    if lower.contains("sample") {
        return false;
    }
    let Some((_, ext)) = lower.rsplit_once('.') else {
        return false;
    };
    VIDEO_EXTENSIONS.contains(&ext) || AUDIO_EXTENSIONS.contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walker::RemoteRef;

    #[test]
    fn sample_and_nfo_are_not_media() {
        let mkv = RemoteRef::from_wire_parts(
            "seedbox".into(),
            std::path::PathBuf::from("movies/The.Matrix.(1999).mkv"),
        )
        .expect("r");
        let sample = RemoteRef::from_wire_parts(
            "seedbox".into(),
            std::path::PathBuf::from("movies/sample.mkv"),
        )
        .expect("r");
        let nfo = RemoteRef::from_wire_parts(
            "seedbox".into(),
            std::path::PathBuf::from("movies/The.Matrix.(1999).nfo"),
        )
        .expect("r");
        assert!(is_media_file(&mkv));
        assert!(!is_media_file(&sample));
        assert!(!is_media_file(&nfo));
    }
}
