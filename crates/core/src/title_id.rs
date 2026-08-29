//! TitleId is identity: `kind:source:id`. Never a path string.

use std::fmt;
use std::str::FromStr;

/// Library title kind. Paired with a single identity source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TitleKind {
    Movie,
    Series,
    Album,
}

impl TitleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Album => "album",
        }
    }

    fn parse(raw: &str) -> Result<Self, TitleIdError> {
        match raw {
            "movie" => Ok(Self::Movie),
            "series" => Ok(Self::Series),
            "album" => Ok(Self::Album),
            other => Err(TitleIdError::InvalidKind(other.to_string())),
        }
    }
}

/// Identity authority for a TitleId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TitleSource {
    Tmdb,
    Tvdb,
    Mbid,
}

impl TitleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Tvdb => "tvdb",
            Self::Mbid => "mbid",
        }
    }

    /// Token used inside PathSchema folders: `{tmdb-603}`.
    pub fn path_token(self) -> &'static str {
        self.as_str()
    }

    fn parse(raw: &str) -> Result<Self, TitleIdError> {
        match raw {
            "tmdb" => Ok(Self::Tmdb),
            "tvdb" => Ok(Self::Tvdb),
            "mbid" => Ok(Self::Mbid),
            other => Err(TitleIdError::InvalidSource(other.to_string())),
        }
    }
}

/// Stable identity. Music remasters key by MBID, not folder year.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TitleId {
    kind: TitleKind,
    source: TitleSource,
    id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TitleIdError {
    #[error("invalid TitleId `{0}`")]
    Invalid(String),
    #[error("invalid TitleId kind `{0}`")]
    InvalidKind(String),
    #[error("invalid TitleId source `{0}`")]
    InvalidSource(String),
    #[error("invalid TitleId pairing {kind}:{authority}")]
    InvalidPairing { kind: String, authority: String },
    #[error("invalid TitleId id `{0}`")]
    InvalidId(String),
}

impl TitleId {
    pub fn movie(tmdb_id: impl Into<String>) -> Result<Self, TitleIdError> {
        Self::from_parts(TitleKind::Movie, TitleSource::Tmdb, tmdb_id.into())
    }

    pub fn series(tvdb_id: impl Into<String>) -> Result<Self, TitleIdError> {
        Self::from_parts(TitleKind::Series, TitleSource::Tvdb, tvdb_id.into())
    }

    pub fn album(mbid: impl Into<String>) -> Result<Self, TitleIdError> {
        Self::from_parts(TitleKind::Album, TitleSource::Mbid, mbid.into())
    }

    pub fn kind(&self) -> TitleKind {
        self.kind
    }

    pub fn source(&self) -> TitleSource {
        self.source
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Serialize as `kind:source:id` (example `movie:tmdb:603`).
    pub fn render(&self) -> String {
        format!(
            "{}:{}:{}",
            self.kind.as_str(),
            self.source.as_str(),
            self.id
        )
    }

    /// Parse `kind:source:id`. Invalid kind/source/id is an error, never a silent TitleId.
    pub fn parse(raw: &str) -> Result<Self, TitleIdError> {
        let mut parts = raw.splitn(3, ':');
        let kind = parts
            .next()
            .ok_or_else(|| TitleIdError::Invalid(raw.to_string()))?;
        let source = parts
            .next()
            .ok_or_else(|| TitleIdError::Invalid(raw.to_string()))?;
        let id = parts
            .next()
            .ok_or_else(|| TitleIdError::Invalid(raw.to_string()))?;
        if kind.is_empty() || source.is_empty() {
            return Err(TitleIdError::Invalid(raw.to_string()));
        }
        let kind = TitleKind::parse(kind)?;
        let source = TitleSource::parse(source)?;
        Self::from_parts(kind, source, id.to_string())
    }

    fn from_parts(kind: TitleKind, source: TitleSource, id: String) -> Result<Self, TitleIdError> {
        if !valid_pair(kind, source) {
            return Err(TitleIdError::InvalidPairing {
                kind: kind.as_str().to_string(),
                authority: source.as_str().to_string(),
            });
        }
        if !valid_id(source, &id) {
            return Err(TitleIdError::InvalidId(id));
        }
        let id = if source == TitleSource::Mbid {
            id.to_ascii_lowercase()
        } else {
            id
        };
        Ok(Self { kind, source, id })
    }
}

fn valid_pair(kind: TitleKind, source: TitleSource) -> bool {
    matches!(
        (kind, source),
        (TitleKind::Movie, TitleSource::Tmdb)
            | (TitleKind::Series, TitleSource::Tvdb)
            | (TitleKind::Album, TitleSource::Mbid)
    )
}

fn valid_id(source: TitleSource, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    match source {
        TitleSource::Tmdb | TitleSource::Tvdb => id.bytes().all(|b| b.is_ascii_digit()),
        TitleSource::Mbid => is_mbid(id),
    }
}

fn is_mbid(id: &str) -> bool {
    let mut parts = id.split('-');
    let Some(a) = parts.next() else {
        return false;
    };
    let Some(b) = parts.next() else {
        return false;
    };
    let Some(c) = parts.next() else {
        return false;
    };
    let Some(d) = parts.next() else {
        return false;
    };
    let Some(e) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    a.len() == 8
        && b.len() == 4
        && c.len() == 4
        && d.len() == 4
        && e.len() == 12
        && [a, b, c, d, e]
            .into_iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl fmt::Display for TitleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl FromStr for TitleId {
    type Err = TitleIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAYER_MBID: &str = "0f82b02e-c6cd-4242-b195-93d4bf3e0d63";

    #[test]
    fn title_id_movie_round_trip() {
        let id = TitleId::parse("movie:tmdb:603").expect("parse");
        assert_eq!(id.kind(), TitleKind::Movie);
        assert_eq!(id.source(), TitleSource::Tmdb);
        assert_eq!(id.id(), "603");
        assert_eq!(id.render(), "movie:tmdb:603");
        assert_eq!(TitleId::parse(&id.render()).expect("reparse"), id);
        assert_eq!(TitleId::movie("603").expect("movie"), id);
    }

    #[test]
    fn title_id_series_round_trip() {
        let id = TitleId::parse("series:tvdb:79126").expect("parse");
        assert_eq!(id.kind(), TitleKind::Series);
        assert_eq!(id.source(), TitleSource::Tvdb);
        assert_eq!(id.id(), "79126");
        assert_eq!(TitleId::parse(&id.render()).expect("reparse"), id);
    }

    #[test]
    fn title_id_album_round_trip() {
        let serialized = format!("album:mbid:{RELAYER_MBID}");
        let id = TitleId::parse(&serialized).expect("parse");
        assert_eq!(id.kind(), TitleKind::Album);
        assert_eq!(id.source(), TitleSource::Mbid);
        assert_eq!(id.id(), RELAYER_MBID);
        assert_eq!(TitleId::parse(&id.render()).expect("reparse"), id);
        assert_eq!(TitleId::album(RELAYER_MBID).expect("album"), id);
    }

    #[test]
    fn title_id_mbid_normalizes_to_lowercase() {
        let upper = format!("album:mbid:{}", RELAYER_MBID.to_ascii_uppercase());
        let id = TitleId::parse(&upper).expect("parse");
        assert_eq!(id.id(), RELAYER_MBID);
        assert_eq!(id, TitleId::album(RELAYER_MBID).expect("album"));
        assert_eq!(
            TitleId::album(RELAYER_MBID.to_ascii_uppercase()).expect("ctor"),
            id
        );
    }

    #[test]
    fn title_id_identity_law() {
        for raw in [
            "movie:tmdb:603",
            "series:tvdb:79126",
            "album:mbid:0f82b02e-c6cd-4242-b195-93d4bf3e0d63",
        ] {
            let id = TitleId::parse(raw).expect("parse");
            assert_eq!(TitleId::parse(&id.render()).expect("render-parse"), id);
        }
    }

    #[test]
    fn title_id_invalid_kind_source_id_are_errors() {
        assert!(matches!(
            TitleId::parse("film:tmdb:603"),
            Err(TitleIdError::InvalidKind(_))
        ));
        assert!(matches!(
            TitleId::parse("movie:imdb:603"),
            Err(TitleIdError::InvalidSource(_))
        ));
        assert!(matches!(
            TitleId::parse("movie:tmdb:"),
            Err(TitleIdError::InvalidId(_))
        ));
        assert!(matches!(
            TitleId::parse("movie:tmdb:abc"),
            Err(TitleIdError::InvalidId(_))
        ));
        assert!(matches!(
            TitleId::parse("album:mbid:not-a-uuid"),
            Err(TitleIdError::InvalidId(_))
        ));
        assert!(matches!(
            TitleId::parse("movie:tmdb"),
            Err(TitleIdError::Invalid(_))
        ));
        assert!(TitleId::parse("").is_err());
        assert!(TitleId::parse("movie:tvdb:1").is_err());
        assert!(TitleId::parse("series:tmdb:1").is_err());
        assert!(TitleId::parse("album:tmdb:1").is_err());
    }
}
