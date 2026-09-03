//! Hold identity and the decisions-repo port (AD-8).
//!
//! [`ReleaseId`] is the durable release token (torrent infohash or usenet NZB-name
//! hash). Join is on [`HoldKey`], never a scene title. The live ⊖ decided inbox
//! is computed in `sync`. Mapping from Servarr JSON lives only in `arr`.

use crate::digest::Blake3Hex;
use crate::pathschema::{PathSchemaError, Placement, render, strip_placement, strip_scene_tags};
use crate::title_id::TitleId;
use crate::walker::RemoteRef;

/// Durable release identifier. Carried verbatim on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReleaseId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HoldError {
    #[error("empty release_id")]
    EmptyReleaseId,
    #[error("invalid release_id `{0}`")]
    InvalidReleaseId(String),
    #[error("unknown hold decision `{0}`")]
    UnknownDecision(String),
}

impl ReleaseId {
    /// Torrent: lowercase hex of Servarr `downloadId` (infohash).
    pub fn torrent(download_id: &str) -> Result<Self, HoldError> {
        if download_id.is_empty() {
            return Err(HoldError::EmptyReleaseId);
        }
        if !download_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HoldError::InvalidReleaseId(download_id.to_string()));
        }
        Ok(Self(download_id.to_ascii_lowercase()))
    }

    /// Usenet: BLAKE3 of the queue `title` (NZB name).
    pub fn usenet(title: &str) -> Result<Self, HoldError> {
        if title.is_empty() {
            return Err(HoldError::EmptyReleaseId);
        }
        Ok(Self(
            Blake3Hex::of_bytes(title.as_bytes()).as_str().to_string(),
        ))
    }

    /// Wire / store token. Empty is refused; the token is otherwise opaque.
    pub fn parse(raw: &str) -> Result<Self, HoldError> {
        if raw.is_empty() {
            return Err(HoldError::EmptyReleaseId);
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ReleaseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Inbox / decisions key. Never a scene-normalized title string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HoldKey {
    pub title_id: TitleId,
    pub release_id: ReleaseId,
}

impl HoldKey {
    pub fn new(title_id: TitleId, release_id: ReleaseId) -> Self {
        Self {
            title_id,
            release_id,
        }
    }
}

/// Live import-blocked queue item. Age is `max(0, now - added_unix)`.
///
/// `remote` / `placement` are additive planning fields (list JSON stays age/size/reason).
/// `output_path` is the *arr `outputPath` string; seedbox maps it through the allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldLiveItem {
    pub key: HoldKey,
    pub added_unix: i64,
    pub size: u64,
    pub reason: String,
    pub remote: Option<RemoteRef>,
    pub placement: Option<Placement>,
    pub output_path: Option<String>,
}

impl HoldLiveItem {
    pub fn new(key: HoldKey, added_unix: i64, size: u64, reason: impl Into<String>) -> Self {
        Self {
            key,
            added_unix,
            size,
            reason: reason.into(),
            remote: None,
            placement: None,
            output_path: None,
        }
    }

    pub fn age_secs(&self, now_unix: i64) -> u64 {
        u64::try_from(now_unix.saturating_sub(self.added_unix)).unwrap_or(0)
    }
}

/// PathSchema-preflight for `hold approve`: leftover scene tags and spaces refuse.
///
/// Strip scene tags, then `render`. Spaces or leftover REPACJ/REPACK/PROPER are
/// policy (no decision row).
pub fn preflight_approve_placement(
    title_id: &TitleId,
    placement: &Placement,
) -> Result<Placement, PathSchemaError> {
    if let Some(token) = placement_space_token(placement) {
        return Err(PathSchemaError::SpaceRefused(token.to_string()));
    }
    if let Some(token) = placement_leftover_token(placement) {
        return Err(PathSchemaError::LeftoverSceneTag(token.to_string()));
    }
    let stripped = strip_placement(placement);
    render(title_id, &stripped)?;
    Ok(stripped)
}

fn placement_tokens(placement: &Placement) -> Vec<&str> {
    match placement {
        Placement::Movie {
            title, extension, ..
        } => vec![title, extension],
        Placement::Episode {
            title, extension, ..
        } => vec![title, extension],
        Placement::Track {
            album,
            title,
            extension,
            ..
        } => vec![album, title, extension],
    }
}

fn placement_space_token(placement: &Placement) -> Option<&str> {
    placement_tokens(placement)
        .into_iter()
        .find(|token| token.chars().any(char::is_whitespace))
}

fn placement_leftover_token(placement: &Placement) -> Option<&str> {
    placement_tokens(placement)
        .into_iter()
        .find(|token| strip_scene_tags(token) != **token)
}

/// Persistable operator decision. `approved`/`rejected` so 6.2 needs no extra migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldDecision {
    Approved,
    Rejected,
}

impl HoldDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HoldError> {
        match raw {
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            other => Err(HoldError::UnknownDecision(other.to_string())),
        }
    }
}

/// Decisions table port. Adapter lives in `store`. `put` is for tests and 6.2.
///
/// A trait, not I/O: async signatures only.
#[allow(async_fn_in_trait)]
pub trait HoldsRepo: Send + Sync {
    type Error;

    async fn get(&self, key: &HoldKey) -> Result<Option<HoldDecision>, Self::Error>;
    async fn list_decided(&self) -> Result<Vec<HoldKey>, Self::Error>;
    async fn put(&self, key: &HoldKey, decision: HoldDecision) -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_release_id_is_lowercase_hex_of_download_id() {
        let id = ReleaseId::torrent("ABCDEF0123").expect("torrent");
        assert_eq!(id.as_str(), "abcdef0123");
        assert_eq!(ReleaseId::parse(id.as_str()).expect("parse"), id);
        assert!(matches!(
            ReleaseId::torrent(""),
            Err(HoldError::EmptyReleaseId)
        ));
        assert!(matches!(
            ReleaseId::torrent("not-hex"),
            Err(HoldError::InvalidReleaseId(_))
        ));
    }

    #[test]
    fn usenet_release_id_is_blake3_of_nzb_title() {
        let title = "The.Wire.S01E01.nzb";
        let id = ReleaseId::usenet(title).expect("usenet");
        assert_eq!(id.as_str(), Blake3Hex::of_bytes(title.as_bytes()).as_str());
        assert_eq!(id.as_str().len(), Blake3Hex::LEN);
        assert_ne!(
            id,
            ReleaseId::usenet("Other.Title.nzb").expect("other"),
            "join is on the NZB-name hash, never a scene title"
        );
        assert!(matches!(
            ReleaseId::usenet(""),
            Err(HoldError::EmptyReleaseId)
        ));
    }

    #[test]
    fn hold_key_identity_is_title_and_release_not_scene_name() {
        let movie = TitleId::movie("603").expect("movie");
        let series = TitleId::series("79126").expect("series");
        let album = TitleId::album("0f82b02e-c6cd-4242-b195-93d4bf3e0d63").expect("album");
        let torrent = ReleaseId::torrent("deadbeef").expect("torrent");
        let usenet = ReleaseId::usenet("Some.NZB").expect("usenet");
        let a = HoldKey::new(movie.clone(), torrent.clone());
        let b = HoldKey::new(movie, torrent);
        assert_eq!(a, b);
        assert_ne!(a, HoldKey::new(series, usenet.clone()));
        assert_ne!(
            HoldKey::new(album, usenet.clone()),
            HoldKey::new(TitleId::movie("604").expect("other"), usenet)
        );
    }

    #[test]
    fn hold_decision_tokens_round_trip() {
        let decisions = [HoldDecision::Approved, HoldDecision::Rejected];
        for d in decisions {
            let token = match d {
                HoldDecision::Approved => "approved",
                HoldDecision::Rejected => "rejected",
            };
            assert_eq!(d.as_str(), token);
            assert_eq!(HoldDecision::parse(token).expect("parse"), d);
        }
        assert_eq!(decisions.len(), 2);
        assert!(matches!(
            HoldDecision::parse("open"),
            Err(HoldError::UnknownDecision(_))
        ));
    }

    #[test]
    fn age_secs_is_max_zero_now_minus_added() {
        let item = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("603").expect("title"),
                ReleaseId::parse("abc").expect("id"),
            ),
            100,
            42,
            "blocked",
        );
        assert_eq!(item.age_secs(150), 50);
        assert_eq!(item.age_secs(100), 0);
        assert_eq!(item.age_secs(50), 0);
        assert_eq!(item.size, 42);
        assert_eq!(item.reason, "blocked");
        assert!(item.remote.is_none());
        assert!(item.placement.is_none());
    }

    #[test]
    fn approve_preflight_refuses_spaces_and_leftover_scene_tags() {
        let title = TitleId::movie("603").expect("title");
        let spaces = Placement::movie("The Matrix", 1999, "mkv");
        assert!(matches!(
            preflight_approve_placement(&title, &spaces),
            Err(PathSchemaError::SpaceRefused(_))
        ));
        let leftover = Placement::movie("The.Matrix.REPACK", 1999, "mkv");
        assert!(matches!(
            preflight_approve_placement(&title, &leftover),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        let repacj = Placement::movie("The.Matrix.REPACJ", 1999, "mkv");
        assert!(matches!(
            preflight_approve_placement(&title, &repacj),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        let proper = Placement::movie("The.Matrix.PROPER", 1999, "mkv");
        assert!(matches!(
            preflight_approve_placement(&title, &proper),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        let ok = Placement::movie("The.Matrix", 1999, "mkv");
        assert_eq!(preflight_approve_placement(&title, &ok).expect("ok"), ok);
    }

    #[test]
    fn wire_parse_rejects_empty_release_id() {
        assert!(matches!(
            ReleaseId::parse(""),
            Err(HoldError::EmptyReleaseId)
        ));
    }
}
