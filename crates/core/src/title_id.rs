//! TitleId is identity: `kind:source:id`. Never a path string.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Library title kind. Paired with a single identity source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
///
/// `Key` is the identity the library itself carries: the dotted
/// `Title.(Year)` folder that Radarr/Sonarr/Lidarr and the operator's own
/// naming rules produce, normalised by [`title_key`]. It is what the planner
/// compares against the disk, because this library has no ID tokens in its
/// paths. `Tmdb`/`Tvdb`/`Mbid` are the *arr authorities and still travel on the
/// wire for holds, wants, and unmonitor; the daemon bridges them to keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TitleSource {
    Tmdb,
    Tvdb,
    Mbid,
    Key,
}

impl TitleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Tvdb => "tvdb",
            Self::Mbid => "mbid",
            Self::Key => "key",
        }
    }

    fn parse(raw: &str) -> Result<Self, TitleIdError> {
        match raw {
            "tmdb" => Ok(Self::Tmdb),
            "tvdb" => Ok(Self::Tvdb),
            "mbid" => Ok(Self::Mbid),
            "key" => Ok(Self::Key),
            other => Err(TitleIdError::InvalidSource(other.to_string())),
        }
    }
}

/// Normalise a display title to its comparison key: NFKC-ish ASCII fold,
/// lowercase, alphanumerics only. `The.Matrix`, `The Matrix`, and
/// `the-matrix` all key to `thematrix`. Mirrors the operator's proven
/// `normalize_key`, so a folder the old sync accepted as "already present"
/// stays present.
pub fn title_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            }
        } else if let Some(folded) = fold_latin(c) {
            out.push_str(folded);
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Drop a trailing `.(YYYY)` / ` (YYYY)` / `(YYYY)` display year.
pub fn strip_trailing_year(text: &str) -> &str {
    let trimmed = text.trim_end();
    let Some(open) = trimmed.rfind('(') else {
        return trimmed;
    };
    let tail = &trimmed[open..];
    let is_year =
        tail.len() == 6 && tail.ends_with(')') && tail[1..5].bytes().all(|b| b.is_ascii_digit());
    if !is_year {
        return trimmed;
    }
    trimmed[..open].trim_end_matches(['.', ' '])
}

/// Best-effort Latin fold for the accented letters that show up in titles.
fn fold_latin(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => "a",
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => "i",
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' | 'Ø' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => "u",
        'ñ' | 'Ñ' => "n",
        'ç' | 'Ç' => "c",
        'ß' => "ss",
        'æ' | 'Æ' => "ae",
        'œ' | 'Œ' => "oe",
        _ => return None,
    })
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

    /// Path-derived movie identity: `movie:key:<title_key>.<year>`.
    pub fn movie_key(title: &str, year: u16) -> Result<Self, TitleIdError> {
        Self::from_parts(
            TitleKind::Movie,
            TitleSource::Key,
            format!("{}.{year}", title_key(title)),
        )
    }

    /// Path-derived series identity: `series:key:<title_key>.<year>`.
    pub fn series_key(title: &str, year: u16) -> Result<Self, TitleIdError> {
        Self::from_parts(
            TitleKind::Series,
            TitleSource::Key,
            format!("{}.{year}", title_key(title)),
        )
    }

    /// Path-derived album identity: `album:key:<artist_key>.<album_key>`.
    ///
    /// No year: a remaster (`Relayer.(1974)` vs `Relayer.(2013)`) is the same
    /// album for "already present" purposes, exactly as the old sync treated it.
    pub fn album_key(artist: &str, album: &str) -> Result<Self, TitleIdError> {
        Self::from_parts(
            TitleKind::Album,
            TitleSource::Key,
            format!(
                "{}.{}",
                title_key(artist),
                title_key(strip_trailing_year(album))
            ),
        )
    }

    /// Whether this identity was derived from a library path rather than an
    /// *arr authority.
    pub fn is_key(&self) -> bool {
        self.source == TitleSource::Key
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

    /// Filesystem-safe rendering of the same identity, for staging directory names.
    ///
    /// [`Self::render`] is colon-formed and is the identity on the wire and in the
    /// store, but a colon cannot appear in a directory name on SMB, exFAT, or NTFS.
    /// Staging paths use this form instead (`movie-tmdb-603`).
    pub fn staging_token(&self) -> String {
        format!(
            "{}-{}-{}",
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
            | (_, TitleSource::Key)
    )
}

fn valid_id(source: TitleSource, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    match source {
        // Leading zeros would mint two identities for one title (`0603` vs `603`),
        // and TitleId is the key story 1.3 indexes on.
        TitleSource::Tmdb | TitleSource::Tvdb => {
            id.bytes().all(|b| b.is_ascii_digit()) && !(id.len() > 1 && id.starts_with('0'))
        }
        TitleSource::Mbid => is_mbid(id),
        TitleSource::Key => is_key_id(id),
    }
}

/// `key` ids are `<segment>(.<segment>)*` where each segment is one or more
/// lowercase alphanumerics — exactly what [`title_key`] emits joined by `.`.
/// No whitespace, no path separators, so the id doubles as a staging token.
fn is_key_id(id: &str) -> bool {
    let mut segments = id.split('.');
    segments.all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_alphanumeric() && !c.is_uppercase())
    })
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

impl Serialize for TitleId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.render())
    }
}

impl<'de> Deserialize<'de> for TitleId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        TitleId::parse(&raw).map_err(serde::de::Error::custom)
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

    #[test]
    fn numeric_ids_reject_leading_zeros() {
        // `0603` and `603` would otherwise be two identities for one title.
        assert!(matches!(
            TitleId::parse("movie:tmdb:0603"),
            Err(TitleIdError::InvalidId(_))
        ));
        assert!(matches!(
            TitleId::series("079126"),
            Err(TitleIdError::InvalidId(_))
        ));
        // A bare zero is still a legal id; only *leading* zeros are refused.
        assert!(TitleId::movie("0").is_ok());
        assert!(TitleId::movie("603").is_ok());
    }

    #[test]
    fn key_ids_normalise_title_and_carry_year_except_albums() {
        let matrix = TitleId::movie_key("The.Matrix", 1999).expect("key");
        assert_eq!(matrix.render(), "movie:key:thematrix.1999");
        assert_eq!(
            TitleId::movie_key("The Matrix", 1999).expect("spaces"),
            matrix
        );
        assert_eq!(
            TitleId::movie_key("the-matrix", 1999).expect("hyphen"),
            matrix
        );
        assert_ne!(
            TitleId::movie_key("The.Matrix", 2003).expect("year"),
            matrix
        );
        assert!(matrix.is_key());
        assert!(!TitleId::movie("603").expect("tmdb").is_key());

        let wire = TitleId::series_key("The.Wire", 2002).expect("series");
        assert_eq!(wire.render(), "series:key:thewire.2002");
        assert_eq!(wire.staging_token(), "series-key-thewire.2002");

        let relayer = TitleId::album_key("Yes", "Relayer").expect("album");
        assert_eq!(relayer.render(), "album:key:yes.relayer");
        // Remaster years never split an album identity.
        assert_eq!(
            TitleId::album_key("Yes", "Relayer.(2013)").expect("remaster"),
            relayer
        );
        assert_eq!(
            TitleId::album_key("Yes", "Relayer (1974)").expect("spaced"),
            relayer
        );
        assert_eq!(strip_trailing_year("OK Computer.(1997)"), "OK Computer");
        assert_eq!(strip_trailing_year("1917"), "1917");
        assert_eq!(
            strip_trailing_year("Blade.Runner.(2049).(2017)"),
            "Blade.Runner.(2049)"
        );
        assert_eq!(
            TitleId::parse("movie:key:thematrix.1999").expect("parse"),
            matrix
        );
        assert!(TitleId::parse("movie:key:").is_err());
        assert!(TitleId::parse("movie:key:The.Matrix").is_err());
        assert!(TitleId::parse("movie:key:the matrix").is_err());
        assert!(TitleId::movie_key("!!!", 1999).is_err());
    }

    #[test]
    fn title_key_folds_accents_and_keeps_non_latin_letters() {
        assert_eq!(title_key("Amélie"), "amelie");
        assert_eq!(title_key("It's.Always.Sunny"), "itsalwayssunny");
        assert_eq!(
            title_key("Spider-Man: Brand New Day"),
            "spidermanbrandnewday"
        );
        assert_eq!(title_key("日本語"), "日本語");
        assert_eq!(title_key("Straße"), "strasse");
    }

    #[test]
    fn staging_token_is_filesystem_safe_and_distinct_from_render() {
        let movie = TitleId::movie("603").expect("movie");
        assert_eq!(movie.render(), "movie:tmdb:603");
        assert_eq!(movie.staging_token(), "movie-tmdb-603");
        let album = TitleId::album(RELAYER_MBID).expect("album");
        assert_eq!(album.staging_token(), format!("album-mbid-{RELAYER_MBID}"));
        for id in [&movie, &album] {
            assert!(
                !id.staging_token().contains(':'),
                "staging tokens must survive SMB/exFAT/NTFS"
            );
        }
    }
}
