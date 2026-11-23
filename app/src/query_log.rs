//! Last battery-query time and last reading per device (R21), so the per-family query floor
//! holds across runs and processes.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use protocol::Reading;
use toml::{Table, Value};

use crate::config::{project_dirs, write_atomically};

/// Top-level table name for stored readings; no `VID:PID` key can collide with it.
const READINGS: &str = "readings";

/// A reading with the Unix seconds it was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stored {
    reading: Reading,
    read_at: u64,
}

/// `VID:PID` (upper-case hex) → Unix seconds of the last query sent to that device, plus the
/// last decoded reading per device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryLog {
    last: BTreeMap<String, u64>,
    readings: BTreeMap<String, Stored>,
}

/// `query-log.toml` in the OS local data folder; `None` when the OS has no such folder.
pub fn path() -> Option<PathBuf> {
    Some(project_dirs()?.data_local_dir().join("query-log.toml"))
}

fn key(vid: u16, pid: u16) -> String {
    format!("{vid:04X}:{pid:04X}")
}

/// Whole seconds since 1970; a time before 1970 is 0.
fn unix_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn from_unix_secs(secs: u64) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

fn secs_of(value: &Value) -> Option<u64> {
    u64::try_from(value.as_integer()?).ok()
}

/// `None` unless every field is present and in range.
fn parse_stored(value: &Value) -> Option<Stored> {
    let table = value.as_table()?;
    Some(Stored {
        reading: Reading {
            percent: u8::try_from(table.get("percent")?.as_integer()?).ok()?,
            charging: table.get("charging")?.as_bool()?,
            asleep: table.get("asleep")?.as_bool()?,
        },
        read_at: secs_of(table.get("read_at")?)?,
    })
}

impl QueryLog {
    /// A missing or unreadable file counts as empty; an invalid entry is dropped on its own.
    pub fn load_from(path: &Path) -> QueryLog {
        let table: Table = fs::read_to_string(path)
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or_default();
        let mut log = QueryLog::default();
        for (key, value) in table {
            if key == READINGS {
                let entries = value.as_table().into_iter().flatten();
                log.readings.extend(
                    entries.filter_map(|(key, value)| Some((key.clone(), parse_stored(value)?))),
                );
            } else if let Some(secs) = secs_of(&value) {
                log.last.insert(key, secs);
            }
        }
        log
    }

    /// `None` also when the stored seconds do not fit a `SystemTime`.
    pub fn last(&self, vid: u16, pid: u16) -> Option<SystemTime> {
        from_unix_secs(*self.last.get(&key(vid, pid))?)
    }

    /// Records `at` in whole seconds; a time before 1970 is stored as 0. Stored readings stay.
    pub fn stamp(&mut self, vid: u16, pid: u16, at: SystemTime) {
        self.last.insert(key(vid, pid), unix_secs(at));
    }

    /// The last decoded reading and when it was taken; the refresh worker reads it between queries.
    pub fn reading(&self, vid: u16, pid: u16) -> Option<(Reading, SystemTime)> {
        let stored = self.readings.get(&key(vid, pid))?;
        Some((stored.reading, from_unix_secs(stored.read_at)?))
    }

    /// Keeps `reading` taken at `read_at` (whole seconds); the query stamp is not touched.
    pub fn record_reading(&mut self, vid: u16, pid: u16, reading: Reading, read_at: SystemTime) {
        let read_at = unix_secs(read_at);
        self.readings
            .insert(key(vid, pid), Stored { reading, read_at });
    }

    /// Integer stamps first, then the `[readings]` table, so older builds still read the stamps.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let stamps: Table = self
            .last
            .iter()
            .map(|(key, &secs)| {
                let secs = i64::try_from(secs).unwrap_or(i64::MAX);
                (key.clone(), Value::Integer(secs))
            })
            .collect();
        let mut text = stamps.to_string();
        if !self.readings.is_empty() {
            text.push_str(&format!("\n[{READINGS}]\n"));
            for (key, stored) in &self.readings {
                let Reading {
                    percent,
                    charging,
                    asleep,
                } = stored.reading;
                let read_at = i64::try_from(stored.read_at).unwrap_or(i64::MAX);
                text.push_str(&format!(
                    "{} = {{ percent = {percent}, charging = {charging}, asleep = {asleep}, \
                     read_at = {read_at} }}\n",
                    Value::String(key.clone())
                ));
            }
        }
        write_atomically(path, &text)
    }

    /// Runs `f` on the log as it is on disk right now, under an OS lock on `query-log.lock`
    /// next to the log, so two processes never interleave; saves only when `f` returns `Ok`.
    /// The OS drops the lock when the process dies.
    pub fn update<T, E: From<anyhow::Error>>(
        path: &Path,
        f: impl FnOnce(&mut QueryLog) -> Result<T, E>,
    ) -> Result<T, E> {
        let dir = path.parent().context("path has no parent folder")?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let lock_path = dir.join("query-log.lock");
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open {}", lock_path.display()))?;
        lock.lock()
            .with_context(|| format!("lock {}", lock_path.display()))?;

        let mut log = QueryLog::load_from(path);
        let value = f(&mut log)?;
        log.save_to(path)?;
        // The lock is released when `lock` drops after the save.
        drop(lock);
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the OS temp dir, unique per test and process.
    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("juicetray-query-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn query_log_roundtrip() {
        let root = temp_root("roundtrip");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("query-log.toml");
        let mut log = QueryLog::default();
        log.stamp(
            0x3837,
            0x6011,
            at(1_800_000_000) + Duration::from_millis(700),
        );

        log.save_to(&path).unwrap();
        let loaded = QueryLog::load_from(&path);
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.last(0x3837, 0x6011), Some(at(1_800_000_000)));
        assert_eq!(loaded.last(0x3837, 0x6010), None);
        assert!(text.contains("3837:6011"), "{text}");
    }

    #[test]
    fn query_log_garbage_is_empty() {
        let root = temp_root("garbage");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("broken.toml");
        fs::write(&broken, "not = [toml").unwrap();
        let odd = root.join("odd.toml");
        fs::write(
            &odd,
            "\"3837:6011\" = \"yesterday\"\n\"373B:1204\" = -5\n\"3554:FA09\" = 9223372036854775807\n",
        )
        .unwrap();

        let broken_log = QueryLog::load_from(&broken);
        let odd_log = QueryLog::load_from(&odd);
        let missing_log = QueryLog::load_from(&root.join("missing.toml"));
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(broken_log, QueryLog::default());
        assert_eq!(missing_log, QueryLog::default());
        assert_eq!(odd_log.last(0x3837, 0x6011), None);
        assert_eq!(odd_log.last(0x373B, 0x1204), None);
        // Too far in the future for `SystemTime` on some platforms; never a panic.
        let _ = odd_log.last(0x3554, 0xFA09);
    }

    #[test]
    fn query_log_save_creates_missing_folder() {
        let root = temp_root("folder");
        let path = root.join("nested").join("query-log.toml");
        let mut log = QueryLog::default();
        log.stamp(0x373B, 0x1204, at(1_800_000_000));

        log.save_to(&path).unwrap();
        let loaded = QueryLog::load_from(&path);
        let files: Vec<_> = fs::read_dir(root.join("nested"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded, log);
        assert_eq!(files, ["query-log.toml"], "no temp file is left behind");
    }

    fn sample() -> Reading {
        Reading {
            percent: 92,
            charging: true,
            asleep: false,
        }
    }

    #[test]
    fn integer_only_file_still_loads() {
        let root = temp_root("integer-only");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("query-log.toml");
        fs::write(&path, "\"3837:6011\" = 1800000000\n").unwrap();

        let loaded = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.last(0x3837, 0x6011), Some(at(1_800_000_000)));
        assert_eq!(loaded.reading(0x3837, 0x6011), None);
    }

    #[test]
    fn readings_roundtrip_next_to_stamps() {
        let root = temp_root("readings");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("query-log.toml");
        let mut log = QueryLog::default();
        log.stamp(0x3837, 0x6011, at(1_800_000_000));
        log.record_reading(
            0x3837,
            0x6011,
            sample(),
            at(1_800_000_001) + Duration::from_millis(900),
        );

        log.save_to(&path).unwrap();
        let loaded = QueryLog::load_from(&path);
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.last(0x3837, 0x6011), Some(at(1_800_000_000)));
        assert_eq!(
            loaded.reading(0x3837, 0x6011),
            Some((sample(), at(1_800_000_001)))
        );
        assert_eq!(loaded.reading(0x3837, 0x6010), None);
        assert!(text.starts_with("\"3837:6011\" = 1800000000\n"), "{text}");
        assert!(text.contains("[readings]"), "{text}");
    }

    #[test]
    fn bad_reading_entries_are_dropped_one_by_one() {
        let root = temp_root("bad-readings");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("query-log.toml");
        fs::write(
            &path,
            "\"3837:6011\" = 5\n[readings]\n\
             \"3837:6011\" = { percent = 92, charging = false, asleep = false, read_at = 7 }\n\
             \"3837:6010\" = { percent = 300, charging = false, asleep = false, read_at = 7 }\n\
             \"373B:1204\" = { percent = 50, charging = \"no\", asleep = false, read_at = 7 }\n\
             \"3554:FA09\" = { percent = 50, charging = false, asleep = false }\n\
             \"373B:1107\" = 12\n",
        )
        .unwrap();

        let loaded = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.last(0x3837, 0x6011), Some(at(5)));
        assert!(loaded.reading(0x3837, 0x6011).is_some());
        for (vid, pid) in [
            (0x3837, 0x6010),
            (0x373B, 0x1204),
            (0x3554, 0xFA09),
            (0x373B, 0x1107),
        ] {
            assert_eq!(loaded.reading(vid, pid), None, "{vid:04X}:{pid:04X}");
        }
    }

    #[test]
    fn update_keeps_a_stamp_another_writer_saved() {
        let root = temp_root("update-merge");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("query-log.toml");
        // This process loaded the log before another writer stamped the file.
        let loaded = QueryLog::load_from(&path);
        let mut other = QueryLog::default();
        other.stamp(0x3837, 0x6011, at(1_800_000_000));
        other.save_to(&path).unwrap();

        let result = QueryLog::update(&path, |log| {
            log.record_reading(0x3837, 0x6011, sample(), at(1_800_000_050));
            Ok::<_, anyhow::Error>(())
        });
        let saved = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        result.unwrap();
        assert_eq!(loaded, QueryLog::default());
        assert_eq!(saved.last(0x3837, 0x6011), Some(at(1_800_000_000)));
        assert_eq!(
            saved.reading(0x3837, 0x6011),
            Some((sample(), at(1_800_000_050)))
        );
    }

    #[test]
    fn update_creates_folder_and_lock_file_and_skips_save_on_error() {
        let root = temp_root("update-create");
        let path = root.join("nested").join("query-log.toml");

        let refused: Result<(), anyhow::Error> = QueryLog::update(&path, |log| {
            log.stamp(0x3837, 0x6011, at(1_800_000_000));
            Err(anyhow::anyhow!("refused"))
        });
        let lock_made = root.join("nested").join("query-log.lock").is_file();
        let saved_after_error = path.exists();
        let ok: Result<(), anyhow::Error> = QueryLog::update(&path, |log| {
            log.stamp(0x3837, 0x6011, at(1_800_000_000));
            Ok(())
        });
        let saved_after_ok = QueryLog::load_from(&path);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(refused.unwrap_err().to_string(), "refused");
        assert!(lock_made, "folder and lock file are created on first use");
        assert!(!saved_after_error);
        ok.unwrap();
        assert_eq!(saved_after_ok.last(0x3837, 0x6011), Some(at(1_800_000_000)));
    }
}
