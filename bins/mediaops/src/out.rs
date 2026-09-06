//! Human stdout: one operator, their machines.
//!
//! Color and bold only when stdout is a tty. Progress writes to stderr and
//! only when that is a tty. JSON output never calls these helpers.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use mediaops_core::{
    Placement, TitleId, TitleIndexEntry, TitleKind, TitleSource, parse_placement, parse_remote,
    title_key,
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
            color: io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").as_deref() != Ok("dumb"),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn plain() -> Self {
        Self { color: false }
    }

    fn paint(self, code: &str, text: &str) -> String {
        let text = inert(text);
        if self.color && !text.is_empty() {
            format!("{code}{text}{RESET}")
        } else {
            text
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

/// Single-line operator text must not execute terminal controls from filenames
/// or remote diagnostics. JSON retains the original values.
pub fn inert(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

pub fn fmt_progress(done: u64, total: u64) -> String {
    if total == 0 {
        return format!("{} / unknown", fmt_bytes(done));
    }
    let percent = u128::from(done.min(total)) * 100 / u128::from(total);
    format!("{percent}%  {} / {}", fmt_bytes(done), fmt_bytes(total))
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
        line.push_str(&inert(meta));
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

/// `\r` progress on stderr. No-op when stderr is not a tty.
pub struct PullMeter {
    title: String,
    active: bool,
    last: Instant,
    last_len: usize,
    painted: bool,
    baseline: Option<(Instant, u64)>,
}

impl PullMeter {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: inert(&title.into()),
            active: io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb"),
            last: Instant::now(),
            last_len: 0,
            painted: false,
            baseline: None,
        }
    }

    pub fn update(&mut self, done: u64, total: u64) {
        if !self.active {
            return;
        }
        let now = Instant::now();
        let baseline = *self.baseline.get_or_insert((now, done));
        let due = !self.painted
            || done >= total
            || now.duration_since(self.last) >= Duration::from_millis(100);
        if !due {
            return;
        }
        self.last = now;
        self.painted = true;
        let mut line = format!("pull      {}  {}", self.title, fmt_progress(done, total),);
        line.push_str(&transfer_rate(
            done,
            total,
            baseline.1,
            now.duration_since(baseline.0),
        ));
        eprint!("\r{}", crate::progress::terminal_line(&line));
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

impl Drop for PullMeter {
    fn drop(&mut self) {
        self.finish();
    }
}

fn transfer_rate(done: u64, total: u64, baseline: u64, elapsed: Duration) -> String {
    let transferred = done.saturating_sub(baseline);
    if elapsed < Duration::from_secs(1) || transferred == 0 {
        return String::new();
    }
    let rate = transferred as f64 / elapsed.as_secs_f64();
    let mut text = format!("  {}/s", fmt_bytes(rate as u64));
    if total > done {
        let remaining = ((total - done) as f64 / rate).ceil() as u64;
        text.push_str(&format!("  ~{} left", fmt_age(remaining)));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn transfer_progress_does_not_count_resumed_bytes_as_speed() {
        assert_eq!(fmt_progress(512, 1024), "50%  512 B / 1 KiB");
        assert_eq!(
            fmt_progress(u64::MAX, u64::MAX),
            "100%  16777216 TiB / 16777216 TiB"
        );
        assert_eq!(fmt_progress(999, 1000), "99%  999 B / 1000 B");
        assert_eq!(fmt_progress(0, 0), "0 B / unknown");
        assert_eq!(transfer_rate(900, 1000, 900, Duration::from_secs(1)), "");
        assert_eq!(
            transfer_rate(950, 1000, 900, Duration::from_secs(5)),
            "  10 B/s  ~5s left"
        );
        assert_eq!(
            transfer_rate(1000, 1000, 900, Duration::from_secs(5)),
            "  20 B/s"
        );
        assert_eq!(
            inert("remote\u{1b}[31m\nfile\tname"),
            "remote [31m file name"
        );
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
            human_title_id(&TitleId::series_key("Foundation", 2021).expect("id")),
            "Foundation (2021)"
        );
        assert_eq!(
            human_from_path("movies/The.Matrix.(1999)/The.Matrix.(1999).mkv").as_deref(),
            Some("The Matrix (1999)")
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
        let hints = [hint_from_path(
            id.clone(),
            "movies/Hearts.of.Darkness.A.Filmmaker's.Apocalypse.(1991)/Hearts.of.Darkness.A.Filmmaker's.Apocalypse.(1991).mkv",
        )];
        assert_eq!(resolve_title("movie:tmdb:4539", &hints).expect("id"), id);
        assert_eq!(
            resolve_title("hearts of darkness", &hints).expect("name"),
            id
        );
        assert!(resolve_title("Silo", &hints).is_err());
        assert!(resolve_title("not-a-title", &[]).is_err());
    }
}
