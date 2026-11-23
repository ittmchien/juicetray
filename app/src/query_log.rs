//! Last battery-query time per device (R21), so the per-family query floor holds across runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use toml::{Table, Value};

use crate::config::{project_dirs, write_atomically};

/// `VID:PID` (upper-case hex) → Unix seconds of the last query sent to that device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryLog {
    last: BTreeMap<String, u64>,
}

/// `query-log.toml` in the OS local data folder; `None` when the OS has no such folder.
pub fn path() -> Option<PathBuf> {
    Some(project_dirs()?.data_local_dir().join("query-log.toml"))
}

fn key(vid: u16, pid: u16) -> String {
    format!("{vid:04X}:{pid:04X}")
}

impl QueryLog {
    /// A missing or unreadable file, and any entry that is not a whole non-negative number,
    /// count as no earlier query.
    pub fn load_from(path: &Path) -> QueryLog {
        let table: Table = fs::read_to_string(path)
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or_default();
        let last = table
            .into_iter()
            .filter_map(|(key, value)| Some((key, u64::try_from(value.as_integer()?).ok()?)))
            .collect();
        QueryLog { last }
    }

    /// `None` also when the stored seconds do not fit a `SystemTime`.
    pub fn last(&self, vid: u16, pid: u16) -> Option<SystemTime> {
        let secs = *self.last.get(&key(vid, pid))?;
        UNIX_EPOCH.checked_add(Duration::from_secs(secs))
    }

    /// Records `at` in whole seconds; a time before 1970 is stored as 0.
    pub fn stamp(&mut self, vid: u16, pid: u16, at: SystemTime) {
        let secs = at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        self.last.insert(key(vid, pid), secs);
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let table: Table = self
            .last
            .iter()
            .map(|(key, &secs)| {
                let secs = i64::try_from(secs).unwrap_or(i64::MAX);
                (key.clone(), Value::Integer(secs))
            })
            .collect();
        write_atomically(path, &table.to_string())
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
        let tmp_left = root.join("nested").join("query-log.toml.tmp").exists();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded, log);
        assert!(!tmp_left);
    }
}
