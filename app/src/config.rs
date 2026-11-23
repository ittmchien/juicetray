use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use toml::{Table, Value};

pub const REFRESH_CHOICES: [Option<u32>; 6] = [None, Some(1), Some(2), Some(5), Some(10), Some(30)];
pub const LOW_CHOICES: [Option<u8>; 5] = [None, Some(10), Some(15), Some(20), Some(30)];

const DEFAULT_REFRESH_MINUTES: Option<u32> = Some(2);
const DEFAULT_LOW_PERCENT: Option<u8> = Some(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub refresh_minutes: Option<u32>,
    pub low_battery_percent: Option<u8>,
    pub hidden: Vec<String>,
    pub show_disconnected: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_minutes: DEFAULT_REFRESH_MINUTES,
            low_battery_percent: DEFAULT_LOW_PERCENT,
            hidden: Vec::new(),
            show_disconnected: false,
        }
    }
}

impl Config {
    /// Reads each key on its own, so one bad field never discards the others.
    pub fn parse(text: &str) -> Self {
        let table: Table = text.parse().unwrap_or_default();
        let defaults = Self::default();
        Self {
            refresh_minutes: choice(
                &table,
                "refresh_minutes",
                &REFRESH_CHOICES,
                defaults.refresh_minutes,
            ),
            low_battery_percent: choice(
                &table,
                "low_battery_percent",
                &LOW_CHOICES,
                defaults.low_battery_percent,
            ),
            hidden: string_list(&table, "hidden").unwrap_or(defaults.hidden),
            show_disconnected: table
                .get("show_disconnected")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.show_disconnected),
        }
    }

    /// `0` stands for Off in the file.
    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        table.insert(
            "refresh_minutes".into(),
            Value::Integer(self.refresh_minutes.map_or(0, i64::from)),
        );
        table.insert(
            "low_battery_percent".into(),
            Value::Integer(self.low_battery_percent.map_or(0, i64::from)),
        );
        table.insert(
            "hidden".into(),
            Value::Array(self.hidden.iter().cloned().map(Value::String).collect()),
        );
        table.insert(
            "show_disconnected".into(),
            Value::Boolean(self.show_disconnected),
        );
        table.to_string()
    }

    /// Unreadable or missing file means defaults.
    pub fn load() -> Self {
        path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map_or_else(Self::default, |text| Self::parse(&text))
    }

    pub fn save(&self) -> Result<()> {
        let path = path().context("no config directory available")?;
        self.save_to(&path)
    }

    /// Writes a temp file then renames it, so a crash never leaves a half-written config.
    fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path.parent().context("config path has no parent folder")?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, self.to_toml()).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))
    }
}

pub fn path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("io.github", "ittmchien", "juicetray")?;
    Some(dirs.config_dir().join("config.toml"))
}

/// An integer key matching one of `choices` (`0` = Off); anything else gives `default`.
fn choice<T: Copy + Into<i64>>(
    table: &Table,
    key: &str,
    choices: &[Option<T>],
    default: Option<T>,
) -> Option<T> {
    let Some(number) = table.get(key).and_then(Value::as_integer) else {
        return default;
    };
    choices
        .iter()
        .copied()
        .find(|c| c.map_or(0, Into::into) == number)
        .unwrap_or(default)
}

/// An array of strings; `None` when the key is missing or holds anything else.
fn string_list(table: &Table, key: &str) -> Option<Vec<String>> {
    table
        .get(key)?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        assert_eq!(Config::parse(""), Config::default());
    }

    #[test]
    fn roundtrip() {
        let config = Config {
            refresh_minutes: Some(10),
            low_battery_percent: None,
            hidden: vec!["Pixel \"Watch\"".into(), "FIIL Atom".into()],
            show_disconnected: true,
        };
        assert_eq!(Config::parse(&config.to_toml()), config);
    }

    #[test]
    fn invalid_refresh_falls_back() {
        let config = Config::parse(
            "refresh_minutes = 7\nlow_battery_percent = 20\nshow_disconnected = true\n",
        );
        assert_eq!(config.refresh_minutes, Some(2));
        assert_eq!(config.low_battery_percent, Some(20));
        assert!(config.show_disconnected);
    }

    #[test]
    fn garbage_file_gives_defaults() {
        assert_eq!(Config::parse("\u{0}][ = = not toml {{"), Config::default());
    }

    #[test]
    fn zero_means_off() {
        let config = Config::parse("refresh_minutes = 0\nlow_battery_percent = 0\n");
        assert_eq!(config.refresh_minutes, None);
        assert_eq!(config.low_battery_percent, None);
    }

    #[test]
    fn save_creates_missing_folder() {
        let root =
            std::env::temp_dir().join(format!("juicetray-config-test-{}", std::process::id()));
        let path = root.join("nested").join("config.toml");
        let config = Config {
            refresh_minutes: Some(5),
            ..Config::default()
        };

        config.save_to(&path).unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded, config);
    }
}
