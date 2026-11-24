//! Always-on trace for hang diagnosis: one `<unix ms> <thread> <event>` line per call in
//! `juicetray.log`, written unbuffered so the last line survives a stuck thread.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::query_log;

/// A log above this size at startup starts over, so runs never pile up.
const START_MAX_BYTES: u64 = 1_000_000;
/// A running app starts the file over at this size; the keeper traces every 2 s.
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

/// `juicetray.log` next to `query-log.toml`.
fn open() -> Result<Sink> {
    let path = query_log::path()
        .context("no data directory")?
        .with_file_name("juicetray.log");
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
    line(&format!("start pid {}", std::process::id()));
}

/// Appends `<unix ms> <thread name> <event>`; the lock is held for one small write.
pub fn line(event: &str) {
    let Some(sink) = SINK.get() else {
        return;
    };
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let thread = std::thread::current();
    let text = format!("{millis} {} {event}\n", thread.name().unwrap_or("unnamed"));
    sink.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(&text, RUN_MAX_BYTES);
}

/// Logs `<name> begin` now and `<name> end <n> ms` when dropped, so a stuck step shows as a
/// `begin` with no `end`.
#[must_use = "the end line is written when the span drops"]
pub struct Span {
    name: String,
    started: Instant,
}

pub fn span(name: impl Into<String>) -> Span {
    let name = name.into();
    line(&format!("{name} begin"));
    Span {
        name,
        started: Instant::now(),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        line(&format!(
            "{} end {} ms",
            self.name,
            self.started.elapsed().as_millis()
        ));
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
}
