//! Versioned library-path grammar. The only renderer/parser of library paths.
//!
//! Golden paths (dots, no spaces; year copied into folder and stem):
//! - `movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv`
//! - `series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.mkv`
//! - `music/Relayer.(2013).{mbid-0f82b02e-c6cd-4242-b195-93d4bf3e0d63}/01.The.Gates.Of.Delirium.(2013).flac`

use std::path::{Component, Path, PathBuf};

use crate::title_id::{TitleId, TitleKind, TitleSource};

/// PathSchema grammar version. Bump when render/parse rules change.
pub const GRAMMAR_VERSION: u32 = 1;

const SCENE_TAGS: &[&str] = &["REPACJ", "REPACK", "PROPER"];

/// Display placement used only to render a path. Identity stays on [`TitleId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Movie {
        title: String,
        year: u16,
        extension: String,
    },
    Episode {
        title: String,
        year: u16,
        season: u8,
        episode: u8,
        extension: String,
    },
    Track {
        album: String,
        year: u16,
        track: u8,
        title: String,
        extension: String,
    },
}

impl Placement {
    pub fn movie(title: impl Into<String>, year: u16, extension: impl Into<String>) -> Self {
        Self::Movie {
            title: title.into(),
            year,
            extension: extension.into(),
        }
    }

    pub fn episode(
        title: impl Into<String>,
        year: u16,
        season: u8,
        episode: u8,
        extension: impl Into<String>,
    ) -> Self {
        Self::Episode {
            title: title.into(),
            year,
            season,
            episode,
            extension: extension.into(),
        }
    }

    pub fn track(
        album: impl Into<String>,
        year: u16,
        track: u8,
        title: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        Self::Track {
            album: album.into(),
            year,
            track,
            title: title.into(),
            extension: extension.into(),
        }
    }
}

/// Explicit reject bins under `_ops/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectBin {
    NeedsSplit,
    NeedsYear,
}

impl RejectBin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsSplit => "needs-split",
            Self::NeedsYear => "needs-year",
        }
    }

    pub fn rel_dir(self) -> PathBuf {
        PathBuf::from("_ops").join(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathSchemaError {
    #[error("reject bin {0}")]
    RejectBin(&'static str),
    #[error("space refused in `{0}`")]
    SpaceRefused(String),
    #[error("leftover scene tag in `{0}`")]
    LeftoverSceneTag(String),
    #[error("invalid library path `{0}`")]
    Invalid(String),
    #[error("kind/placement mismatch")]
    KindMismatch,
    #[error("year mismatch between folder ({folder}) and file stem ({file})")]
    YearMismatch { folder: u16, file: u16 },
    #[error("year {0} is outside 1000..=9999")]
    InvalidYear(u16),
    #[error("empty or invalid TitleId")]
    InvalidTitleId,
    #[error("empty final name")]
    EmptyFinalName,
}

impl PathSchemaError {
    pub fn reject_bin(&self) -> Option<RejectBin> {
        match self {
            Self::RejectBin("needs-split") => Some(RejectBin::NeedsSplit),
            Self::RejectBin("needs-year") => Some(RejectBin::NeedsYear),
            _ => None,
        }
    }
}

/// Strip scene tags `REPACJ`, `REPACK`, and `PROPER` from a name.
pub fn strip_scene_tags(name: &str) -> String {
    name.split(['.', '-', '_'])
        .filter(|part| !part.is_empty() && !is_scene_tag(part))
        .collect::<Vec<_>>()
        .join(".")
}

/// Render a library-relative path from TitleId plus placement.
pub fn render(title_id: &TitleId, placement: &Placement) -> Result<PathBuf, PathSchemaError> {
    match (title_id.kind(), placement) {
        (
            TitleKind::Movie,
            Placement::Movie {
                title,
                year,
                extension,
            },
        ) => {
            let title = validate_display_token(title)?;
            let year = validate_year(*year)?;
            let extension = validate_extension(extension)?;
            let folder = title_folder(&title, year, title_id);
            let file = format!("{title}.({year}).{extension}");
            Ok(PathBuf::from("movies").join(folder).join(file))
        }
        (
            TitleKind::Series,
            Placement::Episode {
                title,
                year,
                season,
                episode,
                extension,
            },
        ) => {
            let title = validate_display_token(title)?;
            let year = validate_year(*year)?;
            let extension = validate_extension(extension)?;
            let folder = title_folder(&title, year, title_id);
            let file = format!("{title}.({year}).S{season:02}E{episode:02}.{extension}");
            Ok(PathBuf::from("series").join(folder).join(file))
        }
        (
            TitleKind::Album,
            Placement::Track {
                album,
                year,
                track,
                title,
                extension,
            },
        ) => {
            let album = validate_display_token(album)?;
            let title = validate_display_token(title)?;
            let year = validate_year(*year)?;
            let extension = validate_extension(extension)?;
            let folder = title_folder(&album, year, title_id);
            let file = format!("{track:02}.{title}.({year}).{extension}");
            Ok(PathBuf::from("music").join(folder).join(file))
        }
        _ => Err(PathSchemaError::KindMismatch),
    }
}

/// Parse a library-relative path to a TitleId.
///
/// Year in the path is display. Recovered identity is the `{tmdb|tvdb|mbid-…}`
/// token in the title folder. Album remasters with different folder years and
/// the same MBID yield the same TitleId.
///
/// Paths under `_ops/needs-split` or `_ops/needs-year` are classified as those
/// reject bins, not a TitleId.
pub fn parse(path: impl AsRef<Path>) -> Result<TitleId, PathSchemaError> {
    let path = path.as_ref();
    let raw = path_utf8(path)?;
    if let Some(bin) = classify_reject_bin(path) {
        return Err(PathSchemaError::RejectBin(bin.as_str()));
    }
    if raw.chars().any(char::is_whitespace) {
        return Err(PathSchemaError::SpaceRefused(raw.to_string()));
    }
    if path.components().any(|c| match c {
        Component::Normal(name) => name
            .to_str()
            .is_some_and(|s| s.split(['.', '-', '_']).any(is_scene_tag)),
        _ => false,
    }) {
        return Err(PathSchemaError::LeftoverSceneTag(raw.to_string()));
    }
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }

    let mut comps = path.components();
    let kind_dir = component_str(comps.next(), raw)?;
    let folder = component_str(comps.next(), raw)?;
    let file = match comps.next() {
        Some(Component::Normal(name)) => Some(os_str(name, raw)?),
        Some(_) => return Err(PathSchemaError::Invalid(raw.to_string())),
        None => None,
    };
    if comps.next().is_some() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }

    let (display_title, folder_year, source, id) = parse_title_folder(folder, raw)?;
    let kind = match kind_dir {
        "movies" => TitleKind::Movie,
        "series" => TitleKind::Series,
        "music" => TitleKind::Album,
        _ => return Err(PathSchemaError::Invalid(raw.to_string())),
    };
    let title_id = TitleId::parse(&format!("{}:{}:{id}", kind.as_str(), source.as_str()))
        .map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;

    if let Some(file) = file {
        match kind {
            TitleKind::Movie => {
                let stem = file_stem(file, raw)?;
                let file_year = year_from_movie_stem(stem, &display_title, raw)?;
                if file_year != folder_year {
                    return Err(PathSchemaError::YearMismatch {
                        folder: folder_year,
                        file: file_year,
                    });
                }
            }
            TitleKind::Series => {
                let stem = file_stem(file, raw)?;
                let file_year = year_from_episode_stem(stem, &display_title, raw)?;
                if file_year != folder_year {
                    return Err(PathSchemaError::YearMismatch {
                        folder: folder_year,
                        file: file_year,
                    });
                }
            }
            TitleKind::Album => {
                let stem = file_stem(file, raw)?;
                let file_year = year_from_track_stem(stem, raw)?;
                if file_year != folder_year {
                    return Err(PathSchemaError::YearMismatch {
                        folder: folder_year,
                        file: file_year,
                    });
                }
            }
        }
    }

    Ok(title_id)
}

/// Staging layout: `_incoming/<kind:source:id>/<final_name>`.
pub fn staging_path(title_id: &TitleId, final_name: &str) -> Result<PathBuf, PathSchemaError> {
    if title_id.id().is_empty() {
        return Err(PathSchemaError::InvalidTitleId);
    }
    if final_name.is_empty() {
        return Err(PathSchemaError::EmptyFinalName);
    }
    if final_name.chars().any(char::is_whitespace) {
        return Err(PathSchemaError::SpaceRefused(final_name.to_string()));
    }
    reject_reserved_component(final_name)?;
    if final_name.contains('/') || final_name.contains('\\') {
        return Err(PathSchemaError::Invalid(final_name.to_string()));
    }
    Ok(PathBuf::from("_incoming")
        .join(title_id.render())
        .join(final_name))
}

fn title_folder(title: &str, year: u16, title_id: &TitleId) -> String {
    format!(
        "{title}.({year}).{{{}-{}}}",
        title_id.source().path_token(),
        title_id.id()
    )
}

fn validate_display_token(token: &str) -> Result<String, PathSchemaError> {
    if token.is_empty() {
        return Err(PathSchemaError::Invalid(token.to_string()));
    }
    if token.chars().any(char::is_whitespace) {
        return Err(PathSchemaError::SpaceRefused(token.to_string()));
    }
    reject_reserved_component(token)?;
    if token.split(['.', '-', '_']).any(is_scene_tag) {
        return Err(PathSchemaError::LeftoverSceneTag(token.to_string()));
    }
    if token.contains('/') || token.contains('\\') || token.contains('{') || token.contains('}') {
        return Err(PathSchemaError::Invalid(token.to_string()));
    }
    Ok(token.to_string())
}

fn reject_reserved_component(name: &str) -> Result<(), PathSchemaError> {
    if name.contains('\0') || name == "." || name == ".." {
        return Err(PathSchemaError::Invalid(name.to_string()));
    }
    Ok(())
}

fn validate_year(year: u16) -> Result<u16, PathSchemaError> {
    if (1000..=9999).contains(&year) {
        Ok(year)
    } else {
        Err(PathSchemaError::InvalidYear(year))
    }
}

fn parse_year_4(digits: &str, raw: &str) -> Result<u16, PathSchemaError> {
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let year: u16 = digits
        .parse()
        .map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    validate_year(year).map_err(|_| PathSchemaError::Invalid(raw.to_string()))
}

fn validate_extension(extension: &str) -> Result<String, PathSchemaError> {
    let extension = extension.strip_prefix('.').unwrap_or(extension);
    if extension.is_empty()
        || extension.chars().any(|c| !c.is_ascii_alphanumeric())
        || extension.contains('.')
    {
        return Err(PathSchemaError::Invalid(extension.to_string()));
    }
    Ok(extension.to_string())
}

fn is_scene_tag(part: &str) -> bool {
    SCENE_TAGS.iter().any(|tag| part.eq_ignore_ascii_case(tag))
}

fn classify_reject_bin(path: &Path) -> Option<RejectBin> {
    let names: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    if names
        .windows(2)
        .any(|w| w[0] == "_ops" && w[1] == "needs-split")
    {
        return Some(RejectBin::NeedsSplit);
    }
    if names
        .windows(2)
        .any(|w| w[0] == "_ops" && w[1] == "needs-year")
    {
        return Some(RejectBin::NeedsYear);
    }
    None
}

fn parse_title_folder<'a>(
    folder: &'a str,
    raw: &str,
) -> Result<(String, u16, TitleSource, &'a str), PathSchemaError> {
    let Some(brace) = folder.rfind(".{") else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    if !folder.ends_with('}') {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let token = &folder[brace + 2..folder.len() - 1];
    let prefix = &folder[..brace];
    if prefix.len() < 7 {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let year_suffix = &prefix[prefix.len() - 7..];
    if !year_suffix.starts_with(".(") || !year_suffix.ends_with(')') {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let year = parse_year_4(&year_suffix[2..6], raw)?;
    let title = &prefix[..prefix.len() - 7];
    if title.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let Some((source_raw, id)) = token.split_once('-') else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    let source = match source_raw {
        "tmdb" => TitleSource::Tmdb,
        "tvdb" => TitleSource::Tvdb,
        "mbid" => TitleSource::Mbid,
        _ => return Err(PathSchemaError::Invalid(raw.to_string())),
    };
    if id.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    Ok((title.to_string(), year, source, id))
}

fn year_from_movie_stem(stem: &str, title: &str, raw: &str) -> Result<u16, PathSchemaError> {
    let expected_prefix = format!("{title}.(");
    let rest = stem
        .strip_prefix(&expected_prefix)
        .ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
    let Some((year_str, after)) = rest.split_once(')') else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    if !after.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    parse_year_4(year_str, raw)
}

fn year_from_episode_stem(stem: &str, title: &str, raw: &str) -> Result<u16, PathSchemaError> {
    let expected_prefix = format!("{title}.(");
    let rest = stem
        .strip_prefix(&expected_prefix)
        .ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
    let Some((year_str, after)) = rest.split_once(')') else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    if !is_sxxexx(after) {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    parse_year_4(year_str, raw)
}

fn year_from_track_stem(stem: &str, raw: &str) -> Result<u16, PathSchemaError> {
    if stem.len() < 8 {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let year_suffix = &stem[stem.len() - 7..];
    if !year_suffix.starts_with(".(") || !year_suffix.ends_with(')') {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    parse_year_4(&year_suffix[2..6], raw)
}

fn is_sxxexx(after: &str) -> bool {
    let b = after.as_bytes();
    b.len() == 7
        && b[0] == b'.'
        && b[1] == b'S'
        && b[4] == b'E'
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
}

fn file_stem<'a>(file: &'a str, raw: &str) -> Result<&'a str, PathSchemaError> {
    let Some((stem, _ext)) = file.rsplit_once('.') else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    if stem.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    Ok(stem)
}

fn path_utf8(path: &Path) -> Result<&str, PathSchemaError> {
    path.to_str()
        .ok_or_else(|| PathSchemaError::Invalid(path.display().to_string()))
}

fn component_str<'a>(
    component: Option<Component<'a>>,
    raw: &str,
) -> Result<&'a str, PathSchemaError> {
    match component {
        Some(Component::Normal(name)) => os_str(name, raw),
        _ => Err(PathSchemaError::Invalid(raw.to_string())),
    }
}

fn os_str<'a>(name: &'a std::ffi::OsStr, raw: &str) -> Result<&'a str, PathSchemaError> {
    name.to_str()
        .ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAYER_MBID: &str = "0f82b02e-c6cd-4242-b195-93d4bf3e0d63";

    fn movie_id() -> TitleId {
        TitleId::movie("603").expect("movie")
    }

    fn series_id() -> TitleId {
        TitleId::series("79126").expect("series")
    }

    fn album_id() -> TitleId {
        TitleId::album(RELAYER_MBID).expect("album")
    }

    #[test]
    fn grammar_is_versioned() {
        assert_eq!(GRAMMAR_VERSION, 1);
    }

    #[test]
    fn pathschema_parse_render_identity_movie_series_album() {
        let movie = render(&movie_id(), &Placement::movie("The.Matrix", 1999, "mkv"))
            .expect("render movie");
        assert_eq!(
            movie.to_str().expect("utf8"),
            "movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv"
        );
        assert_eq!(parse(&movie).expect("parse movie"), movie_id());

        let series = render(
            &series_id(),
            &Placement::episode("The.Wire", 2002, 1, 1, "mkv"),
        )
        .expect("render series");
        assert_eq!(
            series.to_str().expect("utf8"),
            "series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.mkv"
        );
        assert_eq!(parse(&series).expect("parse series"), series_id());

        let album = render(
            &album_id(),
            &Placement::track("Relayer", 2013, 1, "The.Gates.Of.Delirium", "flac"),
        )
        .expect("render album");
        assert_eq!(
            album.to_str().expect("utf8"),
            "music/Relayer.(2013).{mbid-0f82b02e-c6cd-4242-b195-93d4bf3e0d63}/01.The.Gates.Of.Delirium.(2013).flac"
        );
        assert_eq!(parse(&album).expect("parse album"), album_id());
    }

    #[test]
    fn library_year_matches_folder_and_file_stem() {
        let path =
            render(&movie_id(), &Placement::movie("The.Matrix", 1999, "mkv")).expect("render");
        let text = path.to_str().expect("utf8");
        let folder_year = text
            .split('/')
            .nth(1)
            .expect("folder")
            .matches("(1999)")
            .count();
        let file_year = text
            .split('/')
            .nth(2)
            .expect("file")
            .matches("(1999)")
            .count();
        assert_eq!(folder_year, 1);
        assert_eq!(file_year, 1);
        assert!(!text.contains(' '));

        let mismatch = PathBuf::from("movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1998).mkv");
        assert!(matches!(
            parse(&mismatch),
            Err(PathSchemaError::YearMismatch {
                folder: 1999,
                file: 1998
            })
        ));

        let album = render(
            &album_id(),
            &Placement::track("Relayer", 2013, 1, "The.Gates.Of.Delirium", "flac"),
        )
        .expect("render album");
        let album_text = album.to_str().expect("utf8");
        assert!(album_text.contains("/Relayer.(2013).{"));
        assert!(album_text.ends_with("01.The.Gates.Of.Delirium.(2013).flac"));
        let album_mismatch = PathBuf::from(format!(
            "music/Relayer.(2013).{{mbid-{RELAYER_MBID}}}/01.The.Gates.Of.Delirium.(1974).flac"
        ));
        assert!(matches!(
            parse(&album_mismatch),
            Err(PathSchemaError::YearMismatch {
                folder: 2013,
                file: 1974
            })
        ));
    }

    #[test]
    fn spaces_in_title_are_refused() {
        let err =
            render(&movie_id(), &Placement::movie("The Matrix", 1999, "mkv")).expect_err("space");
        assert!(matches!(err, PathSchemaError::SpaceRefused(_)));
        assert!(matches!(
            parse("movies/The Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv"),
            Err(PathSchemaError::SpaceRefused(_))
        ));
    }

    #[test]
    fn remaster_folders_share_album_title_id() {
        let id = album_id();
        let y1974 = PathBuf::from(format!(
            "music/Relayer.(1974).{{mbid-{RELAYER_MBID}}}/01.The.Gates.Of.Delirium.(1974).flac"
        ));
        let y2013 = PathBuf::from(format!(
            "music/Relayer.(2013).{{mbid-{RELAYER_MBID}}}/01.The.Gates.Of.Delirium.(2013).flac"
        ));
        assert_eq!(parse(&y1974).expect("1974"), id);
        assert_eq!(parse(&y2013).expect("2013"), id);
        assert_eq!(parse(&y1974).expect("1974"), parse(&y2013).expect("2013"));
    }

    #[test]
    fn scene_tag_strip_removes_repacj_repack_proper() {
        assert_eq!(
            strip_scene_tags("House.of.the.Dragon.S02E07.REPACJ.1080p"),
            "House.of.the.Dragon.S02E07.1080p"
        );
        assert_eq!(strip_scene_tags("The.Matrix.REPACK.mkv"), "The.Matrix.mkv");
        assert_eq!(strip_scene_tags("The.Wire.PROPER.mkv"), "The.Wire.mkv");
        assert_eq!(
            strip_scene_tags("Title.REPACJ.REPACK.PROPER.mkv"),
            "Title.mkv"
        );
    }

    #[test]
    fn leftover_scene_tag_in_library_path_is_not_a_successful_parse() {
        assert!(matches!(
            parse("movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).REPACK.mkv"),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        assert!(matches!(
            parse("movies/The.Matrix.PROPER.(1999).{tmdb-603}/The.Matrix.(1999).mkv"),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        assert!(matches!(
            parse("series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.REPACJ.mkv"),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
    }

    #[test]
    fn reject_bins_needs_split_and_needs_year() {
        let split = parse("_ops/needs-split/season-pack.mkv").expect_err("split");
        assert_eq!(split.reject_bin(), Some(RejectBin::NeedsSplit));
        assert!(matches!(split, PathSchemaError::RejectBin("needs-split")));

        let year = parse("_ops/needs-year/unknown.mkv").expect_err("year");
        assert_eq!(year.reject_bin(), Some(RejectBin::NeedsYear));
        assert!(matches!(year, PathSchemaError::RejectBin("needs-year")));

        assert_eq!(
            RejectBin::NeedsSplit.rel_dir(),
            PathBuf::from("_ops/needs-split")
        );
        assert_eq!(
            RejectBin::NeedsYear.rel_dir(),
            PathBuf::from("_ops/needs-year")
        );

        let not_ops = parse("needs-split/season-pack.mkv").expect_err("not ops");
        assert!(not_ops.reject_bin().is_none());
        assert!(matches!(not_ops, PathSchemaError::Invalid(_)));
        let nested = parse("movies/needs-split/x.mkv").expect_err("component");
        assert!(nested.reject_bin().is_none());
    }

    #[test]
    fn render_rejects_years_outside_four_digit_range() {
        assert!(matches!(
            render(&movie_id(), &Placement::movie("The.Matrix", 999, "mkv")),
            Err(PathSchemaError::InvalidYear(999))
        ));
        assert!(matches!(
            render(&movie_id(), &Placement::movie("The.Matrix", 10000, "mkv")),
            Err(PathSchemaError::InvalidYear(10000))
        ));
        assert!(render(&movie_id(), &Placement::movie("The.Matrix", 1000, "mkv")).is_ok());
        assert!(render(&movie_id(), &Placement::movie("The.Matrix", 9999, "mkv")).is_ok());
    }

    #[test]
    fn reserved_dot_dotdot_and_nul_are_rejected() {
        let id = movie_id();
        assert!(matches!(
            staging_path(&id, "."),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            staging_path(&id, ".."),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            staging_path(&id, "file\0name.mkv"),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            render(&id, &Placement::movie(".", 1999, "mkv")),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            render(&id, &Placement::movie("..", 1999, "mkv")),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            render(&id, &Placement::movie("The.Matrix\0x", 1999, "mkv")),
            Err(PathSchemaError::Invalid(_))
        ));
    }

    #[test]
    fn movie_and_series_stems_are_strict() {
        assert!(matches!(
            parse("movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).S01E01.mkv"),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            parse("series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S1E01.mkv"),
            Err(PathSchemaError::Invalid(_))
        ));
        assert!(matches!(
            parse("series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.Director.mkv"),
            Err(PathSchemaError::Invalid(_))
        ));
    }

    #[test]
    fn staging_path_uses_serialized_title_id() {
        let id = movie_id();
        let staged = staging_path(&id, "The.Matrix.(1999).mkv").expect("staging");
        assert_eq!(
            staged.to_str().expect("utf8"),
            "_incoming/movie:tmdb:603/The.Matrix.(1999).mkv"
        );
        assert!(matches!(
            staging_path(&id, ""),
            Err(PathSchemaError::EmptyFinalName)
        ));
        assert!(TitleId::parse("").is_err());
        assert!(TitleId::parse("movie:tmdb:").is_err());
    }
}
