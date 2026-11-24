use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use toml::{Table, Value};
use ui_model::{DisplayOptions, IconChoice, IconColor};

use crate::interval::INTERVAL_SECONDS;
use crate::log;

/// Auto-refresh quick choices in seconds; any other value in `INTERVAL_SECONDS` is a custom one.
pub const REFRESH_CHOICES: [Option<u32>; 6] =
    [None, Some(60), Some(120), Some(300), Some(600), Some(1800)];
pub const LOW_CHOICES: [Option<u8>; 6] = [None, Some(5), Some(10), Some(15), Some(20), Some(30)];
/// "Hide offline after" choices in minutes; `None` is Never (`0` in the file).
pub const HIDE_OFFLINE_CHOICES: [Option<u32>; 5] = [None, Some(5), Some(15), Some(30), Some(60)];

const DEFAULT_REFRESH_SECONDS: Option<u32> = Some(120);
const DEFAULT_LOW_PERCENT: Option<u8> = Some(15);
const DEFAULT_HIDE_OFFLINE_MINUTES: Option<u32> = Some(15);
const SECONDS_PER_MINUTE: i64 = 60;

const ICON_AUTO: &str = "auto";
const ICON_SVG: &str = "svg";
const COLOR_AUTO: &str = "auto";
const COLOR_ORIGINAL: &str = "original";

/// One device's strip options, stored under its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceConfig {
    /// The device name at the last change, for people reading the file.
    pub name: String,
    pub options: DisplayOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub refresh_seconds: Option<u32>,
    pub low_battery_percent: Option<u8>,
    /// compat: device names hidden by milestone 1; still matched, never added to.
    pub hidden: Vec<String>,
    pub hidden_ids: Vec<String>,
    /// Minutes an offline device stays on the strip before it is dropped; `None` = never.
    pub hide_offline_minutes: Option<u32>,
    /// Keyed by device id; entries of devices not seen this run are kept.
    pub devices: BTreeMap<String, DeviceConfig>,
    /// Hand-edited `VID:PID` entries that let unconfirmed receivers be queried; kept verbatim.
    pub allow: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_seconds: DEFAULT_REFRESH_SECONDS,
            low_battery_percent: DEFAULT_LOW_PERCENT,
            hidden: Vec::new(),
            hidden_ids: Vec::new(),
            hide_offline_minutes: DEFAULT_HIDE_OFFLINE_MINUTES,
            devices: BTreeMap::new(),
            allow: Vec::new(),
        }
    }
}

impl Config {
    /// Reads each key on its own, so one bad field never discards the others.
    pub fn parse(text: &str) -> Self {
        let table: Table = text.parse().unwrap_or_default();
        let defaults = Self::default();
        Self {
            refresh_seconds: refresh_seconds(&table),
            low_battery_percent: choice(
                &table,
                "low_battery_percent",
                &LOW_CHOICES,
                defaults.low_battery_percent,
            ),
            hidden: string_list(&table, "hidden").unwrap_or(defaults.hidden),
            hidden_ids: string_list(&table, "hidden_ids").unwrap_or(defaults.hidden_ids),
            hide_offline_minutes: choice(
                &table,
                "hide_offline_minutes",
                &HIDE_OFFLINE_CHOICES,
                legacy_hide_offline(&table),
            ),
            devices: devices(&table),
            allow: string_items(&table, "allow"),
        }
    }

    /// `0` stands for Off in the file; `refresh_minutes` and `show_disconnected` are never written again.
    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        table.insert(
            "refresh_seconds".into(),
            Value::Integer(self.refresh_seconds.map_or(0, i64::from)),
        );
        table.insert(
            "low_battery_percent".into(),
            Value::Integer(self.low_battery_percent.map_or(0, i64::from)),
        );
        table.insert("hidden".into(), string_array(&self.hidden));
        table.insert("hidden_ids".into(), string_array(&self.hidden_ids));
        table.insert(
            "hide_offline_minutes".into(),
            Value::Integer(self.hide_offline_minutes.map_or(0, i64::from)),
        );
        let devices: Table = self
            .devices
            .iter()
            .map(|(id, device)| (id.clone(), Value::Table(device_table(device))))
            .collect();
        table.insert("devices".into(), Value::Table(devices));
        if !self.allow.is_empty() {
            table.insert("allow".into(), string_array(&self.allow));
        }
        table.to_string()
    }

    /// The `allow` entries that are valid `VID:PID`; others stay in `allow` but are ignored.
    pub fn allowed(&self) -> Vec<(u16, u16)> {
        self.allow
            .iter()
            .filter_map(|entry| crate::query::parse_vid_pid(entry).ok())
            .collect()
    }

    /// Hidden by id, or by name in the milestone-1 list.
    pub fn is_hidden(&self, id: &str, name: &str) -> bool {
        self.hidden_ids.iter().any(|hidden| hidden == id)
            || self.hidden.iter().any(|hidden| hidden == name)
    }

    /// Hides a shown device by id; showing it again clears both its id and its old name entry.
    pub fn toggle_hidden(&mut self, id: &str, name: &str) {
        if self.is_hidden(id, name) {
            self.hidden_ids.retain(|hidden| hidden != id);
            self.hidden.retain(|hidden| hidden != name);
        } else {
            self.hidden_ids.push(id.to_owned());
        }
    }

    pub fn display_options(&self, id: &str) -> DisplayOptions {
        self.devices
            .get(id)
            .map(|device| device.options.clone())
            .unwrap_or_default()
    }

    /// Applies `change` to a device's options, creating its entry on first use; an empty
    /// `name` (device gone since the menu was built) keeps the stored one.
    pub fn update_device(
        &mut self,
        id: &str,
        name: &str,
        change: impl FnOnce(&mut DisplayOptions),
    ) {
        let device = self.devices.entry(id.to_owned()).or_default();
        if !name.is_empty() {
            device.name = name.to_owned();
        }
        change(&mut device.options);
        drop_stray_original(&mut device.options);
    }

    /// Unreadable or missing file means defaults.
    pub fn load() -> Self {
        let _span = log::span("config load");
        path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map_or_else(Self::default, |text| Self::parse(&text))
    }

    pub fn save(&self) -> Result<()> {
        let _span = log::span("config save");
        let path = path().context("no config directory available")?;
        self.save_to(&path)
    }

    /// Adds a device the owner confirmed from the menu to `allow`, in memory and in the file.
    pub fn allow_and_save(&mut self, vid: u16, pid: u16) -> Result<()> {
        let _span = log::span("config allow and save");
        let path = path().context("no config directory available")?;
        self.allow_and_save_to(&path, vid, pid)
    }

    /// The file's `allow` (hand edits win, as in `save_to`) plus this device, written with the
    /// rest of the in-memory config; `save_to` itself would drop the new entry.
    fn allow_and_save_to(&mut self, path: &Path, vid: u16, pid: u16) -> Result<()> {
        if let Ok(text) = fs::read_to_string(path) {
            self.allow = Self::parse(&text).allow;
        }
        if !self.allow_device(vid, pid) {
            return Ok(());
        }
        write_atomically(path, &self.to_toml())
    }

    /// `allow` is owner-edited while the app runs, so the file's value wins over the in-memory one.
    fn save_to(&self, path: &Path) -> Result<()> {
        let mut config = self.clone();
        if let Ok(text) = fs::read_to_string(path) {
            config.allow = Self::parse(&text).allow;
        }
        write_atomically(path, &config.to_toml())
    }

    /// Adds `VID:PID` unless an entry already names the device; other entries stay as they are.
    fn allow_device(&mut self, vid: u16, pid: u16) -> bool {
        if self.allowed().contains(&(vid, pid)) {
            return false;
        }
        self.allow.push(format!("{vid:04X}:{pid:04X}"));
        true
    }
}

/// Writes `<path>.<pid>.tmp` then renames it over `path`, so a crash never leaves a half-written
/// file and two processes never share a temp file. Creates the parent folder when it is missing.
pub(crate) fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().context("path has no parent folder")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))
}

/// The app's OS folders; the one place that names the app's qualifier, organization and name.
pub(crate) fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io.github", "ittmchien", "juicetray")
}

pub fn path() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().join("config.toml"))
}

/// `auto`, `original` or a hex colour `RRGGBB` (optional `#`, any case).
pub fn parse_color(text: &str) -> Option<IconColor> {
    match text {
        COLOR_AUTO => Some(IconColor::Auto),
        COLOR_ORIGINAL => Some(IconColor::Original),
        _ => {
            let hex = text.strip_prefix('#').unwrap_or(text);
            if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            let channel = |start: usize| u8::from_str_radix(&hex[start..start + 2], 16).ok();
            Some(IconColor::Rgb(channel(0)?, channel(2)?, channel(4)?))
        }
    }
}

/// `RRGGBB` in upper case, as menu ids and the file write it.
pub fn hex(red: u8, green: u8, blue: u8) -> String {
    format!("{red:02X}{green:02X}{blue:02X}")
}

/// `Original` only means something for an SVG icon; with a built-in icon it is Auto, per theme.
fn drop_stray_original(options: &mut DisplayOptions) {
    if matches!(options.icon, IconChoice::Svg(_)) {
        return;
    }
    for color in [&mut options.color_light, &mut options.color_dark] {
        if *color == IconColor::Original {
            *color = IconColor::Auto;
        }
    }
}

/// `refresh_seconds` wins; without it the milestone-1 `refresh_minutes` is read. `0` is Off,
/// anything outside 5 s – 24 h gives the default.
fn refresh_seconds(table: &Table) -> Option<u32> {
    let seconds = match (table.get("refresh_seconds"), table.get("refresh_minutes")) {
        (Some(seconds), _) => seconds.as_integer(),
        (None, Some(minutes)) => minutes
            .as_integer()
            .map(|minutes| minutes.saturating_mul(SECONDS_PER_MINUTE)),
        (None, None) => return DEFAULT_REFRESH_SECONDS,
    };
    match seconds {
        Some(0) => None,
        Some(seconds) => u32::try_from(seconds)
            .ok()
            .filter(|seconds| INTERVAL_SECONDS.contains(seconds))
            .or(DEFAULT_REFRESH_SECONDS),
        None => DEFAULT_REFRESH_SECONDS,
    }
}

/// compat: the old `show_disconnected` toggle; true meant "keep them", so Never hide,
/// anything else the default. Only the fallback when `hide_offline_minutes` is missing or invalid.
fn legacy_hide_offline(table: &Table) -> Option<u32> {
    match table.get("show_disconnected").and_then(Value::as_bool) {
        Some(true) => None,
        _ => DEFAULT_HIDE_OFFLINE_MINUTES,
    }
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

/// The string items of an array key; non-strings are skipped, a missing key gives none.
fn string_items(table: &Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn string_array(items: &[String]) -> Value {
    Value::Array(items.iter().cloned().map(Value::String).collect())
}

/// The `[devices]` table; entries that are not tables are dropped.
fn devices(table: &Table) -> BTreeMap<String, DeviceConfig> {
    let Some(devices) = table.get("devices").and_then(Value::as_table) else {
        return BTreeMap::new();
    };
    devices
        .iter()
        .filter_map(|(id, entry)| Some((id.clone(), device_config(entry.as_table()?))))
        .collect()
}

/// A missing or invalid field takes its default: unknown icon or `svg` without a path → Auto.
fn device_config(entry: &Table) -> DeviceConfig {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    let icon = match (text("icon"), text("svg_path")) {
        (Some(ICON_SVG), Some(path)) => IconChoice::Svg(path.to_owned()),
        (Some(name), _) => name.parse().map_or(IconChoice::Auto, IconChoice::Kind),
        (None, _) => IconChoice::Auto,
    };
    let flag = |key: &str| entry.get(key).and_then(Value::as_bool).unwrap_or(true);
    // compat: the single `color` key of older files stands in for whichever theme key is missing.
    let legacy = text("color").and_then(parse_color);
    let theme_color = |key: &str| {
        text(key)
            .and_then(parse_color)
            .or(legacy)
            .unwrap_or(IconColor::Auto)
    };
    let mut options = DisplayOptions {
        show_percent: flag("show_percent"),
        show_label: flag("show_label"),
        icon,
        color_light: theme_color("color_light"),
        color_dark: theme_color("color_dark"),
    };
    drop_stray_original(&mut options);
    DeviceConfig {
        name: text("name").unwrap_or_default().to_owned(),
        options,
    }
}

fn device_table(device: &DeviceConfig) -> Table {
    let options = &device.options;
    let mut entry = Table::new();
    entry.insert("name".into(), Value::String(device.name.clone()));
    entry.insert("show_percent".into(), Value::Boolean(options.show_percent));
    entry.insert("show_label".into(), Value::Boolean(options.show_label));
    let icon = match &options.icon {
        IconChoice::Auto => ICON_AUTO,
        IconChoice::Kind(kind) => kind.as_str(),
        IconChoice::Svg(path) => {
            entry.insert("svg_path".into(), Value::String(path.clone()));
            ICON_SVG
        }
    };
    entry.insert("icon".into(), Value::String(icon.to_owned()));
    // The single `color` key is no longer written; `device_config` still reads it.
    for (key, color) in [
        ("color_light", options.color_light),
        ("color_dark", options.color_dark),
    ] {
        let text = match color {
            IconColor::Auto => COLOR_AUTO.to_owned(),
            IconColor::Original => COLOR_ORIGINAL.to_owned(),
            IconColor::Rgb(red, green, blue) => format!("#{}", hex(red, green, blue)),
        };
        entry.insert(key.into(), Value::String(text));
    }
    entry
}

#[cfg(test)]
mod tests {
    use ui_model::DeviceKind;

    use super::*;

    const MAC_ID: &str = "bt:a4-c3-f0-11-22-33";

    #[test]
    fn defaults_when_empty() {
        assert_eq!(Config::parse(""), Config::default());
    }

    #[test]
    fn roundtrip() {
        let config = Config {
            refresh_seconds: Some(600),
            low_battery_percent: None,
            hidden: vec!["Pixel \"Watch\"".into(), "FIIL Atom".into()],
            hidden_ids: vec![MAC_ID.into()],
            hide_offline_minutes: Some(30),
            devices: BTreeMap::new(),
            allow: vec!["373B:1204".into(), "junk".into()],
        };
        assert_eq!(Config::parse(&config.to_toml()), config);
    }

    #[test]
    fn hide_offline_parse_write_and_compat() {
        let minutes = |text: &str| Config::parse(text).hide_offline_minutes;
        assert_eq!(minutes(""), Some(15));
        assert_eq!(minutes("hide_offline_minutes = 0"), None);
        assert_eq!(minutes("hide_offline_minutes = 60"), Some(60));
        // Not one of the choices, or not a number: default, other fields survive.
        assert_eq!(minutes("hide_offline_minutes = 7"), Some(15));
        assert_eq!(minutes("hide_offline_minutes = \"5\""), Some(15));
        let bad = Config::parse("hide_offline_minutes = \"x\"\nlow_battery_percent = 20\n");
        assert_eq!(bad.low_battery_percent, Some(20));
        // compat: show_disconnected only counts without a valid new key.
        assert_eq!(minutes("show_disconnected = true"), None);
        assert_eq!(minutes("show_disconnected = false"), Some(15));
        assert_eq!(
            minutes("show_disconnected = true\nhide_offline_minutes = 5"),
            Some(5)
        );
        assert_eq!(
            minutes("show_disconnected = true\nhide_offline_minutes = 7"),
            None
        );

        let never = Config {
            hide_offline_minutes: None,
            ..Config::default()
        };
        let written: Table = never.to_toml().parse().unwrap();
        assert_eq!(
            written
                .get("hide_offline_minutes")
                .and_then(Value::as_integer),
            Some(0)
        );
        assert!(!written.contains_key("show_disconnected"));
        assert_eq!(Config::parse(&never.to_toml()), never);
    }

    #[test]
    fn allow_absent_is_empty_and_not_written() {
        let config = Config::parse("");
        assert!(config.allow.is_empty());
        let written: Table = config.to_toml().parse().unwrap();
        assert!(!written.contains_key("allow"));
    }

    #[test]
    fn allow_skips_non_strings_and_bad_entries() {
        let config =
            Config::parse("allow = [\"373b:1204\", 5, \"zzzz:1\", \"373B\", \"0001:ABCD\"]\n");
        assert_eq!(config.allow, ["373b:1204", "zzzz:1", "373B", "0001:ABCD"]);
        assert_eq!(config.allowed(), [(0x373B, 0x1204), (0x0001, 0xABCD)]);
    }

    #[test]
    fn save_keeps_allow_edited_on_disk() {
        let root =
            std::env::temp_dir().join(format!("juicetray-allow-test-{}", std::process::id()));
        let path = root.join("config.toml");
        let config = Config::default();
        config.save_to(&path).unwrap();
        fs::write(&path, "allow = [\"373B:1204\"]\n").unwrap();

        config.save_to(&path).unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.allow, ["373B:1204"]);
    }

    #[test]
    fn allow_device_adds_once_and_keeps_others() {
        let mut config = Config::parse("allow = [\"junk\", \"3837:6011\"]\n");
        assert!(config.allow_device(0x373B, 0x1216));
        assert!(!config.allow_device(0x373B, 0x1216));
        assert_eq!(config.allow, ["junk", "3837:6011", "373B:1216"]);

        // An entry in another case already names the device.
        let mut lower = Config::parse("allow = [\"373b:1216\"]\n");
        assert!(!lower.allow_device(0x373B, 0x1216));
        assert_eq!(lower.allow, ["373b:1216"]);
    }

    #[test]
    fn allow_and_save_keeps_hand_edits_and_other_keys() {
        let root =
            std::env::temp_dir().join(format!("juicetray-allow-save-test-{}", std::process::id()));
        let path = root.join("config.toml");
        let mut config = Config {
            refresh_seconds: Some(300),
            ..Config::default()
        };
        config.save_to(&path).unwrap();
        // Added by hand while the app runs; the in-memory list does not have it.
        fs::write(&path, "allow = [\"373B:1204\"]\nrefresh_seconds = 300\n").unwrap();

        config.allow_and_save_to(&path, 0x373B, 0x1216).unwrap();
        config.allow_and_save_to(&path, 0x373B, 0x1216).unwrap();
        // A later ordinary save keeps the entry.
        config.save_to(&path).unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(config.allow, ["373B:1204", "373B:1216"]);
        assert_eq!(loaded.allow, ["373B:1204", "373B:1216"]);
        assert_eq!(loaded.refresh_seconds, Some(300));
    }

    #[test]
    fn invalid_refresh_falls_back() {
        let config = Config::parse(
            "refresh_minutes = 99999\nlow_battery_percent = 20\nshow_disconnected = true\n",
        );
        assert_eq!(config.refresh_seconds, Some(120));
        assert_eq!(config.low_battery_percent, Some(20));
        // The old toggle still maps: true = never hide.
        assert_eq!(config.hide_offline_minutes, None);
        assert_eq!(
            Config::parse("refresh_minutes = -1").refresh_seconds,
            Some(120)
        );
        assert_eq!(
            Config::parse("refresh_minutes = \"5\"").refresh_seconds,
            Some(120)
        );
        // Custom minute values are valid now.
        assert_eq!(
            Config::parse("refresh_minutes = 7").refresh_seconds,
            Some(420)
        );
    }

    #[test]
    fn config_compat_refresh_minutes() {
        let config = Config::parse("refresh_minutes = 5\n");
        assert_eq!(config.refresh_seconds, Some(300));
        let written: Table = config.to_toml().parse().unwrap();
        assert_eq!(
            written.get("refresh_seconds").and_then(Value::as_integer),
            Some(300)
        );
        assert!(!written.contains_key("refresh_minutes"));
    }

    #[test]
    fn seconds_win_over_minutes() {
        let config = Config::parse("refresh_seconds = 45\nrefresh_minutes = 5\n");
        assert_eq!(config.refresh_seconds, Some(45));
    }

    #[test]
    fn refresh_seconds_3_defaults() {
        assert_eq!(
            Config::parse("refresh_seconds = 3").refresh_seconds,
            Some(120)
        );
        assert_eq!(
            Config::parse("refresh_seconds = 86401").refresh_seconds,
            Some(120)
        );
        assert_eq!(
            Config::parse("refresh_seconds = 5").refresh_seconds,
            Some(5)
        );
        assert_eq!(
            Config::parse("refresh_seconds = 86400").refresh_seconds,
            Some(86_400)
        );
    }

    #[test]
    fn garbage_file_gives_defaults() {
        assert_eq!(Config::parse("\u{0}][ = = not toml {{"), Config::default());
    }

    #[test]
    fn zero_means_off() {
        let config = Config::parse("refresh_minutes = 0\nlow_battery_percent = 0\n");
        assert_eq!(config.refresh_seconds, None);
        assert_eq!(config.low_battery_percent, None);
        assert_eq!(Config::parse("refresh_seconds = 0").refresh_seconds, None);
    }

    #[test]
    fn config_device_roundtrip() {
        let hex_id = "bt:{0123456789abcdef0123456789abcdef}";
        let mut config = Config::default();
        config.update_device(MAC_ID, "FIIL Atom", |options| {
            *options = DisplayOptions {
                show_percent: false,
                show_label: false,
                icon: IconChoice::Svg("C:\\icons\\atom.svg".into()),
                color_light: IconColor::Original,
                color_dark: IconColor::Rgb(0xFF, 0x88, 0x00),
            };
        });
        config.update_device(hex_id, "MX Master", |options| {
            *options = DisplayOptions {
                show_percent: true,
                show_label: true,
                icon: IconChoice::Kind(DeviceKind::Mouse),
                color_light: IconColor::Rgb(0x3E, 0x8E, 0xED),
                color_dark: IconColor::Auto,
            };
        });
        let loaded = Config::parse(&config.to_toml());
        assert_eq!(loaded, config);
        assert_eq!(loaded.devices[MAC_ID].name, "FIIL Atom");
        assert_eq!(
            loaded.display_options(hex_id).color_light,
            IconColor::Rgb(0x3E, 0x8E, 0xED)
        );
        // The single `color` key is never written again.
        let written: Table = config.to_toml().parse().unwrap();
        let entry = written["devices"][MAC_ID].as_table().unwrap();
        assert!(!entry.contains_key("color"));
        assert_eq!(entry["show_label"].as_bool(), Some(false));
    }

    #[test]
    fn legacy_color_applies_to_both_themes() {
        let config = Config::parse(concat!(
            "[devices.\"bt:a\"]\n",
            "color = \"#3E8EED\"\n",
            "[devices.\"bt:b\"]\n",
            "color = \"#3E8EED\"\n",
            "color_dark = \"auto\"\n",
            "[devices.\"bt:c\"]\n",
            "color = \"#3E8EED\"\n",
            "color_light = \"#FF8800\"\n",
            "color_dark = \"#000001\"\n",
        ));
        let blue = IconColor::Rgb(0x3E, 0x8E, 0xED);
        let a = config.display_options("bt:a");
        assert_eq!((a.color_light, a.color_dark), (blue, blue));
        // A present new key wins; the old one fills only the missing theme.
        let b = config.display_options("bt:b");
        assert_eq!((b.color_light, b.color_dark), (blue, IconColor::Auto));
        let c = config.display_options("bt:c");
        assert_eq!(c.color_light, IconColor::Rgb(0xFF, 0x88, 0x00));
        assert_eq!(c.color_dark, IconColor::Rgb(0x00, 0x00, 0x01));
        // Saving migrates to the new keys.
        assert_eq!(Config::parse(&config.to_toml()), config);
        assert!(!config.to_toml().contains("color = "));
    }

    #[test]
    fn show_label_defaults_on() {
        let config = Config::parse(concat!(
            "[devices.\"bt:a\"]\n",
            "show_label = false\n",
            "[devices.\"bt:b\"]\n",
        ));
        assert!(!config.display_options("bt:a").show_label);
        assert!(config.display_options("bt:b").show_label);
        assert!(config.display_options("bt:unknown").show_label);
    }

    #[test]
    fn config_bad_device_fields_default() {
        let config = Config::parse(concat!(
            "[devices.\"bt:a\"]\n",
            "color = \"zzz\"\n",
            "icon = \"svg\"\n",
            "show_percent = \"yes\"\n",
            "[devices.\"bt:b\"]\n",
            "icon = \"mouse\"\n",
            "color = \"original\"\n",
            "[devices.\"bt:c\"]\n",
            "icon = \"toaster\"\n",
            "color = \"#12345\"\n",
        ));
        assert_eq!(config.display_options("bt:a"), DisplayOptions::default());
        assert_eq!(config.devices["bt:a"].name, "");
        // `original` with a built-in icon reads as Auto.
        let b = config.display_options("bt:b");
        assert_eq!(b.icon, IconChoice::Kind(DeviceKind::Mouse));
        assert_eq!(b.color_light, IconColor::Auto);
        assert_eq!(b.color_dark, IconColor::Auto);
        assert_eq!(config.display_options("bt:c"), DisplayOptions::default());
        assert_eq!(
            config.display_options("bt:unknown"),
            DisplayOptions::default()
        );
    }

    #[test]
    fn hidden_by_id_or_old_name() {
        let mut config = Config::parse("hidden = [\"FIIL Atom\"]\n");
        assert!(config.is_hidden(MAC_ID, "FIIL Atom"));
        config.toggle_hidden(MAC_ID, "FIIL Atom");
        assert!(!config.is_hidden(MAC_ID, "FIIL Atom"));
        assert!(config.hidden.is_empty());
        config.toggle_hidden(MAC_ID, "FIIL Atom");
        assert_eq!(config.hidden_ids, [MAC_ID]);
        assert!(config.hidden.is_empty());
    }

    #[test]
    fn switching_to_builtin_icon_drops_original() {
        let mut config = Config::default();
        config.update_device(MAC_ID, "FIIL Atom", |options| {
            options.icon = IconChoice::Svg("a.svg".into());
            options.color_light = IconColor::Original;
            options.color_dark = IconColor::Original;
        });
        config.update_device(MAC_ID, "", |options| options.icon = IconChoice::Auto);
        let options = config.display_options(MAC_ID);
        assert_eq!(options.color_light, IconColor::Auto);
        assert_eq!(options.color_dark, IconColor::Auto);
        assert_eq!(config.devices[MAC_ID].name, "FIIL Atom");
    }

    #[test]
    fn parse_color_forms() {
        assert_eq!(
            parse_color("#3e8eed"),
            Some(IconColor::Rgb(0x3E, 0x8E, 0xED))
        );
        assert_eq!(
            parse_color("FF8800"),
            Some(IconColor::Rgb(0xFF, 0x88, 0x00))
        );
        assert_eq!(parse_color("auto"), Some(IconColor::Auto));
        assert_eq!(parse_color("+12345"), None);
        assert_eq!(parse_color("#GG0000"), None);
        assert_eq!(hex(0x3E, 0x8E, 0xED), "3E8EED");
    }

    #[test]
    fn save_creates_missing_folder() {
        let root =
            std::env::temp_dir().join(format!("juicetray-config-test-{}", std::process::id()));
        let path = root.join("nested").join("config.toml");
        let config = Config {
            refresh_seconds: Some(300),
            ..Config::default()
        };

        config.save_to(&path).unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded, config);
    }
}
