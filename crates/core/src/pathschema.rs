//! Versioned library-path grammar. The only renderer/parser of library paths.
//!
//! Grammar v2 is the operator's dotted Jellyfin layout (`AGENTS.md`), which is
//! also exactly what Radarr/Sonarr/Lidarr are configured to write on the box:
//!
//! - `movies/The.Matrix.(1999)/The.Matrix.(1999).mkv`
//! - `series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.The.Target.mkv`
//! - `music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac`
//! - `music/Radiohead/OK.Computer.(1997)/Disc.01/OK.Computer.(1997).01.Airbag.flac`
//!
//! No identity token lives in a path. Identity recovered from a path is a
//! [`crate::title_id::TitleSource::Key`] TitleId (normalised title + year;
//! artist + album for music). Rendering is strict (dots, no spaces); parsing is lenient about
//! spaces and `Title - Subtitle (Year)` so that what *arr actually writes on the
//! seedbox still classifies.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::title_id::{TitleId, TitleKind, strip_trailing_year};

/// PathSchema grammar version. Bump when render/parse rules change.
pub const GRAMMAR_VERSION: u32 = 2;

const SCENE_TAGS: &[&str] = &["REPACJ", "REPACK", "PROPER"];

/// Tokens after which an episode/track title is release noise, not a title.
/// A Sonarr `{Episode.CleanTitle}` never contains these; a scene name does.
const STOP_TOKENS: &[&str] = &[
    "web",
    "internal",
    "readnfo",
    "hdr",
    "hdr10",
    "hdr10+",
    "hdr10plus",
    "dv",
    "dovi",
    "h",
    "xvid",
    "8bit",
    "opus",
    "flac",
    "amzn",
    "dsnp",
    "atvp",
    "hmax",
    "nf",
    "hulu",
    "pcok",
    "uhd",
    "multi",
    "hybrid",
    "imax",
    "repack",
    "repack2",
    "proper",
    "complete",
    "season",
    "pack",
];

/// Display placement used only to render a path. Identity stays on [`TitleId`].
///
/// Added fields default so plans written by grammar v1 still load.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
        episode: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        episode_end: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        episode_title: Option<String>,
        extension: String,
    },
    Track {
        #[serde(default)]
        artist: String,
        album: String,
        year: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disc: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<u8>,
        title: String,
        extension: String,
    },
}

/// Which file of a title this is: the dedupe / already-present unit.
///
/// A movie is one file. An episode is `(season, episode)` inside its show.
/// A track is `(disc, track)` inside its album; the album year is display, so
/// a remaster and the original share tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileKey {
    Whole,
    Episode { season: u8, episode: u16 },
    Track { disc: u8, track: u8 },
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
        episode: u16,
        extension: impl Into<String>,
    ) -> Self {
        Self::Episode {
            title: title.into(),
            year,
            season,
            episode,
            episode_end: None,
            episode_title: None,
            extension: extension.into(),
        }
    }

    pub fn episode_titled(
        title: impl Into<String>,
        year: u16,
        season: u8,
        episode: u16,
        episode_end: Option<u16>,
        episode_title: Option<String>,
        extension: impl Into<String>,
    ) -> Self {
        Self::Episode {
            title: title.into(),
            year,
            season,
            episode,
            episode_end,
            episode_title: episode_title.filter(|t| !t.is_empty()),
            extension: extension.into(),
        }
    }

    pub fn track(
        artist: impl Into<String>,
        album: impl Into<String>,
        year: u16,
        disc: Option<u8>,
        track: Option<u8>,
        title: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        Self::Track {
            artist: artist.into(),
            album: album.into(),
            year,
            disc,
            track,
            title: title.into(),
            extension: extension.into(),
        }
    }

    pub fn kind(&self) -> TitleKind {
        match self {
            Self::Movie { .. } => TitleKind::Movie,
            Self::Episode { .. } => TitleKind::Series,
            Self::Track { .. } => TitleKind::Album,
        }
    }

    pub fn extension(&self) -> &str {
        match self {
            Self::Movie { extension, .. }
            | Self::Episode { extension, .. }
            | Self::Track { extension, .. } => extension,
        }
    }

    /// The per-file identity unit. Tracks without a number fall back to `Whole`
    /// (one such file per album is all the grammar can name).
    pub fn file_key(&self) -> FileKey {
        match self {
            Self::Movie { .. } => FileKey::Whole,
            Self::Episode {
                season, episode, ..
            } => FileKey::Episode {
                season: *season,
                episode: *episode,
            },
            Self::Track {
                disc,
                track: Some(track),
                ..
            } => FileKey::Track {
                disc: disc.unwrap_or(1),
                track: *track,
            },
            Self::Track { track: None, .. } => FileKey::Whole,
        }
    }

    /// The `key` TitleId this placement names: the identity the planner and the
    /// on-disk scan compare.
    pub fn key_title_id(&self) -> Result<TitleId, PathSchemaError> {
        match self {
            Self::Movie { title, year, .. } => TitleId::movie_key(title, *year),
            Self::Episode { title, year, .. } => TitleId::series_key(title, *year),
            Self::Track { artist, album, .. } => TitleId::album_key(artist, album),
        }
        .map_err(|_| PathSchemaError::Invalid(format!("{self:?}")))
    }

    /// Human label for `why`/`status`: `The.Matrix.(1999)`, `The.Wire.(2002) S01E01`.
    pub fn label(&self) -> String {
        match self {
            Self::Movie { title, year, .. } => format!("{title}.({year})"),
            Self::Episode {
                title,
                year,
                season,
                episode,
                episode_end,
                ..
            } => match episode_end {
                Some(end) => format!("{title}.({year}) S{season:02}E{episode:02}-E{end:02}"),
                None => format!("{title}.({year}) S{season:02}E{episode:02}"),
            },
            Self::Track {
                artist,
                album,
                year,
                track,
                ..
            } => match track {
                Some(n) => format!("{artist}/{album}.({year}) {n:02}"),
                None => format!("{artist}/{album}.({year})"),
            },
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
    #[error("reject bin {}", .0.as_str())]
    RejectBin(RejectBin),
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
    #[error("season {season} / episode {episode} is outside range")]
    SeasonEpisodeOutOfRange { season: u8, episode: u16 },
    #[error("track {0} is outside 1..=99")]
    TrackOutOfRange(u8),
    #[error("empty final name")]
    EmptyFinalName,
}

impl PathSchemaError {
    pub fn reject_bin(&self) -> Option<RejectBin> {
        match self {
            Self::RejectBin(bin) => Some(*bin),
            _ => None,
        }
    }
}

/// Dotted display form: whitespace, `:`, `/`, `\`, `[]{}`, `,`, `_` and
/// ` - ` become `.`; runs of dots collapse; leading/trailing dots go.
/// Parentheses survive so `(1999)` stays a year. Intra-word hyphens and
/// apostrophes survive (`Spider-Man`, `It's.Always.Sunny`).
pub fn dotted(text: &str) -> String {
    let mut s = text.replace('\u{a0}', " ");
    s = s.replace(" - ", " ");
    let mapped: String = s
        .chars()
        .map(|c| match c {
            ':' | '/' | '\\' | '[' | ']' | '{' | '}' | ',' | '_' | '—' | '–' | '\u{2019}' => {
                '.'
            }
            c if c.is_whitespace() => '.',
            c => c,
        })
        .collect();
    let mut out = String::with_capacity(mapped.len());
    let mut last_dot = true;
    for c in mapped.chars() {
        if c == '.' {
            if !last_dot {
                out.push('.');
            }
            last_dot = true;
        } else {
            out.push(c);
            last_dot = false;
        }
    }
    while out.ends_with('.') {
        out.pop();
    }
    out
}

/// Strip scene tags `REPACJ`, `REPACK`, and `PROPER` from a name.
///
/// Only the tag tokens are removed. The `.`, `-`, and `_` separators between the
/// surviving tokens are preserved as written, so `Spider-Man.REPACK` becomes
/// `Spider-Man`, not `Spider.Man`.
pub fn strip_scene_tags(name: &str) -> String {
    let mut tokens: Vec<&str> = Vec::new();
    let mut seps: Vec<char> = Vec::new();
    let mut start = 0usize;
    for (i, c) in name.char_indices() {
        if matches!(c, '.' | '-' | '_') {
            tokens.push(&name[start..i]);
            seps.push(c);
            start = i + c.len_utf8();
        }
    }
    tokens.push(&name[start..]);

    let mut out = String::with_capacity(name.len());
    let mut first = true;
    for (idx, token) in tokens.iter().enumerate() {
        if is_scene_tag(token) {
            continue;
        }
        if first {
            first = false;
        } else {
            out.push(seps[idx - 1]);
        }
        out.push_str(token);
    }
    out
}

/// Strip scene tags from placement display tokens. Year / S/E / track stay put.
pub fn strip_placement(placement: &Placement) -> Placement {
    match placement {
        Placement::Movie {
            title,
            year,
            extension,
        } => Placement::movie(strip_scene_tags(title), *year, extension),
        Placement::Episode {
            title,
            year,
            season,
            episode,
            episode_end,
            episode_title,
            extension,
        } => Placement::episode_titled(
            strip_scene_tags(title),
            *year,
            *season,
            *episode,
            *episode_end,
            episode_title.as_deref().map(strip_scene_tags),
            extension,
        ),
        Placement::Track {
            artist,
            album,
            year,
            disc,
            track,
            title,
            extension,
        } => Placement::track(
            strip_scene_tags(artist),
            strip_scene_tags(album),
            *year,
            *disc,
            *track,
            strip_scene_tags(title),
            extension,
        ),
    }
}

/// Normalise every display token of a placement to dotted form and strip
/// scene tags: the one door from *arr JSON (spaces, colons) to a renderable
/// placement.
pub fn normalize_placement(placement: &Placement) -> Placement {
    let dotted_p = match placement {
        Placement::Movie {
            title,
            year,
            extension,
        } => Placement::movie(dotted(title), *year, extension.trim_start_matches('.')),
        Placement::Episode {
            title,
            year,
            season,
            episode,
            episode_end,
            episode_title,
            extension,
        } => Placement::episode_titled(
            dotted(title),
            *year,
            *season,
            *episode,
            *episode_end,
            episode_title.as_deref().map(dotted),
            extension.trim_start_matches('.'),
        ),
        Placement::Track {
            artist,
            album,
            year,
            disc,
            track,
            title,
            extension,
        } => Placement::track(
            dotted(artist),
            dotted(strip_trailing_year(album)),
            *year,
            *disc,
            *track,
            dotted(title),
            extension.trim_start_matches('.'),
        ),
    };
    strip_placement(&dotted_p)
}

/// Render a library-relative path from TitleId plus placement.
///
/// The TitleId only has to agree on kind; the path never carries its id.
pub fn render(title_id: &TitleId, placement: &Placement) -> Result<PathBuf, PathSchemaError> {
    if title_id.kind() != placement.kind() {
        return Err(PathSchemaError::KindMismatch);
    }
    render_placement(placement)
}

/// Render a library-relative path from a placement alone.
pub fn render_placement(placement: &Placement) -> Result<PathBuf, PathSchemaError> {
    match placement {
        Placement::Movie {
            title,
            year,
            extension,
        } => {
            let title = validate_display_token(title)?;
            let year = validate_year(*year)?;
            let extension = validate_extension(extension)?;
            let folder = format!("{title}.({year})");
            let file = format!("{folder}.{extension}");
            Ok(PathBuf::from("movies").join(folder).join(file))
        }
        Placement::Episode {
            title,
            year,
            season,
            episode,
            episode_end,
            episode_title,
            extension,
        } => {
            let title = validate_display_token(title)?;
            let year = validate_year(*year)?;
            let extension = validate_extension(extension)?;
            if *season > 99 || *episode > 999 || episode_end.is_some_and(|e| e > 999) {
                return Err(PathSchemaError::SeasonEpisodeOutOfRange {
                    season: *season,
                    episode: *episode,
                });
            }
            let folder = format!("{title}.({year})");
            let mut stem = format!("{folder}.S{season:02}E{episode:02}");
            if let Some(end) = episode_end {
                stem.push_str(&format!("-E{end:02}"));
            }
            if let Some(ep_title) = episode_title.as_deref().filter(|t| !t.is_empty()) {
                let ep_title = validate_display_token(ep_title)?;
                stem.push('.');
                stem.push_str(&ep_title);
            }
            Ok(PathBuf::from("series")
                .join(folder)
                .join(format!("Season.{season:02}"))
                .join(format!("{stem}.{extension}")))
        }
        Placement::Track {
            artist,
            album,
            year,
            disc,
            track,
            title,
            extension,
        } => {
            let artist = validate_display_token(artist)?;
            let album = validate_display_token(album)?;
            let title = validate_display_token(title)?;
            let year = validate_year(*year)?;
            let extension = validate_extension(extension)?;
            if let Some(n) = track
                && !(1..=99).contains(n)
            {
                return Err(PathSchemaError::TrackOutOfRange(*n));
            }
            if let Some(d) = disc
                && !(1..=99).contains(d)
            {
                return Err(PathSchemaError::TrackOutOfRange(*d));
            }
            let album_folder = format!("{album}.({year})");
            let mut dir = PathBuf::from("music").join(artist).join(&album_folder);
            if let Some(d) = disc {
                dir = dir.join(format!("Disc.{d:02}"));
            }
            let file = match track {
                Some(n) => format!("{album_folder}.{n:02}.{title}.{extension}"),
                None => format!("{album_folder}.{title}.{extension}"),
            };
            Ok(dir.join(file))
        }
    }
}

/// Parse a library-relative path to a `key` TitleId.
///
/// Paths under `_ops/needs-split` or `_ops/needs-year` are classified as those
/// reject bins, not a TitleId.
pub fn parse(path: impl AsRef<Path>) -> Result<TitleId, PathSchemaError> {
    parse_inner(path.as_ref(), None).map(|(title_id, _)| title_id)
}

/// Parse a library-relative **file** path to TitleId plus placement.
pub fn parse_placement(path: impl AsRef<Path>) -> Result<(TitleId, Placement), PathSchemaError> {
    parse_inner(path.as_ref(), None)
}

/// Parse a **root-relative** remote path (no `movies/` prefix) with the kind
/// the allowlisted root is declared to hold. `None` infers the kind from
/// shape: `Season.NN` or `SxxEyy` is an episode, artist/album/file is a
/// track, folder/file is a movie.
pub fn parse_remote(
    kind: Option<TitleKind>,
    rel_path: impl AsRef<Path>,
) -> Result<(TitleId, Placement), PathSchemaError> {
    parse_inner(rel_path.as_ref(), Some(kind))
}

/// Root-relative library path: `movies/`, `series/`, `music/` map to a kind.
/// Seedbox roots are commonly named `tv`; accept it as `series`.
pub fn kind_dir(name: &str) -> Option<TitleKind> {
    match name {
        "movies" => Some(TitleKind::Movie),
        "series" | "tv" => Some(TitleKind::Series),
        "music" => Some(TitleKind::Album),
        _ => None,
    }
}

/// `hint == None`: library-relative, first component must be a kind dir.
/// `hint == Some(kind)`: root-relative with a known or inferred kind.
fn parse_inner(
    path: &Path,
    hint: Option<Option<TitleKind>>,
) -> Result<(TitleId, Placement), PathSchemaError> {
    let raw = path_utf8(path)?;
    if let Some(bin) = classify_reject_bin(path) {
        return Err(PathSchemaError::RejectBin(bin));
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
    let mut comps: Vec<&str> = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(name) => comps.push(os_str(name, raw)?),
            Component::CurDir => {}
            _ => return Err(PathSchemaError::Invalid(raw.to_string())),
        }
    }
    if comps.first().is_some_and(|c| c.starts_with('_')) {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }

    let kind = match hint {
        None => {
            let first = comps
                .first()
                .ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
            let kind = kind_dir(first).ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
            comps.remove(0);
            kind
        }
        Some(Some(kind)) => {
            if comps.len() > 2
                && let Some(first_kind) = kind_dir(comps[0])
                && first_kind == kind
            {
                comps.remove(0);
            }
            kind
        }
        Some(None) => {
            // A root that is the parent of the kind dirs (one `~/media` root)
            // yields `movies/...` paths: the dir names the kind.
            if comps.len() > 2
                && let Some(kind) = kind_dir(comps[0])
            {
                comps.remove(0);
                kind
            } else {
                infer_kind(&comps).ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?
            }
        }
    };
    if comps
        .iter()
        .any(|c| c.split(['.', '-', '_', ' ']).any(is_scene_tag))
    {
        return Err(PathSchemaError::LeftoverSceneTag(raw.to_string()));
    }

    let placement = match kind {
        TitleKind::Movie => parse_movie(&comps, raw)?,
        TitleKind::Series => parse_episode(&comps, raw)?,
        TitleKind::Album => parse_track(&comps, raw)?,
    };
    let title_id = placement.key_title_id()?;
    Ok((title_id, placement))
}

fn infer_kind(comps: &[&str]) -> Option<TitleKind> {
    let file = comps.last()?;
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    if episode_tag(stem).is_some() || comps.iter().any(|c| season_dir(c).is_some()) {
        return Some(TitleKind::Series);
    }
    match comps.len() {
        2 => Some(TitleKind::Movie),
        3 | 4 => Some(TitleKind::Album),
        _ => None,
    }
}

fn parse_movie(comps: &[&str], raw: &str) -> Result<Placement, PathSchemaError> {
    let [folder, file] = comps else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    let folder = dotted(folder);
    let (title, year) = split_title_year(&folder, raw)?;
    let (stem, extension) = split_stem_ext(file, raw)?;
    let stem = dotted(stem);
    let prefix = format!("{title}.({year})");
    if !stem.eq_ignore_ascii_case(&prefix) && !starts_with_ci(&stem, &format!("{prefix}.")) {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    validate_display_token(&title).map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    Ok(Placement::movie(title, year, extension))
}

fn parse_episode(comps: &[&str], raw: &str) -> Result<Placement, PathSchemaError> {
    let (folder, file) = match comps {
        [folder, file] => (*folder, *file),
        [folder, season_folder, file] if season_dir(season_folder).is_some() => (*folder, *file),
        _ => return Err(PathSchemaError::Invalid(raw.to_string())),
    };
    let folder = dotted(folder);
    let (title, year) = split_title_year(&folder, raw)?;
    validate_display_token(&title).map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    let (stem, extension) = split_stem_ext(file, raw)?;
    let stem = dotted(stem);
    let (season, episode, episode_end, after) =
        episode_tag(&stem).ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
    if season > 99 || episode > 999 || episode_end.is_some_and(|e| e > 999) {
        return Err(PathSchemaError::SeasonEpisodeOutOfRange { season, episode });
    }
    if let [_, season_folder, _] = comps
        && let Some(dir_season) = season_dir(season_folder)
        && dir_season != season
    {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let episode_title = cut_at_stop_token(after);
    Ok(Placement::episode_titled(
        title,
        year,
        season,
        episode,
        episode_end,
        episode_title,
        extension,
    ))
}

fn parse_track(comps: &[&str], raw: &str) -> Result<Placement, PathSchemaError> {
    let (artist, album_folder, disc, file) = match comps {
        [artist, album, file] => (*artist, *album, None, *file),
        [artist, album, disc_folder, file] => {
            let disc =
                disc_dir(disc_folder).ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
            (*artist, *album, Some(disc), *file)
        }
        _ => return Err(PathSchemaError::Invalid(raw.to_string())),
    };
    let artist = dotted(artist);
    validate_display_token(&artist).map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    let album_folder = dotted(album_folder);
    let (album, year) = split_title_year(&album_folder, raw)?;
    validate_display_token(&album).map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    let (stem, extension) = split_stem_ext(file, raw)?;
    let stem = dotted(stem);
    let prefix = format!("{album}.({year}).");
    let rest = if starts_with_ci(&stem, &prefix) {
        &stem[prefix.len()..]
    } else {
        stem.as_str()
    };
    let (track, title) = match rest.split_once('.') {
        Some((n, title))
            if n.len() <= 3 && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let n: u8 = n
                .parse()
                .map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
            (Some(n), title.to_string())
        }
        _ => (None, rest.to_string()),
    };
    let title = cut_at_stop_token(&title).unwrap_or_else(|| title.clone());
    if title.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    validate_display_token(&title).map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    if let Some(n) = track
        && !(1..=99).contains(&n)
    {
        return Err(PathSchemaError::TrackOutOfRange(n));
    }
    Ok(Placement::track(
        artist, album, year, disc, track, title, extension,
    ))
}

/// `Title.(1999)` → (`Title`, 1999). Year is required.
fn split_title_year(folder: &str, raw: &str) -> Result<(String, u16), PathSchemaError> {
    let title = strip_trailing_year(folder);
    if title.len() == folder.len() {
        return Err(PathSchemaError::RejectBin(RejectBin::NeedsYear));
    }
    let open = folder
        .rfind('(')
        .ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))?;
    let year = parse_year_4(&folder[open + 1..open + 5], raw)?;
    if title.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    Ok((title.to_string(), year))
}

/// `S01E02`, `S01E02-E03`, `S01E02E03` anywhere in a dotted stem.
/// Returns (season, episode, episode_end, remainder-after-tag).
fn episode_tag(stem: &str) -> Option<(u8, u16, Option<u16>, &str)> {
    let bytes = stem.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if (bytes[i] == b'S' || bytes[i] == b's')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let mut j = i + 1;
            let s_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() && j - s_start < 2 {
                j += 1;
            }
            if j > s_start && j < bytes.len() && (bytes[j] == b'E' || bytes[j] == b'e') {
                let e_start = j + 1;
                let mut k = e_start;
                while k < bytes.len() && bytes[k].is_ascii_digit() && k - e_start < 3 {
                    k += 1;
                }
                if k > e_start {
                    let season: u8 = stem[s_start..j].parse().ok()?;
                    let episode: u16 = stem[e_start..k].parse().ok()?;
                    let mut end = None;
                    let mut after = k;
                    // `-E03` or `E03` immediately following.
                    let mut m = k;
                    if m < bytes.len() && bytes[m] == b'-' {
                        m += 1;
                    }
                    if m < bytes.len() && (bytes[m] == b'E' || bytes[m] == b'e') {
                        let e2 = m + 1;
                        let mut n = e2;
                        while n < bytes.len() && bytes[n].is_ascii_digit() && n - e2 < 3 {
                            n += 1;
                        }
                        if n > e2 && (n == bytes.len() || !bytes[n].is_ascii_alphanumeric()) {
                            end = stem[e2..n].parse().ok();
                            after = n;
                        }
                    }
                    if after == bytes.len() || !bytes[after].is_ascii_alphanumeric() {
                        return Some((season, episode, end, &stem[after..]));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn season_dir(name: &str) -> Option<u8> {
    let name = dotted(name);
    let rest = name
        .strip_prefix("Season.")
        .or_else(|| name.strip_prefix("season."))?;
    (rest.len() <= 2 && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

fn disc_dir(name: &str) -> Option<u8> {
    let name = dotted(name);
    let rest = name
        .strip_prefix("Disc.")
        .or_else(|| name.strip_prefix("disc."))
        .or_else(|| name.strip_prefix("CD."))
        .or_else(|| name.strip_prefix("CD"))?;
    (rest.len() <= 2 && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

/// Remainder after an episode tag → episode title.
///
/// A clean `{Episode.CleanTitle}` is kept whole. Only when the remainder
/// carries a definitive release marker (a resolution, source, codec, or audio
/// token) is it a scene name, and then the title is cut at the first release
/// token so `Machines.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb` yields `Machines`.
/// Empty → `None`.
fn cut_at_stop_token(after: &str) -> Option<String> {
    let trimmed = after.trim_matches(['.', '-', ' ']);
    if trimmed.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split('.').filter(|t| !t.is_empty()).collect();
    if !tokens.iter().any(|t| is_release_marker(t)) {
        return Some(tokens.join("."));
    }
    let kept: Vec<&str> = tokens
        .iter()
        .copied()
        .take_while(|t| !is_stop_token(t))
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(kept.join("."))
}

/// Tokens that only a release name contains.
fn is_release_marker(token: &str) -> bool {
    let folded = token.to_ascii_lowercase();
    let folded = folded.trim_matches('-');
    if folded.len() >= 4
        && folded.ends_with('p')
        && folded[..folded.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        return true;
    }
    matches!(
        folded,
        "web-dl"
            | "webdl"
            | "webrip"
            | "bluray"
            | "blu-ray"
            | "bdrip"
            | "brrip"
            | "dvdrip"
            | "hdtv"
            | "remux"
            | "x264"
            | "x265"
            | "h264"
            | "h265"
            | "hevc"
            | "avc"
            | "av1"
            | "10bit"
            | "eac3"
            | "ac3"
            | "aac"
            | "atmos"
            | "truehd"
            | "dts"
            | "dts-hd"
    ) || folded
        .strip_prefix("ddp")
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
}

fn is_stop_token(token: &str) -> bool {
    if is_release_marker(token) {
        return true;
    }
    let folded = token.to_ascii_lowercase();
    let folded = folded.trim_matches('-');
    STOP_TOKENS.contains(&folded)
}

/// Staging layout: `_incoming/<kind-source-id>/<final_name>`.
///
/// The directory token is [`TitleId::staging_token`], not [`TitleId::render`]:
/// a colon cannot appear in a directory name on SMB, exFAT, or NTFS.
pub fn staging_path(title_id: &TitleId, final_name: &str) -> Result<PathBuf, PathSchemaError> {
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
        .join(title_id.staging_token())
        .join(final_name))
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
    Ok(extension.to_ascii_lowercase())
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
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
        .any(|w| (w[0] == "_ops" || w[0] == "_incoming") && w[1] == "needs-split")
    {
        return Some(RejectBin::NeedsSplit);
    }
    if names
        .windows(2)
        .any(|w| (w[0] == "_ops" || w[0] == "_incoming") && w[1] == "needs-year")
    {
        return Some(RejectBin::NeedsYear);
    }
    None
}

fn split_stem_ext<'a>(file: &'a str, raw: &str) -> Result<(&'a str, String), PathSchemaError> {
    let Some((stem, ext)) = file.rsplit_once('.') else {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    };
    if stem.is_empty() {
        return Err(PathSchemaError::Invalid(raw.to_string()));
    }
    let extension =
        validate_extension(ext).map_err(|_| PathSchemaError::Invalid(raw.to_string()))?;
    Ok((stem, extension))
}

fn path_utf8(path: &Path) -> Result<&str, PathSchemaError> {
    path.to_str()
        .ok_or_else(|| PathSchemaError::Invalid(path.display().to_string()))
}

fn os_str<'a>(name: &'a std::ffi::OsStr, raw: &str) -> Result<&'a str, PathSchemaError> {
    name.to_str()
        .ok_or_else(|| PathSchemaError::Invalid(raw.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie_id() -> TitleId {
        TitleId::movie_key("The.Matrix", 1999).expect("movie")
    }

    fn series_id() -> TitleId {
        TitleId::series_key("The.Wire", 2002).expect("series")
    }

    fn album_id() -> TitleId {
        TitleId::album_key("Yes", "Relayer").expect("album")
    }

    fn relayer_track() -> Placement {
        Placement::track(
            "Yes",
            "Relayer",
            1974,
            None,
            Some(1),
            "The.Gates.Of.Delirium",
            "flac",
        )
    }

    #[test]
    fn grammar_v2_golden_paths_are_stable() {
        assert_eq!(GRAMMAR_VERSION, 2);
        let cases = [
            (
                movie_id(),
                Placement::movie("The.Matrix", 1999, "mkv"),
                "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv",
            ),
            (
                series_id(),
                Placement::episode_titled(
                    "The.Wire",
                    2002,
                    1,
                    1,
                    None,
                    Some("The.Target".into()),
                    "mkv",
                ),
                "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.The.Target.mkv",
            ),
            (
                series_id(),
                Placement::episode("The.Wire", 2002, 1, 2, "mkv"),
                "series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E02.mkv",
            ),
            (
                album_id(),
                relayer_track(),
                "music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac",
            ),
            (
                TitleId::album_key("Radiohead", "OK.Computer").expect("album"),
                Placement::track(
                    "Radiohead",
                    "OK.Computer",
                    1997,
                    Some(2),
                    Some(3),
                    "Airbag",
                    "flac",
                ),
                "music/Radiohead/OK.Computer.(1997)/Disc.02/OK.Computer.(1997).03.Airbag.flac",
            ),
        ];
        for (id, placement, expected) in cases {
            let rendered = render(&id, &placement).expect("render");
            assert_eq!(rendered.to_str().expect("utf8"), expected);
            let (back_id, back_placement) = parse_placement(&rendered).expect("parse");
            assert_eq!(back_id, id, "{expected}");
            assert_eq!(back_placement, placement, "{expected}");
            assert_eq!(
                render(&back_id, &back_placement).expect("re-render"),
                rendered
            );
        }
    }

    #[test]
    fn render_ignores_authority_source_but_checks_kind() {
        let tmdb = TitleId::movie("603").expect("tmdb");
        let placement = Placement::movie("The.Matrix", 1999, "mkv");
        let path = render(&tmdb, &placement).expect("render");
        assert_eq!(
            path.to_str().expect("utf8"),
            "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv"
        );
        // Parsing recovers the key identity, never the tmdb one.
        assert_eq!(parse(&path).expect("parse"), movie_id());
        assert!(matches!(
            render(&tmdb, &Placement::episode("The.Wire", 2002, 1, 1, "mkv")),
            Err(PathSchemaError::KindMismatch)
        ));
    }

    #[test]
    fn parse_is_lenient_about_spaces_and_arr_folder_styles() {
        // Radarr folder created before dotted naming: spaces and ` - `.
        let (id, placement) = parse_remote(
            Some(TitleKind::Movie),
            "Spider-Man - Brand New Day (2026)/Spider-Man.Brand.New.Day.(2026).mkv",
        )
        .expect("spaced folder");
        assert_eq!(id.render(), "movie:key:spidermanbrandnewday.2026");
        assert_eq!(
            placement,
            Placement::movie("Spider-Man.Brand.New.Day", 2026, "mkv")
        );
        assert_eq!(
            render(&id, &placement)
                .expect("render")
                .to_str()
                .expect("utf8"),
            "movies/Spider-Man.Brand.New.Day.(2026)/Spider-Man.Brand.New.Day.(2026).mkv"
        );

        // Lidarr `{Artist Name}` / `{Album Title}` on the box keep spaces.
        let (id, placement) = parse_remote(
            Some(TitleKind::Album),
            "Radiohead/OK Computer.(1997)/Disc.01/OK Computer.(1997).01.Airbag.flac",
        )
        .expect("lidarr spaces");
        assert_eq!(id.render(), "album:key:radiohead.okcomputer");
        assert_eq!(
            placement,
            Placement::track(
                "Radiohead",
                "OK.Computer",
                1997,
                Some(1),
                Some(1),
                "Airbag",
                "flac"
            )
        );
        assert_eq!(placement.file_key(), FileKey::Track { disc: 1, track: 1 });
    }

    #[test]
    fn remote_kind_inference_and_tv_root_alias() {
        let (id, p) = parse_remote(
            None,
            "Silo.(2023)/Season.01/Silo.(2023).S01E01.Freedom.Day.mkv",
        )
        .expect("episode");
        assert_eq!(id.render(), "series:key:silo.2023");
        assert_eq!(
            p,
            Placement::episode_titled("Silo", 2023, 1, 1, None, Some("Freedom.Day".into()), "mkv")
        );
        let (id, _) = parse_remote(None, "Coco.(2017)/Coco.(2017).mkv").expect("movie");
        assert_eq!(id.render(), "movie:key:coco.2017");
        let (id, _) = parse_remote(
            None,
            "Tool/Lateralus.(2001)/Lateralus.(2001).01.The.Grudge.flac",
        )
        .expect("track");
        assert_eq!(id.render(), "album:key:tool.lateralus");
        // Library-relative with the `tv` alias a seedbox root uses.
        let (id, _) =
            parse_placement("tv/Silo.(2023)/Season.01/Silo.(2023).S01E01.mkv").expect("tv alias");
        assert_eq!(id.kind(), TitleKind::Series);
        // Root-relative with a kind hint that also carries the kind dir.
        let (id, _) = parse_remote(Some(TitleKind::Movie), "movies/Coco.(2017)/Coco.(2017).mkv")
            .expect("hint + dir");
        assert_eq!(id.render(), "movie:key:coco.2017");
    }

    #[test]
    fn episode_tags_double_and_scene_noise_are_cut() {
        let (_, p) = parse_placement(
            "series/The.Simpsons.(1989)/Season.28/The.Simpsons.(1989).S28E12-E13.The.Great.Phatsby.mkv",
        )
        .expect("double");
        assert_eq!(
            p,
            Placement::episode_titled(
                "The.Simpsons",
                1989,
                28,
                12,
                Some(13),
                Some("The.Great.Phatsby".into()),
                "mkv"
            )
        );
        assert_eq!(p.label(), "The.Simpsons.(1989) S28E12-E13");
        let rendered = render_placement(&p).expect("render");
        assert!(
            rendered
                .to_str()
                .expect("utf8")
                .ends_with("S28E12-E13.The.Great.Phatsby.mkv")
        );

        // A scene file inside a proper show folder: noise after the tag is dropped.
        let (_, p) = parse_remote(
            Some(TitleKind::Series),
            "Silo.(2023)/Season.01/Silo.S01E03.Machines.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb.mkv",
        )
        .expect("scene");
        assert_eq!(
            p,
            Placement::episode_titled("Silo", 2023, 1, 3, None, Some("Machines".into()), "mkv")
        );
        let (_, p) = parse_remote(
            Some(TitleKind::Series),
            "Silo.(2023)/Silo.S01E04.1080p.WEB-DL-GROUP.mkv",
        )
        .expect("no season dir, no title");
        assert_eq!(p, Placement::episode("Silo", 2023, 1, 4, "mkv"));
        assert_eq!(
            p.file_key(),
            FileKey::Episode {
                season: 1,
                episode: 4
            }
        );
    }

    #[test]
    fn season_dir_must_agree_with_tag_and_specials_are_season_zero() {
        assert!(
            parse_placement("series/Silo.(2023)/Season.02/Silo.(2023).S01E01.mkv").is_err(),
            "Season.02 folder with an S01 file"
        );
        let (_, p) = parse_placement("series/Silo.(2023)/Season.00/Silo.(2023).S00E01.Recap.mkv")
            .expect("specials");
        assert_eq!(
            p.file_key(),
            FileKey::Episode {
                season: 0,
                episode: 1
            }
        );
    }

    #[test]
    fn movie_stem_must_start_with_folder_name() {
        assert!(parse_placement("movies/The.Matrix.(1999)/The.Matrix.(1998).mkv").is_err());
        assert!(parse_placement("movies/The.Matrix.(1999)/Some.Other.File.mkv").is_err());
        // Extra tokens after the prefix are tolerated (edition tags).
        let (_, p) = parse_placement("movies/The.Matrix.(1999)/The.Matrix.(1999).Remastered.mkv")
            .expect("suffix");
        assert_eq!(p, Placement::movie("The.Matrix", 1999, "mkv"));
    }

    #[test]
    fn missing_year_is_needs_year_and_folder_only_is_invalid() {
        assert!(matches!(
            parse_placement("movies/The.Matrix/The.Matrix.mkv"),
            Err(PathSchemaError::RejectBin(RejectBin::NeedsYear))
        ));
        assert!(parse_placement("movies/The.Matrix.(1999)").is_err());
        assert!(parse_placement("movies").is_err());
        assert!(parse_placement("_incoming/movie-key-x/y.mkv").is_err());
        assert!(matches!(
            parse("_ops/needs-split/Show.S01.1080p"),
            Err(PathSchemaError::RejectBin(RejectBin::NeedsSplit))
        ));
        assert_eq!(
            RejectBin::NeedsSplit.rel_dir(),
            PathBuf::from("_ops/needs-split")
        );
        assert_eq!(
            RejectBin::NeedsYear.rel_dir(),
            PathBuf::from("_ops/needs-year")
        );
    }

    #[test]
    fn spaces_are_refused_on_render_and_normalized_by_dotted() {
        let err =
            render(&movie_id(), &Placement::movie("The Matrix", 1999, "mkv")).expect_err("space");
        assert!(matches!(err, PathSchemaError::SpaceRefused(_)));
        assert_eq!(dotted("The Matrix"), "The.Matrix");
        assert_eq!(
            dotted("Spider-Man - Brand New Day (2026)"),
            "Spider-Man.Brand.New.Day.(2026)"
        );
        assert_eq!(
            dotted("It's Always Sunny: Philly"),
            "It's.Always.Sunny.Philly"
        );
        assert_eq!(dotted("  a   b  "), "a.b");
        assert_eq!(dotted("Blade.Runner.(2049)"), "Blade.Runner.(2049)");
        let normalized =
            normalize_placement(&Placement::movie("Spider-Man: Brand New Day", 2026, ".MKV"));
        assert_eq!(
            normalized,
            Placement::movie("Spider-Man.Brand.New.Day", 2026, "MKV")
        );
        assert_eq!(
            render_placement(&normalized)
                .expect("render")
                .to_str()
                .expect("utf8"),
            "movies/Spider-Man.Brand.New.Day.(2026)/Spider-Man.Brand.New.Day.(2026).mkv"
        );
    }

    #[test]
    fn scene_tags_strip_and_leftovers_refuse() {
        assert_eq!(strip_scene_tags("The.Matrix.REPACK.mkv"), "The.Matrix.mkv");
        assert_eq!(
            strip_scene_tags("Title.REPACJ.REPACK.PROPER.mkv"),
            "Title.mkv"
        );
        assert_eq!(strip_scene_tags("Spider-Man.REPACK"), "Spider-Man");
        assert_eq!(strip_scene_tags("REPACK.PROPER"), "");
        assert!(matches!(
            parse("movies/The.Matrix.(1999)/The.Matrix.(1999).REPACK.mkv"),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        assert!(matches!(
            render(
                &movie_id(),
                &Placement::movie("The.Matrix.PROPER", 1999, "mkv")
            ),
            Err(PathSchemaError::LeftoverSceneTag(_))
        ));
        let stripped = strip_placement(&Placement::movie("The.Matrix.PROPER", 1999, "mkv"));
        assert_eq!(stripped, Placement::movie("The.Matrix", 1999, "mkv"));
    }

    #[test]
    fn remaster_years_share_an_album_identity_but_not_a_path() {
        let a = parse_placement(
            "music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac",
        )
        .expect("1974");
        let b = parse_placement(
            "music/Yes/Relayer.(2013)/Relayer.(2013).01.The.Gates.Of.Delirium.flac",
        )
        .expect("2013");
        assert_eq!(a.0, b.0);
        assert_eq!(a.1.file_key(), b.1.file_key());
        assert_ne!(render_placement(&a.1), render_placement(&b.1));
        // Lidarr multi-disc; `CD1` style discs too.
        let (_, p) = parse_remote(
            Some(TitleKind::Album),
            "Radiohead/Amnesiac.(2001)/CD2/Amnesiac.(2001).04.You.and.Whose.Army.flac",
        )
        .expect("cd2");
        assert_eq!(p.file_key(), FileKey::Track { disc: 2, track: 4 });
        // A track without a number is one Whole per album.
        let (_, p) = parse_placement("music/Yes/Relayer.(1974)/Relayer.(1974).Sound.Chaser.flac")
            .expect("no number");
        assert_eq!(p.file_key(), FileKey::Whole);
        assert!(
            render_placement(&p)
                .expect("render")
                .ends_with("Relayer.(1974).Sound.Chaser.flac")
        );
    }

    #[test]
    fn staging_path_uses_hyphen_token_and_refuses_junk() {
        let path = staging_path(&movie_id(), "The.Matrix.(1999).mkv").expect("staging");
        assert_eq!(
            path.to_str().expect("utf8"),
            "_incoming/movie-key-thematrix.1999/The.Matrix.(1999).mkv"
        );
        assert!(matches!(
            staging_path(&movie_id(), ""),
            Err(PathSchemaError::EmptyFinalName)
        ));
        assert!(matches!(
            staging_path(&movie_id(), "a b.mkv"),
            Err(PathSchemaError::SpaceRefused(_))
        ));
        assert!(staging_path(&movie_id(), "a/b.mkv").is_err());
        assert!(staging_path(&movie_id(), "..").is_err());
    }

    #[test]
    fn unicode_titles_round_trip() {
        for path in [
            "movies/\u{65e5}\u{672c}\u{8a9e}.(2001)/\u{65e5}\u{672c}\u{8a9e}.(2001).mkv",
            "movies/Am\u{e9}lie.(2001)/Am\u{e9}lie.(2001).mkv",
            "music/Sigur.R\u{f3}s/\u{c1}g\u{e6}tis.byrjun.(1999)/\u{c1}g\u{e6}tis.byrjun.(1999).01.Intro.flac",
        ] {
            let (id, p) = parse_placement(path).expect(path);
            assert_eq!(
                render(&id, &p).expect("render").to_str().expect("utf8"),
                path
            );
        }
    }

    #[test]
    fn out_of_range_numbers_refuse() {
        assert!(matches!(
            render_placement(&Placement::episode("X", 2000, 100, 1, "mkv")),
            Err(PathSchemaError::SeasonEpisodeOutOfRange { .. })
        ));
        assert!(matches!(
            render_placement(&Placement::track(
                "A",
                "B",
                2000,
                None,
                Some(0),
                "T",
                "flac"
            )),
            Err(PathSchemaError::TrackOutOfRange(0))
        ));
        assert!(matches!(
            render_placement(&Placement::movie("X", 999, "mkv")),
            Err(PathSchemaError::InvalidYear(999))
        ));
        assert!(render_placement(&Placement::movie("X", 2000, "m k v")).is_err());
    }

    #[test]
    fn placement_labels_and_kinds() {
        assert_eq!(Placement::movie("Coco", 2017, "mkv").label(), "Coco.(2017)");
        assert_eq!(
            Placement::episode("Silo", 2023, 1, 1, "mkv").label(),
            "Silo.(2023) S01E01"
        );
        assert_eq!(relayer_track().label(), "Yes/Relayer.(1974) 01");
        assert_eq!(relayer_track().kind(), TitleKind::Album);
        assert_eq!(relayer_track().extension(), "flac");
        assert_eq!(kind_dir("tv"), Some(TitleKind::Series));
        assert_eq!(kind_dir("photos"), None);
    }

    #[test]
    fn v1_plan_json_placement_still_loads() {
        let json = r#"{"kind":"episode","title":"The.Wire","year":2002,"season":1,"episode":1,"extension":"mkv"}"#;
        let p: Placement = serde_json::from_str(json).expect("v1 episode");
        assert_eq!(p, Placement::episode("The.Wire", 2002, 1, 1, "mkv"));
        let json = r#"{"kind":"track","album":"Relayer","year":2013,"track":1,"title":"X","extension":"flac"}"#;
        let p: Placement = serde_json::from_str(json).expect("v1 track");
        assert_eq!(
            p,
            Placement::track("", "Relayer", 2013, None, Some(1), "X", "flac")
        );
    }
}
