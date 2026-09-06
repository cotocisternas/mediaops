//! Human progress on stderr, independent of the async runtime and blocking I/O.

use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mediaops_core::Blake3Hex;
use unicode_width::UnicodeWidthChar;

#[derive(Clone)]
struct State {
    label: String,
    stage: String,
    detail: String,
    bytes: Option<(u64, u64)>,
    finished: bool,
}

pub(crate) struct OperationProgress {
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    started: Instant,
    enabled: bool,
    terminal: bool,
    completed: bool,
}

impl OperationProgress {
    pub fn new(enabled: bool, label: impl Into<String>) -> Self {
        Self::build(enabled, label, true)
    }

    /// Commands that also emit warnings use complete lines so tracing never
    /// lands in the middle of an in-place indicator.
    pub fn lines(enabled: bool, label: impl Into<String>) -> Self {
        Self::build(enabled, label, false)
    }

    fn build(enabled: bool, label: impl Into<String>, in_place: bool) -> Self {
        let state = State {
            label: plain(&label.into()),
            stage: "starting".into(),
            detail: String::new(),
            bytes: None,
            finished: false,
        };
        let started = Instant::now();
        let terminal = in_place
            && io::stderr().is_terminal()
            && std::env::var("TERM").is_ok_and(|term| term != "dumb");
        if enabled {
            paint(&state, Duration::ZERO, terminal);
        }
        let state = Arc::new((Mutex::new(state), Condvar::new()));
        let worker = enabled
            .then(|| {
                let shared = state.clone();
                std::thread::Builder::new()
                    .name("operator-progress".into())
                    .spawn(move || {
                        let (lock, wake) = &*shared;
                        let mut last_line = Instant::now();
                        loop {
                            let guard = lock.lock().unwrap_or_else(|err| err.into_inner());
                            let (guard, _) = wake
                                .wait_timeout_while(guard, Duration::from_millis(250), |s| {
                                    !s.finished
                                })
                                .unwrap_or_else(|err| err.into_inner());
                            if guard.finished {
                                break;
                            }
                            let snapshot = guard.clone();
                            drop(guard);
                            // Redirected logs get bounded line output; terminals update in place.
                            if terminal || last_line.elapsed() >= Duration::from_secs(5) {
                                paint(&snapshot, started.elapsed(), terminal);
                                last_line = Instant::now();
                            }
                        }
                    })
                    .ok()
            })
            .flatten();
        Self {
            state,
            worker,
            started,
            enabled,
            terminal,
            completed: false,
        }
    }

    pub fn stage(&self, stage: &str, detail: impl Into<String>) {
        let mut state = self.state.0.lock().unwrap_or_else(|err| err.into_inner());
        state.stage = plain(stage);
        state.detail = plain(&detail.into());
        state.bytes = None;
    }

    pub fn bytes(&self, done: u64, total: u64) {
        self.state
            .0
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .bytes = Some((done, total));
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// End the progress line before the caller prints its successful result.
    pub fn finish(&mut self) {
        self.completed = true;
        self.stop();
    }

    fn stop(&mut self) {
        let snapshot = {
            let mut state = self.state.0.lock().unwrap_or_else(|err| err.into_inner());
            if state.finished {
                return;
            }
            state.finished = true;
            state.clone()
        };
        self.state.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if self.enabled {
            let mut final_state = snapshot;
            final_state.stage = if self.completed { "done" } else { "stopped" }.into();
            final_state.detail.clear();
            final_state.bytes = None;
            paint(&final_state, self.started.elapsed(), self.terminal);
            if self.terminal {
                eprintln!();
            }
        }
    }
}

impl Drop for OperationProgress {
    fn drop(&mut self) {
        self.stop();
    }
}

fn plain(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn line(state: &State, elapsed: Duration) -> String {
    let mut text = format!("{}  {}", state.label, state.stage);
    if let Some((done, total)) = state.bytes {
        if total > 0 {
            let percent = (u128::from(done.min(total)) * 100 / u128::from(total)) as u64;
            text.push_str(&format!("  {percent}%"));
        }
        text.push_str(&format!(
            "  {} / {}",
            crate::out::fmt_bytes(done),
            crate::out::fmt_bytes(total)
        ));
    }
    text.push_str(&format!("  elapsed {}", duration(elapsed)));
    if !state.detail.is_empty() {
        text.push_str("  ");
        text.push_str(&state.detail);
    }
    text
}

pub(crate) fn duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 3600 {
        format!("{}h {:02}m {:02}s", secs / 3600, secs / 60 % 60, secs % 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn terminal_width() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
        && size.ws_col > 0
    {
        usize::from(size.ws_col).saturating_sub(1)
    } else {
        79
    }
}

fn clipped(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        let next = c.width().unwrap_or(0);
        if used + next > width {
            break;
        }
        out.push(c);
        used += next;
    }
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

pub(crate) fn terminal_line(text: &str) -> String {
    clipped(text, terminal_width())
}

fn paint(state: &State, elapsed: Duration, terminal: bool) {
    let text = line(state, elapsed);
    let mut stderr = io::stderr().lock();
    if terminal {
        let _ = write!(stderr, "\r{}", clipped(&text, terminal_width()));
    } else {
        let _ = writeln!(stderr, "{text}");
    }
    let _ = stderr.flush();
}

struct MeteredReader<'a, R> {
    reader: R,
    progress: &'a OperationProgress,
    done: u64,
    total: u64,
}

impl<R: Read> Read for MeteredReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = loop {
            match self.reader.read(buf) {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                result => break result?,
            }
        };
        self.done = self.done.saturating_add(size as u64);
        self.progress.bytes(self.done, self.total);
        Ok(size)
    }
}

pub(crate) fn hash_file(path: &Path, progress: &OperationProgress) -> io::Result<Blake3Hex> {
    let reader = std::fs::File::open(path)?;
    let total = reader.metadata()?.len();
    progress.bytes(0, total);
    Blake3Hex::of_reader(MeteredReader {
        reader,
        progress,
        done: 0,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_screen_reports_stage_bytes_elapsed_and_safe_detail() {
        let progress = OperationProgress::new(false, "reindex");
        progress.stage("hashing 2/4", "映画\nfile\u{1b}.mkv");
        progress.bytes(512, 1024);
        let state = progress.state.0.lock().unwrap().clone();
        assert_eq!(
            line(&state, Duration::from_secs(65)),
            "reindex  hashing 2/4  50%  512 B / 1 KiB  elapsed 1m 05s  映画 file .mkv"
        );
        assert_eq!(clipped("映画 abc", 6), "映画 a");
        progress.bytes(1, 0);
        assert!(!line(&progress.state.0.lock().unwrap(), Duration::ZERO).contains('%'));
    }

    #[test]
    fn hash_reader_counts_real_bytes_and_preserves_digest_and_errors() {
        let progress = OperationProgress::new(false, "reindex");
        let bytes = vec![7; 200_000];
        let reader = MeteredReader {
            reader: io::Cursor::new(&bytes),
            progress: &progress,
            done: 0,
            total: bytes.len() as u64,
        };
        assert_eq!(
            Blake3Hex::of_reader(reader).unwrap(),
            Blake3Hex::of_bytes(&bytes)
        );
        assert_eq!(
            progress.state.0.lock().unwrap().bytes,
            Some((200_000, 200_000))
        );
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("disk unavailable"))
            }
        }
        let reader = MeteredReader {
            reader: Failed,
            progress: &progress,
            done: 0,
            total: 1,
        };
        assert_eq!(
            Blake3Hex::of_reader(reader).unwrap_err().to_string(),
            "disk unavailable"
        );
    }
}
