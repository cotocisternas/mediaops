//! Human stdout: one operator, their machines.
//!
//! Color and bold only when stdout is a tty. Progress writes to stderr and
//! only when that is a tty. `--json` never calls these helpers.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use mediaops_core::{
    HoldLiveItem, Job, Placement, TitleId, TitleIndexEntry, TitleKind, TitleSource,
    parse_placement, parse_remote, title_key,
};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";

const VERB_WIDTH: usize = 10;

#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    pub fn stdout() -> Self {
        Self {
            color: io::stdout().is_terminal(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn plain() -> Self {
        Self { color: false }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color && !text.is_empty() {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint(BOLD, text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint(DIM, text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint(GREEN, text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.paint(YELLOW, text)
    }

    pub fn red(self, text: &str) -> String {
        self.paint(RED, text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Go,
    Wait,
    Quiet,
    Bad,
}

pub fn fmt_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    let x = n as f64;
    if x >= TIB {
        fmt_scaled(x / TIB, "TiB")
    } else if x >= GIB {
        fmt_scaled(x / GIB, "GiB")
    } else if x >= MIB {
        format!("{:.0} MiB", x / MIB)
    } else if x >= KIB {
        format!("{:.0} KiB", x / KIB)
    } else {
        format!("{n} B")
    }
}

fn fmt_scaled(n: f64, unit: &str) -> String {
    if n >= 10.0 && (n - n.round()).abs() < 0.05 {
        format!("{:.0} {unit}", n.round())
    } else {
        format!("{n:.1} {unit}")
    }
}

pub fn fmt_age(secs: u64) -> String {
    if secs < 90 {
        format!("{secs}s")
    } else if secs < 90 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 48 * 3600 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// `Hearts of Darkness A Filmmaker's Apocalypse (1991)`, `Mr Robot (2015) S01E02`.
pub fn human_placement(placement: &Placement) -> String {
    match placement {
        Placement::Movie { title, year, .. } => format!("{} ({year})", undot(title)),
        Placement::Episode {
            title,
            year,
            season,
            episode,
            episode_end,
            ..
        } => {
            let ep = match episode_end {
                Some(end) => format!("S{season:02}E{episode:02}-E{end:02}"),
                None => format!("S{season:02}E{episode:02}"),
            };
            format!("{} ({year}) {ep}", undot(title))
        }
        Placement::Track {
            artist,
            album,
            year,
            track,
            ..
        } => match track {
            Some(n) => format!("{} / {} ({year}) {n:02}", undot(artist), undot(album)),
            None => format!("{} / {} ({year})", undot(artist), undot(album)),
        },
    }
}

pub fn placement_from_path(path: &str) -> Option<Placement> {
    parse_placement(Path::new(path))
        .or_else(|_| parse_remote(None, Path::new(path)))
        .ok()
        .map(|(_, placement)| placement)
}

pub fn human_from_path(path: &str) -> Option<String> {
    placement_from_path(path).as_ref().map(human_placement)
}

/// Show / album / movie — not a single episode. For `why` of a TitleId.
pub fn human_title_from_placement(placement: &Placement) -> String {
    match placement {
        Placement::Episode { title, year, .. } => format!("{} ({year})", undot(title)),
        other => human_placement(other),
    }
}

/// Best-effort headline from a TitleId when no placement is around.
pub fn human_title_id(id: &TitleId) -> String {
    if id.source() != TitleSource::Key {
        return id.render();
    }
    match id.kind() {
        TitleKind::Movie | TitleKind::Series => {
            let raw = id.id();
            let Some((name, year)) = raw.rsplit_once('.') else {
                return id.render();
            };
            if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) {
                return format!("{} ({year})", title_case_key(name));
            }
            id.render()
        }
        TitleKind::Album => {
            let raw = id.id();
            let Some((artist, album)) = raw.split_once('.') else {
                return id.render();
            };
            format!("{} / {}", title_case_key(artist), title_case_key(album))
        }
    }
}

pub fn human_title_id_str(rendered: &str) -> String {
    TitleId::parse(rendered)
        .map(|id| human_title_id(&id))
        .unwrap_or_else(|_| rendered.to_string())
}

/// Dotted schema labels (`Mr.Robot.(2015) S01E02`) → the same human form.
pub fn humanize_schema_label(s: &str) -> String {
    if let Some((head, rest)) = s.split_once(" S")
        && looks_like_episode(rest)
    {
        return format!("{} S{rest}", undot_year(head));
    }
    undot_year(s).replace('/', " / ")
}

fn looks_like_episode(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() >= 5 && bytes[0].is_ascii_digit() && rest.contains('E')
}

fn undot_year(s: &str) -> String {
    undot(&s.replace(".(", " ("))
}

fn undot(s: &str) -> String {
    s.split('.')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case_key(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn row(style: Style, verb: &str, tone: Tone, title: &str, meta: &str) -> String {
    let painted = match tone {
        Tone::Go => style.green(verb),
        Tone::Wait => style.yellow(verb),
        Tone::Quiet => style.dim(verb),
        Tone::Bad => style.red(verb),
    };
    let pad = VERB_WIDTH.saturating_sub(verb.chars().count());
    let mut line = format!("{painted}{:pad$}", "");
    if !title.is_empty() {
        line.push_str(&style.bold(title));
    }
    if !meta.is_empty() {
        if !title.is_empty() {
            line.push_str("  ");
        }
        line.push_str(meta);
    }
    line
}

pub fn indent(style: Style, text: &str) -> String {
    format!("{}{}", " ".repeat(VERB_WIDTH), style.dim(text))
}

pub fn finish(mut lines: Vec<String>) -> String {
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// A name the operator might type, bound to a TitleId.
#[derive(Debug, Clone)]
pub struct TitleHint {
    pub id: TitleId,
    pub names: Vec<String>,
}

pub fn hint_from_id(id: TitleId) -> TitleHint {
    TitleHint {
        names: vec![human_title_id(&id), id.render()],
        id,
    }
}

pub fn hint_from_path(id: TitleId, path: &str) -> TitleHint {
    let mut names = Vec::new();
    if let Some(human) = human_from_path(path) {
        names.push(human);
    }
    names.push(human_title_id(&id));
    names.push(id.render());
    names.push(path.to_string());
    TitleHint { id, names }
}

pub fn hint_from_placement(id: TitleId, placement: &Placement) -> TitleHint {
    TitleHint {
        id: id.clone(),
        names: vec![
            human_placement(placement),
            human_title_id(&id),
            id.render(),
            placement.label(),
        ],
    }
}

pub fn hints_from_index(titles: &[TitleIndexEntry]) -> Vec<TitleHint> {
    titles
        .iter()
        .map(|row| {
            if row.path_missing() {
                hint_from_id(row.title_id().clone())
            } else {
                hint_from_path(row.title_id().clone(), row.path())
            }
        })
        .collect()
}

pub fn hints_from_jobs(jobs: &[Job]) -> Vec<TitleHint> {
    jobs.iter()
        .map(|job| hint_from_id(job.title_id().clone()))
        .collect()
}

pub fn hints_from_holds(holds: &[HoldLiveItem]) -> Vec<TitleHint> {
    holds
        .iter()
        .map(|item| match &item.placement {
            Some(placement) => hint_from_placement(item.key.title_id.clone(), placement),
            None => {
                let mut hint = hint_from_id(item.key.title_id.clone());
                if let Some(path) = &item.output_path {
                    hint.names.push(path.clone());
                }
                hint
            }
        })
        .collect()
}

pub fn merge_hints(hints: Vec<TitleHint>) -> Vec<TitleHint> {
    let mut out: Vec<TitleHint> = Vec::new();
    for hint in hints {
        if let Some(existing) = out.iter_mut().find(|h| h.id == hint.id) {
            for name in hint.names {
                if !existing.names.contains(&name) {
                    existing.names.push(name);
                }
            }
        } else {
            out.push(hint);
        }
    }
    out
}

pub fn resolve_title(query: &str, hints: &[TitleHint]) -> Result<TitleId, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("say a title or a title id".into());
    }
    if let Ok(id) = TitleId::parse(query) {
        return Ok(id);
    }
    let needle = title_key(query);
    if needle.is_empty() {
        return Err(format!("no title matches `{query}`"));
    }
    let mut hits: Vec<&TitleHint> = hints
        .iter()
        .filter(|hint| {
            hint.names
                .iter()
                .any(|name| title_key(name).contains(&needle))
        })
        .collect();
    hits.sort_by(|a, b| a.id.render().cmp(&b.id.render()));
    hits.dedup_by(|a, b| a.id == b.id);
    match hits.len() {
        1 => Ok(hits[0].id.clone()),
        0 => Err(format!("no title matches `{query}`")),
        n => Err(format!("`{query}` matches {n} titles; use the id")),
    }
}

pub fn names_for(id: &TitleId, hints: &[TitleHint]) -> String {
    hints
        .iter()
        .find(|h| h.id == *id)
        .and_then(|h| h.names.first())
        .cloned()
        .unwrap_or_else(|| human_title_id(id))
}

/// `\r` progress on stderr. No-op when stderr is not a tty.
pub struct PullMeter {
    title: String,
    active: bool,
    last: Instant,
    last_len: usize,
    painted: bool,
}

impl PullMeter {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            active: io::stderr().is_terminal(),
            last: Instant::now(),
            last_len: 0,
            painted: false,
        }
    }

    pub fn update(&mut self, done: u64, total: u64) {
        if !self.active {
            return;
        }
        let now = Instant::now();
        let due = !self.painted
            || done >= total
            || now.duration_since(self.last) >= Duration::from_millis(100);
        if !due {
            return;
        }
        self.last = now;
        self.painted = true;
        let line = format!(
            "pull    {}  {} / {}",
            self.title,
            fmt_bytes(done),
            fmt_bytes(total)
        );
        let width = self.last_len.max(line.len());
        eprint!("\r{line:<width$}");
        let _ = io::stderr().flush();
        self.last_len = line.len();
        if total > 0 && done >= total {
            eprintln!();
            self.active = false;
        }
    }

    pub fn finish(&mut self) {
        if self.active && self.last_len > 0 {
            eprintln!();
        }
        self.active = false;
    }
}

pub fn lock_command(lock: &serde_json::Value) -> String {
    lock.get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            lock.get("unparsed")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "mediaops".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{HoldKey, ReleaseId};

    #[test]
    fn bytes_and_age_are_what_an_operator_says() {
        assert_eq!(fmt_bytes(7_588_856_506), "7.1 GiB");
        assert_eq!(fmt_bytes(7_250_189_951), "6.8 GiB");
        assert_eq!(fmt_bytes(4_262_637_394), "4.0 GiB");
        assert_eq!(fmt_bytes(3_773), "4 KiB");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(4_182_917_251_072), "3.8 TiB");
        assert_eq!(fmt_bytes(274_877_906_944), "256 GiB");
        assert_eq!(fmt_bytes(744_189_419_520), "693.1 GiB");
        assert_eq!(fmt_age(12), "12s");
        assert_eq!(fmt_age(748), "12m");
        assert_eq!(fmt_age(4_000), "66m");
        assert_eq!(fmt_age(8_000), "2h");
        assert_eq!(fmt_age(200_000), "2d");
    }

    #[test]
    fn titles_are_spoken_english() {
        assert_eq!(
            human_placement(&Placement::movie(
                "Hearts.of.Darkness.A.Filmmaker's.Apocalypse",
                1991,
                "mkv"
            )),
            "Hearts of Darkness A Filmmaker's Apocalypse (1991)"
        );
        assert_eq!(
            human_placement(&Placement::episode("Mr.Robot", 2015, 1, 2, "mkv")),
            "Mr Robot (2015) S01E02"
        );
        assert_eq!(
            human_placement(&Placement::track(
                "Yes",
                "Relayer",
                2013,
                None,
                Some(1),
                "The.Gates.Of.Delirium",
                "flac"
            )),
            "Yes / Relayer (2013) 01"
        );
        assert_eq!(
            humanize_schema_label("Mr.Robot.(2015) S01E02"),
            "Mr Robot (2015) S01E02"
        );
        assert_eq!(
            human_title_id(&TitleId::series_key("Foundation", 2021).expect("id")),
            "Foundation (2021)"
        );
        assert_eq!(
            human_from_path("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv").as_deref(),
            Some("The Matrix (1999)")
        );
        assert_eq!(
            human_title_from_placement(&placement_from_path(
                "Mr.Robot.(2015)/Season.01/Mr.Robot.(2015).S01E02.eps1.1_ones-and-zer0es.mpeg.mkv"
            )
            .expect("remote episode")),
            "Mr Robot (2015)"
        );
    }

    #[test]
    fn rows_align_without_color_in_tests() {
        let style = Style::plain();
        assert_eq!(
            row(
                style,
                "copy",
                Tone::Go,
                "Hearts of Darkness (1991)",
                "7.1 GiB"
            ),
            "copy      Hearts of Darkness (1991)  7.1 GiB"
        );
        assert_eq!(
            indent(style, "movie:tmdb:4539"),
            "          movie:tmdb:4539"
        );
    }

    #[test]
    fn resolve_title_accepts_id_or_unique_name() {
        let id = TitleId::movie("4539").expect("id");
        let hints = [hint_from_placement(
            id.clone(),
            &Placement::movie("Hearts.of.Darkness.A.Filmmaker's.Apocalypse", 1991, "mkv"),
        )];
        assert_eq!(resolve_title("movie:tmdb:4539", &hints).expect("id"), id);
        assert_eq!(
            resolve_title("hearts of darkness", &hints).expect("name"),
            id
        );
        assert!(resolve_title("Silo", &hints).is_err());
        assert!(resolve_title("not-a-title", &[]).is_err());
    }

    #[test]
    fn hold_hint_uses_placement() {
        let mut item = HoldLiveItem::new(
            HoldKey::new(
                TitleId::movie("4539").expect("id"),
                ReleaseId::parse("deadbeef").expect("rel"),
            ),
            0,
            1,
            "blocked",
        );
        item.placement = Some(Placement::movie(
            "Hearts.of.Darkness.A.Filmmaker's.Apocalypse",
            1991,
            "mkv",
        ));
        let hints = hints_from_holds(&[item]);
        assert_eq!(
            resolve_title("Hearts of Darkness", &hints)
                .expect("hit")
                .render(),
            "movie:tmdb:4539"
        );
    }
}
