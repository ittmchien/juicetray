//! `juicetray.log`: one `<unix ms> <thread> <event>` line per call, written unbuffered so the
//! last line survives a stuck thread. `line` always writes (panics, errors, hang dumps);
//! `trace` and `span` write in dev and beta builds; an official release (built with
//! `JUICETRAY_CHANNEL=stable`) writes them only with `JUICETRAY_TRACE=1`.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::query_log;

/// A log above this size at startup starts over, so runs never pile up.
const START_MAX_BYTES: u64 = 1_000_000;
/// A running app starts the file over at this size.
const RUN_MAX_BYTES: u64 = 8_000_000;

struct Sink {
    file: File,
    bytes: u64,
}

impl Sink {
    /// Appends `text`; past `max` bytes the file starts over first.
    fn push(&mut self, text: &str, max: u64) {
        if self.bytes + text.len() as u64 > max
            && self.file.set_len(0).is_ok()
            && self.file.seek(SeekFrom::Start(0)).is_ok()
        {
            self.bytes = 0;
        }
        // A failed write only loses a trace line; the app must never stop for it.
        if self.file.write_all(text.as_bytes()).is_ok() {
            self.bytes += text.len() as u64;
        }
    }
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
static TRACE: OnceLock<bool> = OnceLock::new();

/// Whether routine tracing is on: always in debug builds, in release only with
/// `JUICETRAY_TRACE=1`; read once.
fn tracing() -> bool {
    *TRACE.get_or_init(|| {
        !is_stable_build() || std::env::var_os("JUICETRAY_TRACE").is_some_and(|value| value == "1")
    })
}

/// Only official releases (CI sets `JUICETRAY_CHANNEL=stable`) trace off by default; dev and
/// beta builds keep the trace so testers can send a log.
fn is_stable_build() -> bool {
    !cfg!(debug_assertions) && option_env!("JUICETRAY_CHANNEL") == Some("stable")
}

/// `juicetray.log` next to `query-log.toml`; `None` when the OS has no data folder.
pub fn path() -> Option<std::path::PathBuf> {
    Some(query_log::path()?.with_file_name("juicetray.log"))
}

fn open() -> Result<Sink> {
    let path = path().context("no data directory")?;
    let dir = path.parent().context("path has no parent folder")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    // Not `append`: Windows append handles cannot truncate, and one process writes the file.
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    if file.metadata()?.len() > START_MAX_BYTES {
        file.set_len(0)?;
    }
    let bytes = file.seek(SeekFrom::End(0))?;
    Ok(Sink { file, bytes })
}

/// Opens the log and records panics in it; without a log file the app runs unlogged.
pub fn init() {
    match open() {
        Ok(sink) => {
            // `init` runs once; a second call keeps the first sink.
            let _ = SINK.set(Mutex::new(sink));
        }
        Err(error) => eprintln!("trace log unavailable: {error:#}"),
    }
    // Release builds abort on panic, so the hook is the only place a panic can be recorded.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        line(&format!("PANIC {}", info.to_string().replace('\n', " | ")));
        previous(info);
    }));
    trace(format_args!("start pid {}", std::process::id()));
}

/// Appends `<unix ms> <thread name> <event>`; the lock is held for one small write.
pub fn line(event: &str) {
    let Some(sink) = SINK.get() else {
        return;
    };
    write_line(sink, event);
}

fn write_line(sink: &Mutex<Sink>, event: &str) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let thread = std::thread::current();
    let text = format!(
        "{millis} {} {event}
",
        thread.name().unwrap_or("unnamed")
    );
    sink.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(&text, RUN_MAX_BYTES);
}

/// A routine event: written only when tracing is on, and `event` is not formatted otherwise.
pub fn trace(event: fmt::Arguments) {
    trace_to(tracing(), SINK.get(), event);
}

fn trace_to(enabled: bool, sink: Option<&Mutex<Sink>>, event: fmt::Arguments) {
    if let (true, Some(sink)) = (enabled, sink) {
        write_line(sink, &event.to_string());
    }
}

/// Logs `<name> begin` now and `<name> end <n> ms` when dropped, so a stuck step shows as a
/// `begin` with no `end`. Off unless tracing is on: then it holds nothing and writes nothing.
#[must_use = "the end line is written when the span drops"]
pub struct Span(Option<(String, Instant)>);

pub fn span(name: &str) -> Span {
    span_fmt(format_args!("{name}"))
}

/// `span` with a formatted name; the name is built only when tracing is on.
pub fn span_fmt(name: fmt::Arguments) -> Span {
    Span::start(tracing(), name)
}

impl Span {
    fn start(enabled: bool, name: fmt::Arguments) -> Self {
        if !enabled {
            return Self(None);
        }
        let name = name.to_string();
        line(&format!("{name} begin"));
        Self(Some((name, Instant::now())))
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, started)) = &self.0 {
            line(&format!("{name} end {} ms", started.elapsed().as_millis()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_starts_over_past_the_cap() {
        let path = std::env::temp_dir().join(format!("juicetray-log-test-{}", std::process::id()));
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let mut sink = Sink { file, bytes: 0 };

        sink.push("aaaa\n", 12);
        sink.push("bbbb\n", 12);
        sink.push("cccc\n", 12);

        let text = fs::read_to_string(&path).unwrap();
        let bytes = sink.bytes;
        drop(sink);
        fs::remove_file(&path).unwrap();
        assert_eq!(text, "cccc\n");
        assert_eq!(bytes, 5);
    }

    #[test]
    fn trace_and_span_write_nothing_when_off() {
        let path =
            std::env::temp_dir().join(format!("juicetray-trace-test-{}", std::process::id()));
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let sink = Mutex::new(Sink { file, bytes: 0 });

        trace_to(false, Some(&sink), format_args!("off"));
        let off = Span::start(false, format_args!("off span"));
        let off_is_empty = off.0.is_none();
        drop(off);
        let quiet = fs::read_to_string(&path).unwrap();
        trace_to(true, Some(&sink), format_args!("on"));
        let loud = fs::read_to_string(&path).unwrap();
        drop(sink);
        fs::remove_file(&path).unwrap();

        assert!(off_is_empty);
        assert_eq!(quiet, "");
        assert!(loud.trim_end().ends_with(" on"));
    }
}
