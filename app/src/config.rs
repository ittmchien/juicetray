use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use protocol::{Family, Kind, is_recovery};
use toml::{Table, Value};
use ui_model::{DisplayOptions, IconChoice, IconColor};

use crate::device::Device;
use crate::interval::INTERVAL_SECONDS;
use crate::log;
use crate::query::parse_vid_pid;

/// Auto-refresh quick choices in seconds; any other value in `INTERVAL_SECONDS` is a custom one.
pub const REFRESH_CHOICES: [Option<u32>; 6] =
    [None, Some(60), Some(120), Some(300), Some(600), Some(1800)];
/// Low-battery quick choices in percent; any other value in `LOW_PERCENT` is a custom one.
pub const LOW_CHOICES: [Option<u8>; 6] = [None, Some(5), Some(10), Some(15), Some(20), Some(30)];
/// "Hide offline after" choices in minutes; `None` is Never (`0` in the file). Any other value
/// in `HIDE_OFFLINE_MINUTES` is a custom one.
pub const HIDE_OFFLINE_CHOICES: [Option<u32>; 5] = [None, Some(5), Some(15), Some(30), Some(60)];

/// Allowed low-battery thresholds in percent.
pub const LOW_PERCENT: RangeInclusive<u8> = 1..=99;
/// Allowed "Hide offline after" values in minutes: 1 min to 24 h.
pub const HIDE_OFFLINE_MINUTES: RangeInclusive<u32> = 1..=1440;

const DEFAULT_REFRESH_SECONDS: Option<u32> = Some(300);
const DEFAULT_LOW_PERCENT: Option<u8> = Some(15);
const DEFAULT_HIDE_OFFLINE_MINUTES: Option<u32> = Some(15);
const SECONDS_PER_MINUTE: i64 = 60;

const ICON_AUTO: &str = "auto";
const ICON_SVG: &str = "svg";
const ICON_SYMBOL: &str = "symbol";
const COLOR_AUTO: &str = "auto";
const COLOR_ORIGINAL: &str = "original";

/// One device's strip options, stored under its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceConfig {
    /// The device name at the last change, for people reading the file.
    pub name: String,
    pub options: DisplayOptions,
}

/// One `custom_devices` entry as the file holds it (missing text fields are empty); kept
/// verbatim so a typo is never lost, and checked by `Config::custom`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomEntry {
    /// `VID:PID`.
    pub id: String,
    pub name: String,
    /// A `protocol::Family` variant name, as in the catalogue files.
    pub family: String,
    /// `mouse`, `headset`, `keyboard`, `gamepad` or `other`.
    pub kind: String,
    /// `receiver` or `cable`.
    pub connection: String,
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
    /// `VID:PID` entries of unverified catalogue models the owner added, so the worker queries
    /// them; hand-editable, kept verbatim.
    pub allow: Vec<String>,
    /// `VID:PID` entries the new-device popup asked about, whatever the answer (the key keeps its
    /// old name so existing files still work); never offered again, kept verbatim.
    pub declined: Vec<String>,
    /// Devices outside the catalogue the owner added with a family; kept verbatim.
    pub custom_devices: Vec<CustomEntry>,
    /// Offer unverified and unknown family devices in the new-device popup (Windows); on by
    /// default. Off leaves plug-in and wake refreshes of queried devices as they are.
    pub detect_new_devices: bool,
    /// Keep auto-refreshing while a laptop runs on battery; off by default.
    pub refresh_on_battery: bool,
    /// Ask GitHub Releases for a newer version once a day (Windows); on by default.
    pub check_updates: bool,
    /// The newest version already announced, so one release is never announced twice.
    pub notified_update: Option<String>,
    /// Offer to move a copy running outside Applications there (macOS); off once the owner
    /// declined with "Not now".
    pub ask_move_to_applications: bool,
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
            declined: Vec::new(),
            custom_devices: Vec::new(),
            detect_new_devices: true,
            refresh_on_battery: false,
            check_updates: true,
            notified_update: None,
            ask_move_to_applications: true,
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
            low_battery_percent: ranged(
                &table,
                "low_battery_percent",
                &LOW_PERCENT,
                defaults.low_battery_percent,
            ),
            hidden: string_list(&table, "hidden").unwrap_or(defaults.hidden),
            hidden_ids: string_list(&table, "hidden_ids").unwrap_or(defaults.hidden_ids),
            hide_offline_minutes: ranged(
                &table,
                "hide_offline_minutes",
                &HIDE_OFFLINE_MINUTES,
                legacy_hide_offline(&table),
            ),
            devices: devices(&table),
            allow: string_items(&table, "allow"),
            declined: string_items(&table, "declined"),
            custom_devices: custom_entries(&table),
            detect_new_devices: table
                .get("detect_new_devices")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.detect_new_devices),
            refresh_on_battery: table
                .get("refresh_on_battery")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.refresh_on_battery),
            check_updates: table
                .get("check_updates")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.check_updates),
            notified_update: table
                .get("notified_update")
                .and_then(Value::as_str)
                .filter(|version| !version.is_empty())
                .map(str::to_owned),
            ask_move_to_applications: table
                .get("ask_move_to_applications")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.ask_move_to_applications),
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
        table.insert(
            "refresh_on_battery".into(),
            Value::Boolean(self.refresh_on_battery),
        );
        table.insert("check_updates".into(), Value::Boolean(self.check_updates));
        table.insert(
            "ask_move_to_applications".into(),
            Value::Boolean(self.ask_move_to_applications),
        );
        table.insert(
            "detect_new_devices".into(),
            Value::Boolean(self.detect_new_devices),
        );
        if let Some(version) = &self.notified_update {
            table.insert("notified_update".into(), Value::String(version.clone()));
        }
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
        if !self.declined.is_empty() {
            table.insert("declined".into(), string_array(&self.declined));
        }
        if !self.custom_devices.is_empty() {
            let entries = self.custom_devices.iter().map(custom_table).collect();
            table.insert("custom_devices".into(), Value::Array(entries));
        }
        table.to_string()
    }

    /// The ids the worker may query unverified: valid `allow` entries plus valid custom device
    /// ids (so one stays read if a later catalogue lists it unverified). Invalid ones are skipped.
    pub fn allowed(&self) -> Vec<(u16, u16)> {
        let mut ids = valid_ids(&self.allow);
        for device in self
            .custom_devices
            .iter()
            .filter_map(|entry| custom_device(entry).ok())
        {
            let id = (device.vid, device.pid);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    /// The `declined` entries that are valid `VID:PID`; others stay in the list but are ignored.
    #[cfg_attr(
        not(windows),
        allow(dead_code, reason = "only Windows offers new devices")
    )]
    pub fn declined_ids(&self) -> Vec<(u16, u16)> {
        valid_ids(&self.declined)
    }

    /// The valid `custom_devices` entries; an invalid one stays in the file but is ignored,
    /// with a log line naming why.
    pub fn custom(&self) -> Vec<Device> {
        self.custom_devices
            .iter()
            .filter_map(|entry| {
                custom_device(entry)
                    .map_err(|why| {
                        log::line(&format!(
                            "config: custom device {:?} ignored: {why}",
                            entry.id
                        ));
                    })
                    .ok()
            })
            .collect()
    }

    /// Takes the owner-edited device keys (`allow`, `declined`, `custom_devices`) from `file`,
    /// the config as last read from disk, so hand edits made while the app runs win.
    pub fn take_device_lists(&mut self, file: Config) {
        self.allow = file.allow;
        self.declined = file.declined;
        self.custom_devices = file.custom_devices;
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

    /// Adds a catalogue model the owner added in the Add window to `allow`, in memory and in the
    /// file.
    #[cfg_attr(
        not(windows),
        allow(dead_code, reason = "only Windows has the Add window")
    )]
    pub fn allow_and_save(&mut self, vid: u16, pid: u16) -> Result<()> {
        let _span = log::span("config allow and save");
        let path = path().context("no config directory available")?;
        self.add_id_and_save_to(&path, |config| &mut config.allow, vid, pid)
    }

    /// Adds a custom device (Add window or popup catalogue row) in memory and in the file;
    /// an entry with the same id is replaced.
    #[cfg_attr(
        not(windows),
        allow(dead_code, reason = "only Windows has the Add window")
    )]
    pub fn add_custom_and_save(&mut self, entry: CustomEntry) -> Result<()> {
        let _span = log::span("config add custom device and save");
        let path = path().context("no config directory available")?;
        self.add_custom_and_save_to(&path, entry)
    }

    /// The file's device keys (hand edits win) with `entry` in place of any entry naming its id.
    fn add_custom_and_save_to(&mut self, path: &Path, entry: CustomEntry) -> Result<()> {
        self.take_file_device_lists(path)?;
        let id = parse_vid_pid(&entry.id).ok();
        self.custom_devices
            .retain(|old| id.is_none() || parse_vid_pid(&old.id).ok() != id);
        self.custom_devices.push(entry);
        write_atomically(path, &self.to_toml())
    }

    /// Adds a device the owner was asked about in the new-device popup (whatever the answer) to
    /// `declined`, in memory and in the file, so it is never offered again.
    #[cfg_attr(
        not(windows),
        allow(dead_code, reason = "only Windows offers new devices")
    )]
    pub fn asked_and_save(&mut self, vid: u16, pid: u16) -> Result<()> {
        let _span = log::span("config asked and save");
        let path = path().context("no config directory available")?;
        self.add_id_and_save_to(&path, |config| &mut config.declined, vid, pid)
    }

    /// The file's device keys (hand edits win, as in `save_to`) plus this device in the `list`
    /// key, written with the rest of the in-memory config; `save_to` itself would drop the new
    /// entry.
    fn add_id_and_save_to(
        &mut self,
        path: &Path,
        list: fn(&mut Self) -> &mut Vec<String>,
        vid: u16,
        pid: u16,
    ) -> Result<()> {
        self.take_file_device_lists(path)?;
        if !add_id(list(self), vid, pid) {
            return Ok(());
        }
        write_atomically(path, &self.to_toml())
    }

    /// The device keys are owner-edited while the app runs, so the file's values win over the
    /// in-memory ones.
    fn save_to(&self, path: &Path) -> Result<()> {
        let mut config = self.clone();
        config.take_file_device_lists(path)?;
        write_atomically(path, &config.to_toml())
    }

    /// Takes the device keys of the file at `path` (none when it is missing). A file that is not
    /// valid TOML is an error: parsing it would give empty lists, and the write would erase them.
    fn take_file_device_lists(&mut self, path: &Path) -> Result<()> {
        // Only a missing file has no lists; an unreadable one (e.g. UTF-16) would be erased too.
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("{} could not be read; nothing was saved", path.display())
                });
            }
        };
        text.parse::<Table>().with_context(|| {
            format!(
                "{} is not valid TOML; fix it first, nothing was saved",
                path.display()
            )
        })?;
        self.take_device_lists(Self::parse(&text));
        Ok(())
    }
}

/// The entries of an id list that are valid `VID:PID`.
fn valid_ids(entries: &[String]) -> Vec<(u16, u16)> {
    entries
        .iter()
        .filter_map(|entry| parse_vid_pid(entry).ok())
        .collect()
}

/// Adds `VID:PID` unless an entry already names the device; other entries stay as they are.
fn add_id(entries: &mut Vec<String>, vid: u16, pid: u16) -> bool {
    if valid_ids(entries).contains(&(vid, pid)) {
        return false;
    }
    entries.push(format!("{vid:04X}:{pid:04X}"));
    true
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

/// An integer key inside `range` (`0` = Off); anything else gives `default`.
fn ranged<T: TryFrom<i64> + PartialOrd>(
    table: &Table,
    key: &str,
    range: &RangeInclusive<T>,
    default: Option<T>,
) -> Option<T> {
    let Some(number) = table.get(key).and_then(Value::as_integer) else {
        return default;
    };
    if number == 0 {
        return None;
    }
    T::try_from(number)
        .ok()
        .filter(|value| range.contains(value))
        .or(default)
}

/// A whole number inside `range`, or a message to show the owner.
fn parse_in_range<T: FromStr + PartialOrd + Display>(
    input: &str,
    range: &RangeInclusive<T>,
) -> Result<T, String> {
    input
        .trim()
        .parse()
        .ok()
        .filter(|value| range.contains(value))
        .ok_or_else(|| {
            format!(
                "Type a whole number from {} to {}.",
                range.start(),
                range.end()
            )
        })
}

/// A typed low-battery threshold: 1-99, with or without a trailing `%`.
pub fn parse_low_percent(input: &str) -> Result<u8, String> {
    parse_in_range(input.trim().trim_end_matches('%'), &LOW_PERCENT)
}

/// Typed minutes before an offline device leaves the strip: 1-1440.
pub fn parse_hide_offline_minutes(input: &str) -> Result<u32, String> {
    parse_in_range(input, &HIDE_OFFLINE_MINUTES)
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

/// The `custom_devices` array; items that are not tables are dropped, text fields kept as is.
fn custom_entries(table: &Table) -> Vec<CustomEntry> {
    let Some(items) = table.get("custom_devices").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_table)
        .map(|entry| {
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            CustomEntry {
                id: text("id"),
                name: text("name"),
                family: text("family"),
                kind: text("kind"),
                connection: text("connection"),
            }
        })
        .collect()
}

/// An entry's non-empty text fields, so a missing field stays missing in the file.
fn custom_table(entry: &CustomEntry) -> Value {
    let fields = [
        ("id", &entry.id),
        ("name", &entry.name),
        ("family", &entry.family),
        ("kind", &entry.kind),
        ("connection", &entry.connection),
    ];
    Value::Table(
        fields
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(key, text)| (key.to_owned(), Value::String(text.clone())))
            .collect(),
    )
}

/// A checked custom device, or why the entry is ignored. ATK serial, RongYuan F7 and Driveall are
/// refused because only catalogue PIDs tell them apart, Generic because its frame lives in the
/// catalogue row; an empty name shows the id.
pub(crate) fn custom_device(entry: &CustomEntry) -> Result<Device, String> {
    let (vid, pid) = parse_vid_pid(&entry.id).map_err(|_| "id is not VID:PID".to_owned())?;
    if is_recovery(vid, pid) {
        return Err("recovery or bootloader id".to_owned());
    }
    let family = match entry.family.as_str() {
        "Compx" => Family::Compx,
        "Report13" => Family::Report13,
        "Mchose" => Family::Mchose,
        "AtkBitmouse" => Family::AtkBitmouse,
        "VgnKc2" => Family::VgnKc2,
        // These are told apart from look-alike protocols only by catalogue PIDs.
        "AtkSerial" => return Err("ATK serial works only for catalogue models".to_owned()),
        "RongYuanF7" => return Err("RongYuan F7 works only for catalogue models".to_owned()),
        "Driveall" => return Err("Driveall works only for catalogue models".to_owned()),
        "Generic" => return Err("Generic works only for catalogue models".to_owned()),
        other => return Err(format!("unknown family {other:?}")),
    };
    let kind = match entry.kind.as_str() {
        "mouse" => Kind::Mouse,
        "headset" => Kind::Headset,
        "keyboard" => Kind::Keyboard,
        "gamepad" => Kind::Gamepad,
        "other" => Kind::Other,
        other => return Err(format!("unknown kind {other:?}")),
    };
    let cable = match entry.connection.as_str() {
        "receiver" => false,
        "cable" => true,
        other => return Err(format!("unknown connection {other:?}")),
    };
    let name = if entry.name.is_empty() {
        format!("{vid:04X}:{pid:04X}")
    } else {
        entry.name.clone()
    };
    Ok(Device {
        vid,
        pid,
        name,
        family,
        kind,
        verified: false,
        cable,
        needs_capture: false,
        custom: true,
    })
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

/// A missing or invalid field takes its default: unknown icon, `svg` without a path or `symbol`
/// without a name → Auto.
fn device_config(entry: &Table) -> DeviceConfig {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    let icon = match (text("icon"), text("svg_path")) {
        (Some(ICON_SVG), Some(path)) => IconChoice::Svg(path.to_owned()),
        (Some(ICON_SYMBOL), _) => text("symbol_name")
            .filter(|name| !name.is_empty())
            .map_or(IconChoice::Auto, |name| IconChoice::Symbol(name.to_owned())),
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
        IconChoice::Symbol(name) => {
            entry.insert("symbol_name".into(), Value::String(name.clone()));
            ICON_SYMBOL
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
            declined: vec!["3554:FA09".into()],
            custom_devices: vec![CustomEntry {
                id: "1234:5678".into(),
                name: "My \"mouse\"".into(),
                family: "Compx".into(),
                kind: "mouse".into(),
                connection: "receiver".into(),
            }],
            detect_new_devices: false,
            refresh_on_battery: true,
            check_updates: false,
            notified_update: Some("0.2.0".into()),
            ask_move_to_applications: false,
        };
        assert_eq!(Config::parse(&config.to_toml()), config);
    }

    #[test]
    fn detect_new_devices_defaults_on_and_round_trips() {
        // Old files, and a wrongly typed value, keep detection on.
        for text in ["refresh_seconds = 300", "detect_new_devices = \"no\""] {
            assert!(Config::parse(text).detect_new_devices, "{text}");
        }
        assert!(
            Config::default()
                .to_toml()
                .contains("detect_new_devices = true")
        );
        let off = Config::parse("detect_new_devices = false");
        assert!(!off.detect_new_devices);
        assert!(off.to_toml().contains("detect_new_devices = false"));
        assert_eq!(Config::parse(&off.to_toml()), off);
    }

    #[test]
    fn update_keys_default_for_old_files_and_round_trip() {
        // An old file without the keys, or with wrongly typed ones, keeps the defaults.
        for text in [
            "refresh_seconds = 300",
            "check_updates = \"no\"\nnotified_update = 3",
        ] {
            let config = Config::parse(text);
            assert!(config.check_updates, "{text}");
            assert_eq!(config.notified_update, None, "{text}");
        }
        assert!(!Config::parse("check_updates = false").check_updates);
        // The move prompt defaults on and only an explicit false turns it off.
        assert!(Config::default().ask_move_to_applications);
        assert!(Config::parse("ask_move_to_applications = \"no\"").ask_move_to_applications);
        assert!(!Config::parse("ask_move_to_applications = false").ask_move_to_applications);
        assert_eq!(
            Config::parse("notified_update = \"\"").notified_update,
            None
        );

        // Nothing announced yet writes no `notified_update` line.
        let written = Config::default().to_toml();
        assert!(written.contains("check_updates = true"));
        assert!(!written.contains("notified_update"));
        let announced = Config {
            notified_update: Some("0.3.1".into()),
            ..Config::default()
        };
        assert!(announced.to_toml().contains("notified_update = \"0.3.1\""));
        assert_eq!(Config::parse(&announced.to_toml()), announced);
    }

    #[test]
    fn refresh_on_battery_defaults_off_and_round_trips() {
        let flag = |text: &str| Config::parse(text).refresh_on_battery;
        // An old file without the key, or a wrongly typed value, keeps the default.
        assert!(!flag("refresh_seconds = 300"));
        assert!(!flag("refresh_on_battery = \"yes\""));
        assert!(flag("refresh_on_battery = true"));

        let on = Config {
            refresh_on_battery: true,
            ..Config::default()
        };
        assert!(on.to_toml().contains("refresh_on_battery = true"));
        assert!(Config::parse(&on.to_toml()).refresh_on_battery);
        assert!(!Config::parse(&Config::default().to_toml()).refresh_on_battery);
    }

    #[test]
    fn hide_offline_parse_write_and_compat() {
        let minutes = |text: &str| Config::parse(text).hide_offline_minutes;
        assert_eq!(minutes(""), Some(15));
        assert_eq!(minutes("hide_offline_minutes = 0"), None);
        assert_eq!(minutes("hide_offline_minutes = 60"), Some(60));
        // A custom value in range is kept; outside it, or not a number: default.
        assert_eq!(minutes("hide_offline_minutes = 7"), Some(7));
        assert_eq!(minutes("hide_offline_minutes = 1"), Some(1));
        assert_eq!(minutes("hide_offline_minutes = 1440"), Some(1440));
        assert_eq!(minutes("hide_offline_minutes = 1441"), Some(15));
        assert_eq!(minutes("hide_offline_minutes = -5"), Some(15));
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
            Some(7)
        );
        assert_eq!(
            minutes("show_disconnected = true\nhide_offline_minutes = 1441"),
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
    fn custom_low_and_offline_values_roundtrip() {
        let config = Config {
            low_battery_percent: Some(7),
            hide_offline_minutes: Some(90),
            ..Config::default()
        };
        let written: Table = config.to_toml().parse().unwrap();
        assert_eq!(
            written
                .get("low_battery_percent")
                .and_then(Value::as_integer),
            Some(7)
        );
        assert_eq!(Config::parse(&config.to_toml()), config);
    }

    #[test]
    fn low_battery_range_and_fallback() {
        let low = |text: &str| Config::parse(text).low_battery_percent;
        assert_eq!(low("low_battery_percent = 1"), Some(1));
        assert_eq!(low("low_battery_percent = 99"), Some(99));
        assert_eq!(low("low_battery_percent = 0"), None);
        assert_eq!(low("low_battery_percent = 100"), DEFAULT_LOW_PERCENT);
        assert_eq!(low("low_battery_percent = -1"), DEFAULT_LOW_PERCENT);
        assert_eq!(low("low_battery_percent = 300"), DEFAULT_LOW_PERCENT);
        assert_eq!(low("low_battery_percent = \"20\""), DEFAULT_LOW_PERCENT);
    }

    #[test]
    fn typed_low_and_offline_values() {
        assert_eq!(parse_low_percent("7"), Ok(7));
        assert_eq!(parse_low_percent(" 25% "), Ok(25));
        assert_eq!(parse_low_percent("99"), Ok(99));
        for bad in ["", "0", "100", "-5", "abc", "1.5", "256", "%"] {
            assert!(parse_low_percent(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_hide_offline_minutes("1"), Ok(1));
        assert_eq!(parse_hide_offline_minutes(" 90 "), Ok(90));
        assert_eq!(parse_hide_offline_minutes("1440"), Ok(1440));
        for bad in ["", "0", "1441", "-1", "2h", "1.5", "99999999999999999999"] {
            assert!(parse_hide_offline_minutes(bad).is_err(), "{bad:?}");
        }
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
    fn allowed_adds_valid_custom_ids_once() {
        // A custom device whose id a later catalogue lists unverified still needs the query gate.
        let config = Config::parse(
            r#"
allow = ["373B:1204"]

[[custom_devices]]
id = "373b:1204"
family = "Compx"
kind = "mouse"
connection = "cable"

[[custom_devices]]
id = "3554:7777"
family = "Report13"
kind = "keyboard"
connection = "receiver"

[[custom_devices]]
id = "3554:8888"
family = "AtkSerial"
kind = "mouse"
connection = "cable"
"#,
        );
        // The duplicate and the invalid entry add nothing; the file's lists stay as they were.
        assert_eq!(config.allowed(), [(0x373B, 0x1204), (0x3554, 0x7777)]);
        assert_eq!(config.allow, ["373B:1204"]);
        assert_eq!(config.custom_devices.len(), 3);
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
    fn saves_refuse_to_overwrite_a_file_that_is_not_toml() {
        let root =
            std::env::temp_dir().join(format!("juicetray-broken-test-{}", std::process::id()));
        let path = root.join("config.toml");
        fs::create_dir_all(&root).unwrap();
        // A hand edit left the file broken; its lists must survive every save path.
        let broken = "allow = [\"373B:1204\"\ndeclined = [\"3554:FA09\"]\n";
        fs::write(&path, broken).unwrap();
        let mut config = Config::default();
        let entry = CustomEntry {
            id: "3554:7777".into(),
            name: "My board".into(),
            family: "Compx".into(),
            kind: "keyboard".into(),
            connection: "receiver".into(),
        };

        let saved = config.save_to(&path);
        let allowed = config.add_id_and_save_to(&path, |config| &mut config.allow, 0x373B, 0x1216);
        let custom = config.add_custom_and_save_to(&path, entry);
        let on_disk = fs::read_to_string(&path).unwrap();
        fs::remove_dir_all(&root).unwrap();

        for result in [saved, allowed, custom] {
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("not valid TOML"), "{error}");
        }
        assert_eq!(on_disk, broken);
    }

    #[test]
    fn add_id_adds_once_and_keeps_others() {
        let mut config = Config::parse("allow = [\"junk\", \"3837:6011\"]\n");
        assert!(add_id(&mut config.allow, 0x373B, 0x1216));
        assert!(!add_id(&mut config.allow, 0x373B, 0x1216));
        assert_eq!(config.allow, ["junk", "3837:6011", "373B:1216"]);

        // An entry in another case already names the device.
        let mut lower = Config::parse("allow = [\"373b:1216\"]\n");
        assert!(!add_id(&mut lower.allow, 0x373B, 0x1216));
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

        let allow: fn(&mut Config) -> &mut Vec<String> = |config| &mut config.allow;
        config
            .add_id_and_save_to(&path, allow, 0x373B, 0x1216)
            .unwrap();
        config
            .add_id_and_save_to(&path, allow, 0x373B, 0x1216)
            .unwrap();
        // A later ordinary save keeps the entry.
        config.save_to(&path).unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(config.allow, ["373B:1204", "373B:1216"]);
        assert_eq!(loaded.allow, ["373B:1204", "373B:1216"]);
        assert_eq!(loaded.refresh_seconds, Some(300));
    }

    #[test]
    fn asked_ids_are_added_once_and_keep_hand_edits() {
        let root = std::env::temp_dir().join(format!(
            "juicetray-decline-save-test-{}",
            std::process::id()
        ));
        let path = root.join("config.toml");
        let mut config = Config::default();
        config.save_to(&path).unwrap();
        // Declined by hand in another casing while the app runs.
        fs::write(
            &path,
            "declined = [\"391d:2001\"]
allow = [\"373B:1204\"]
",
        )
        .unwrap();

        let declined: fn(&mut Config) -> &mut Vec<String> = |config| &mut config.declined;
        config
            .add_id_and_save_to(&path, declined, 0x3554, 0xFA09)
            .unwrap();
        config
            .add_id_and_save_to(&path, declined, 0x391D, 0x2001)
            .unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.declined, ["391d:2001", "3554:FA09"]);
        assert_eq!(loaded.declined_ids(), [(0x391D, 0x2001), (0x3554, 0xFA09)]);
        assert_eq!(loaded.allow, ["373B:1204"]);
    }

    #[test]
    fn add_custom_and_save_replaces_its_id_and_keeps_hand_edits() {
        let root =
            std::env::temp_dir().join(format!("juicetray-custom-save-test-{}", std::process::id()));
        let path = root.join("config.toml");
        let mut config = Config::default();
        config.save_to(&path).unwrap();
        // Written by hand while the app runs, with an older entry for the same id.
        fs::write(
            &path,
            "allow = [\"373B:1204\"]\n\n[[custom_devices]]\nid = \"3554:7777\"\nfamily = \"Report13\"\n\n[[custom_devices]]\nid = \"1234:5678\"\nfamily = \"Compx\"\nkind = \"mouse\"\nconnection = \"cable\"\n",
        )
        .unwrap();
        let entry = CustomEntry {
            id: "3554:7777".into(),
            name: "My board".into(),
            family: "Compx".into(),
            kind: "keyboard".into(),
            connection: "receiver".into(),
        };
        config.add_custom_and_save_to(&path, entry.clone()).unwrap();
        let loaded = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(loaded.allow, ["373B:1204"]);
        assert_eq!(loaded.custom_devices.len(), 2);
        assert_eq!(loaded.custom_devices[0].id, "1234:5678");
        assert_eq!(loaded.custom_devices[1], entry);
        assert_eq!(config.custom_devices, loaded.custom_devices);
    }

    #[test]
    fn device_lists_absent_are_empty_and_not_written() {
        // An old config without the keys loads unchanged and writes none of them.
        let config = Config::parse("refresh_seconds = 300\nallow = [\"373B:1204\"]\n");
        assert!(config.declined.is_empty() && config.custom_devices.is_empty());
        let written: Table = config.to_toml().parse().unwrap();
        assert!(!written.contains_key("declined"));
        assert!(!written.contains_key("custom_devices"));
        assert_eq!(Config::parse(&config.to_toml()), config);
    }

    const CUSTOM: &str = concat!(
        "declined = [\"3554:FA09\", 5]\n",
        "[[custom_devices]]\n",
        "id = \"1234:5678\"\n",
        "name = \"My mouse\"\n",
        "family = \"Compx\"\n",
        "kind = \"mouse\"\n",
        "connection = \"receiver\"\n",
        "[[custom_devices]]\n",
        "id = \"1234:0001\"\n",
        "family = \"Compx2\"\n",
        "kind = \"mouse\"\n",
        "connection = \"receiver\"\n",
    );

    #[test]
    fn declined_and_custom_devices_parse_and_keep_bad_entries() {
        let config = Config::parse(CUSTOM);
        assert_eq!(config.declined, ["3554:FA09"]);
        assert_eq!(config.custom_devices.len(), 2);
        // The typo stays in the file; only the valid entry is used.
        assert_eq!(config.custom_devices[1].family, "Compx2");
        assert_eq!(config.custom_devices[1].name, "");
        let custom = config.custom();
        assert_eq!(custom.len(), 1);
        let mine = &custom[0];
        assert_eq!((mine.vid, mine.pid), (0x1234, 0x5678));
        assert_eq!(mine.name, "My mouse");
        assert_eq!((mine.family, mine.kind), (Family::Compx, Kind::Mouse));
        assert!(mine.custom && !mine.cable && !mine.verified && !mine.needs_capture);

        // A missing field is not written back as an empty one.
        let written = config.to_toml();
        assert_eq!(written.matches("name = ").count(), 1, "{written}");
        assert_eq!(Config::parse(&written), config);
    }

    #[test]
    fn custom_entry_validation() {
        let entry = |id: &str, family: &str, kind: &str, connection: &str| CustomEntry {
            id: id.into(),
            name: String::new(),
            family: family.into(),
            kind: kind.into(),
            connection: connection.into(),
        };
        let good = custom_device(&entry("373b:2000", "AtkBitmouse", "headset", "cable")).unwrap();
        assert_eq!(good.name, "373B:2000");
        assert_eq!(
            (good.family, good.kind),
            (Family::AtkBitmouse, Kind::Headset)
        );
        assert!(good.cable && good.custom);
        for family in ["Compx", "Report13", "Mchose", "AtkBitmouse", "VgnKc2"] {
            assert!(custom_device(&entry("0001:0002", family, "other", "receiver")).is_ok());
        }
        let bad = [
            entry("nope", "Compx", "mouse", "receiver"),
            entry("3554:F2FF", "Compx", "mouse", "receiver"),
            entry("1:2", "compx", "mouse", "receiver"),
            entry("1:2", "AtkSerial", "headset", "receiver"),
            entry("1:2", "RongYuanF7", "keyboard", "receiver"),
            entry("1:2", "Driveall", "keyboard", "receiver"),
            entry("1:2", "Generic", "keyboard", "receiver"),
            entry("1:2", "Compx", "Mouse", "receiver"),
            entry("1:2", "Compx", "mouse", "dongle"),
            entry("1:2", "", "mouse", "receiver"),
        ];
        for item in bad {
            assert!(custom_device(&item).is_err(), "{item:?}");
        }
    }

    #[test]
    fn save_keeps_device_lists_edited_on_disk() {
        let root =
            std::env::temp_dir().join(format!("juicetray-lists-test-{}", std::process::id()));
        let path = root.join("config.toml");
        let mut config = Config::default();
        config.save_to(&path).unwrap();
        fs::write(&path, CUSTOM).unwrap();

        config.save_to(&path).unwrap();
        let saved = Config::parse(&fs::read_to_string(&path).unwrap());
        // Adding to `allow` keeps the file's other device keys too.
        config
            .add_id_and_save_to(&path, |config| &mut config.allow, 0x373B, 0x1216)
            .unwrap();
        let added = Config::parse(&fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(&root).unwrap();

        let file = Config::parse(CUSTOM);
        assert_eq!(saved.declined, file.declined);
        assert_eq!(saved.custom_devices, file.custom_devices);
        assert_eq!(added.declined, file.declined);
        assert_eq!(added.custom_devices, file.custom_devices);
        assert_eq!(added.allow, ["373B:1216"]);
        assert_eq!(config.custom_devices, file.custom_devices);
    }

    #[test]
    fn invalid_refresh_falls_back() {
        let config = Config::parse(
            "refresh_minutes = 99999\nlow_battery_percent = 20\nshow_disconnected = true\n",
        );
        assert_eq!(config.refresh_seconds, DEFAULT_REFRESH_SECONDS);
        assert_eq!(config.low_battery_percent, Some(20));
        // The old toggle still maps: true = never hide.
        assert_eq!(config.hide_offline_minutes, None);
        assert_eq!(
            Config::parse("refresh_minutes = -1").refresh_seconds,
            DEFAULT_REFRESH_SECONDS
        );
        assert_eq!(
            Config::parse("refresh_minutes = \"5\"").refresh_seconds,
            DEFAULT_REFRESH_SECONDS
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
            DEFAULT_REFRESH_SECONDS
        );
        assert_eq!(
            Config::parse("refresh_seconds = 86401").refresh_seconds,
            DEFAULT_REFRESH_SECONDS
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
    fn symbol_icon_roundtrip() {
        let mut config = Config::default();
        config.update_device(MAC_ID, "AirPods Pro", |options| {
            options.icon = IconChoice::Symbol("airpods.pro".into());
        });
        let loaded = Config::parse(&config.to_toml());
        assert_eq!(loaded, config);
        assert_eq!(
            loaded.display_options(MAC_ID).icon,
            IconChoice::Symbol("airpods.pro".into())
        );
        // A symbol entry without a name loads as Auto.
        let nameless = Config::parse(&format!("[devices.\"{MAC_ID}\"]\nicon = \"symbol\"\n"));
        assert_eq!(nameless.display_options(MAC_ID).icon, IconChoice::Auto);
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
