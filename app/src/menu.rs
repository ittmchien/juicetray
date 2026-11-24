use std::fmt::Display;
use std::str::FromStr;
use std::time::SystemTime;

use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use ui_model::{
    DeviceKind, DeviceView, EMPTY_MENU_TEXT, IconChoice, IconColor, group_label, menu_line,
};

use crate::config::{Config, HIDE_OFFLINE_CHOICES, LOW_CHOICES, REFRESH_CHOICES, hex, parse_color};
use crate::interval::format_interval;
use crate::login::LoginState;
use crate::receivers::can_check;

const CHOICE_OFF: &str = "off";
const CHOICE_OFF_LABEL: &str = "Off";
const MINUTES_PER_HOUR: u32 = 60;
const CHECK_BATTERY_TEXT: &str = "Check battery now";
const CUSTOM_REFRESH_ID: &str = "auto:custom";
const ICON_AUTO: &str = "auto";
const SECONDS_PER_MINUTE: u32 = 60;

/// Built-in icon choices in menu order; `None` is Auto (from the device kind).
const BUILT_IN_ICONS: [(&str, Option<DeviceKind>); 7] = [
    ("Auto", None),
    ("Headset", Some(DeviceKind::Headset)),
    ("Earbuds", Some(DeviceKind::Earbuds)),
    ("Mouse", Some(DeviceKind::Mouse)),
    ("Keyboard", Some(DeviceKind::Keyboard)),
    ("Gamepad", Some(DeviceKind::Gamepad)),
    ("Bluetooth", Some(DeviceKind::Other)),
];

/// Icon colour palette in menu order.
const PALETTE: [(&str, IconColor); 7] = [
    ("Red", IconColor::Rgb(0xE5, 0x48, 0x4D)),
    ("Orange", IconColor::Rgb(0xF7, 0x6B, 0x15)),
    ("Yellow", IconColor::Rgb(0xFF, 0xC5, 0x3D)),
    ("Green", IconColor::Rgb(0x30, 0xA4, 0x6C)),
    ("Blue", IconColor::Rgb(0x3E, 0x8E, 0xED)),
    ("Purple", IconColor::Rgb(0x8E, 0x4E, 0xC6)),
    ("Pink", IconColor::Rgb(0xD6, 0x40, 0x9F)),
];

/// Device actions carry the device id, which may itself contain `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Refresh,
    SetRefresh(Option<u32>),
    CustomRefresh,
    SetLow(Option<u8>),
    ToggleHidden(String),
    TogglePercent(String),
    SetIcon(String, IconChoice),
    PickSvg(String),
    SetColor(String, IconColor),
    CustomColor(String),
    /// One battery query for an unconfirmed receiver; carries only the row id.
    CheckBattery(String),
    /// Minutes before an offline device leaves the strip; `None` = never.
    SetHideOffline(Option<u32>),
    ToggleStartAtLogin,
    Quit,
}

/// A platform-neutral menu item; `id: None` with empty text is a separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    pub id: Option<String>,
    pub text: String,
    pub checked: Option<bool>,
    pub enabled: bool,
    pub children: Vec<MenuEntry>,
}

impl MenuEntry {
    fn new(id: Option<String>, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            checked: None,
            enabled: true,
            children: Vec::new(),
        }
    }

    pub fn separator() -> Self {
        Self::new(None, "")
    }

    /// A greyed-out text line.
    pub fn label(text: impl Into<String>) -> Self {
        Self {
            enabled: false,
            ..Self::new(None, text)
        }
    }

    fn item(id: &str, text: impl Into<String>) -> Self {
        Self::new(Some(id.to_owned()), text)
    }

    fn check(id: String, text: impl Into<String>, checked: bool) -> Self {
        Self {
            checked: Some(checked),
            ..Self::new(Some(id), text)
        }
    }

    fn submenu(text: impl Into<String>, children: Vec<MenuEntry>) -> Self {
        Self {
            children,
            ..Self::new(None, text)
        }
    }

    fn is_separator(&self) -> bool {
        self.id.is_none() && self.text.is_empty()
    }
}

/// Maps a clicked menu id back to its action; unknown ids give `None`. Device ids come last
/// (`<action>:<value>:<device id>`), so only the action and value are split off.
pub fn action_for(id: &str) -> Option<MenuAction> {
    match id {
        "refresh" => return Some(MenuAction::Refresh),
        "login" => return Some(MenuAction::ToggleStartAtLogin),
        "quit" => return Some(MenuAction::Quit),
        CUSTOM_REFRESH_ID => return Some(MenuAction::CustomRefresh),
        _ => {}
    }
    let (action, rest) = id.split_once(':')?;
    let device = rest.to_owned();
    match action {
        "auto" => choice_from_id(rest, &REFRESH_CHOICES).map(MenuAction::SetRefresh),
        "low" => choice_from_id(rest, &LOW_CHOICES).map(MenuAction::SetLow),
        "offline" => choice_from_id(rest, &HIDE_OFFLINE_CHOICES).map(MenuAction::SetHideOffline),
        "hide" => Some(MenuAction::ToggleHidden(device)),
        "pct" => Some(MenuAction::TogglePercent(device)),
        "svg" => Some(MenuAction::PickSvg(device)),
        "ccolor" => Some(MenuAction::CustomColor(device)),
        "check" => Some(MenuAction::CheckBattery(device)),
        "icon" => {
            let (value, device) = rest.split_once(':')?;
            let icon = if value == ICON_AUTO {
                IconChoice::Auto
            } else {
                IconChoice::Kind(value.parse().ok()?)
            };
            Some(MenuAction::SetIcon(device.to_owned(), icon))
        }
        "color" => {
            let (value, device) = rest.split_once(':')?;
            Some(MenuAction::SetColor(device.to_owned(), parse_color(value)?))
        }
        _ => None,
    }
}

/// `off` or a number that is one of `choices`; the outer `None` means the id is not valid.
fn choice_from_id<T: FromStr + PartialEq>(text: &str, choices: &[Option<T>]) -> Option<Option<T>> {
    let value = if text == CHOICE_OFF {
        None
    } else {
        Some(text.parse().ok()?)
    };
    choices.contains(&value).then_some(value)
}

fn choice_id<T: Display>(prefix: &str, choice: Option<T>) -> String {
    choice.map_or_else(
        || format!("{prefix}:{CHOICE_OFF}"),
        |value| format!("{prefix}:{value}"),
    )
}

/// Devices that get a menu line: connected ones, plus disconnected ones that still have a
/// reading, greyed out (a paired Mac or phone has none, so never shows).
pub fn listed_devices(devices: &[DeviceView]) -> impl Iterator<Item = &DeviceView> {
    devices
        .iter()
        .filter(|device| device.connected || device.shown_percent().is_some())
}

/// The whole menu as plain data, rebuilt after every result or action.
pub fn menu_entries(
    devices: &[DeviceView],
    config: &Config,
    login: LoginState,
    now: SystemTime,
) -> Vec<MenuEntry> {
    let mut entries = device_entries(devices, config, now);
    if entries.is_empty() {
        entries.push(MenuEntry::label(EMPTY_MENU_TEXT));
    }
    entries.extend([
        MenuEntry::separator(),
        MenuEntry::item("refresh", "Refresh now"),
        MenuEntry::submenu("Auto-refresh", refresh_entries(config.refresh_seconds)),
        MenuEntry::submenu(
            "Low-battery alert",
            choice_entries(
                "low",
                &LOW_CHOICES,
                config.low_battery_percent,
                CHOICE_OFF_LABEL,
                |percent| format!("{percent}%"),
            ),
        ),
        MenuEntry::submenu(
            "Hide offline after",
            choice_entries(
                "offline",
                &HIDE_OFFLINE_CHOICES,
                config.hide_offline_minutes,
                "Never",
                format_minutes,
            ),
        ),
        login_entry(login),
        MenuEntry::separator(),
        MenuEntry::item("quit", "Quit"),
    ]);
    entries
}

/// Device lines under a disabled group header, a separator between groups; `devices` arrive
/// already grouped by kind, so a header marks each change of kind.
fn device_entries(devices: &[DeviceView], config: &Config, now: SystemTime) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    let mut group = None;
    for device in listed_devices(devices) {
        if group != Some(device.kind) {
            if group.is_some() {
                entries.push(MenuEntry::separator());
            }
            entries.push(MenuEntry::label(group_label(device.kind)));
            group = Some(device.kind);
        }
        entries.push(device_entry(device, config, now));
    }
    entries
}

/// The device line opens a submenu holding all of that device's settings.
fn device_entry(device: &DeviceView, config: &Config, now: SystemTime) -> MenuEntry {
    let id = &device.id;
    let options = config.display_options(id);
    let is_svg = matches!(options.icon, IconChoice::Svg(_));
    // Only a receiver held back for want of confirmation offers the one-query check.
    let check = can_check(id, &config.allowed())
        .then(|| MenuEntry::item(&format!("check:{id}"), CHECK_BATTERY_TEXT));
    let mut children: Vec<MenuEntry> = check.into_iter().collect();
    children.extend([
        MenuEntry::check(
            format!("hide:{id}"),
            "Show on strip",
            !config.is_hidden(id, &device.name),
        ),
        MenuEntry::check(format!("pct:{id}"), "Show percentage", options.show_percent),
        MenuEntry::submenu("Icon", icon_entries(id, &options.icon)),
        MenuEntry::submenu("Icon colour", color_entries(id, options.color, is_svg)),
    ]);
    // An offline device's line (disconnected, or a stale receiver) is greyed out.
    MenuEntry {
        enabled: !device.is_offline(),
        ..MenuEntry::submenu(menu_line(device, now), children)
    }
}

/// Built-in icons then `Custom SVG…`, the current choice ticked.
fn icon_entries(id: &str, current: &IconChoice) -> Vec<MenuEntry> {
    let mut entries: Vec<MenuEntry> = BUILT_IN_ICONS
        .iter()
        .map(|&(label, kind)| {
            let (value, choice) = kind.map_or((ICON_AUTO, IconChoice::Auto), |kind| {
                (kind.as_str(), IconChoice::Kind(kind))
            });
            MenuEntry::check(format!("icon:{value}:{id}"), label, *current == choice)
        })
        .collect();
    entries.push(MenuEntry::check(
        format!("svg:{id}"),
        "Custom SVG…",
        matches!(current, IconChoice::Svg(_)),
    ));
    entries
}

/// Auto, Original (SVG icons only), the palette, then `Custom…` showing a non-palette colour.
fn color_entries(id: &str, current: IconColor, is_svg: bool) -> Vec<MenuEntry> {
    let color_entry = |label: &str, color: IconColor| {
        let value = match color {
            IconColor::Auto => "auto".to_owned(),
            IconColor::Original => "original".to_owned(),
            IconColor::Rgb(red, green, blue) => hex(red, green, blue),
        };
        MenuEntry::check(format!("color:{value}:{id}"), label, current == color)
    };
    let mut entries = vec![
        color_entry("Auto", IconColor::Auto),
        MenuEntry {
            enabled: is_svg,
            ..color_entry("Original", IconColor::Original)
        },
    ];
    entries.extend(
        PALETTE
            .iter()
            .map(|&(label, color)| color_entry(label, color)),
    );
    let custom = match current {
        IconColor::Rgb(red, green, blue) if !PALETTE.iter().any(|&(_, c)| c == current) => {
            Some(hex(red, green, blue))
        }
        _ => None,
    };
    entries.push(MenuEntry::check(
        format!("ccolor:{id}"),
        custom
            .as_ref()
            .map_or_else(|| "Custom…".to_owned(), |hex| format!("Custom… (#{hex})")),
        custom.is_some(),
    ));
    entries
}

/// Quick choices, then `Custom…` showing and ticking an interval that is not one of them.
fn refresh_entries(current: Option<u32>) -> Vec<MenuEntry> {
    let mut entries = choice_entries(
        "auto",
        &REFRESH_CHOICES,
        current,
        CHOICE_OFF_LABEL,
        |seconds| format!("{} min", seconds / SECONDS_PER_MINUTE),
    );
    let custom = current.filter(|_| !REFRESH_CHOICES.contains(&current));
    entries.push(MenuEntry::check(
        CUSTOM_REFRESH_ID.to_owned(),
        custom.map_or_else(
            || "Custom…".to_owned(),
            |seconds| format!("Custom… ({})", format_interval(seconds)),
        ),
        custom.is_some(),
    ));
    entries
}

/// `5 min`, `1 h`: whole hours read as hours.
fn format_minutes(minutes: u32) -> String {
    if minutes.is_multiple_of(MINUTES_PER_HOUR) {
        format!("{} h", minutes / MINUTES_PER_HOUR)
    } else {
        format!("{minutes} min")
    }
}

/// One ticked entry per submenu, `none_label` for the `None` choice; muda has no radio items,
/// so the menu is rebuilt instead.
fn choice_entries<T: Copy + PartialEq + Display>(
    prefix: &str,
    choices: &[Option<T>],
    current: Option<T>,
    none_label: &str,
    label: impl Fn(T) -> String,
) -> Vec<MenuEntry> {
    choices
        .iter()
        .map(|&choice| {
            MenuEntry::check(
                choice_id(prefix, choice),
                choice.map_or_else(|| none_label.to_owned(), &label),
                choice == current,
            )
        })
        .collect()
}

fn login_entry(login: LoginState) -> MenuEntry {
    let id = "login".to_owned();
    match login {
        LoginState::Enabled => MenuEntry::check(id, "Start at login", true),
        LoginState::Disabled => MenuEntry::check(id, "Start at login", false),
        LoginState::RequiresApproval => {
            MenuEntry::check(id, "Start at login (approve in System Settings)", false)
        }
        LoginState::Unavailable => MenuEntry {
            enabled: false,
            ..MenuEntry::check(id, "Start at login (app bundle only)", false)
        },
    }
}

/// Maps entries to muda; untested because muda menus can only be created on the macOS main thread.
pub fn build_menu(entries: &[MenuEntry]) -> Menu {
    let menu = Menu::new();
    for entry in entries {
        if let Err(error) = menu.append(to_item(entry).as_ref()) {
            eprintln!("menu item failed: {error}");
        }
    }
    menu
}

fn to_item(entry: &MenuEntry) -> Box<dyn IsMenuItem> {
    if entry.is_separator() {
        return Box::new(PredefinedMenuItem::separator());
    }
    if !entry.children.is_empty() {
        let submenu = Submenu::new(&entry.text, entry.enabled);
        for child in &entry.children {
            if let Err(error) = submenu.append(to_item(child).as_ref()) {
                eprintln!("menu item failed: {error}");
            }
        }
        return Box::new(submenu);
    }
    match (&entry.id, entry.checked) {
        (Some(id), Some(checked)) => Box::new(CheckMenuItem::with_id(
            id.as_str(),
            &entry.text,
            entry.enabled,
            checked,
            None,
        )),
        (Some(id), None) => Box::new(MenuItem::with_id(
            id.as_str(),
            &entry.text,
            entry.enabled,
            None,
        )),
        (None, _) => Box::new(MenuItem::new(&entry.text, entry.enabled, None)),
    }
}

#[cfg(test)]
mod tests {
    use ui_model::{DisplayOptions, Level, Path};

    use super::*;

    const MAC_ID: &str = "bt:a4-c3-f0-11-22-33";
    const HEX_ID: &str = "bt:0123456789abcdef0123456789abcdef";

    fn device(name: &str, level: Level, connected: bool) -> DeviceView {
        DeviceView {
            id: format!("bt:{name}"),
            name: name.to_owned(),
            kind: DeviceKind::Headset,
            path: Path::Bluetooth,
            level,
            charging: false,
            connected,
            updated_at: SystemTime::UNIX_EPOCH,
            stale: false,
            note: None,
        }
    }

    fn entries(devices: &[DeviceView], config: &Config, login: LoginState) -> Vec<MenuEntry> {
        menu_entries(devices, config, login, SystemTime::UNIX_EPOCH)
    }

    fn child<'a>(entry: &'a MenuEntry, text: &str) -> &'a MenuEntry {
        entry.children.iter().find(|e| e.text == text).unwrap()
    }

    /// The first device line: skips the group header; settings submenus come after the devices.
    fn first_device(entries: &[MenuEntry]) -> &MenuEntry {
        entries.iter().find(|e| !e.children.is_empty()).unwrap()
    }

    fn ticked_ids(entry: &MenuEntry) -> Vec<&str> {
        entry
            .children
            .iter()
            .filter(|e| e.checked == Some(true))
            .filter_map(|e| e.id.as_deref())
            .collect()
    }

    #[test]
    fn menu_ids_roundtrip() {
        assert_eq!(action_for("auto:off"), Some(MenuAction::SetRefresh(None)));
        assert_eq!(
            action_for("auto:300"),
            Some(MenuAction::SetRefresh(Some(300)))
        );
        assert_eq!(action_for("auto:custom"), Some(MenuAction::CustomRefresh));
        assert_eq!(action_for("low:20"), Some(MenuAction::SetLow(Some(20))));
        assert_eq!(action_for("login"), Some(MenuAction::ToggleStartAtLogin));
        assert_eq!(action_for("auto:7"), None);
        assert_eq!(action_for("icon:toaster:bt:x"), None);
        assert_eq!(action_for("color:zzz:bt:x"), None);
        assert_eq!(action_for("nope"), None);
    }

    #[test]
    fn menu_ids_roundtrip_device_actions() {
        for id in [MAC_ID, HEX_ID] {
            let owned = id.to_owned();
            assert_eq!(
                action_for(&format!("hide:{id}")),
                Some(MenuAction::ToggleHidden(owned.clone()))
            );
            assert_eq!(
                action_for(&format!("pct:{id}")),
                Some(MenuAction::TogglePercent(owned.clone()))
            );
            assert_eq!(
                action_for(&format!("svg:{id}")),
                Some(MenuAction::PickSvg(owned.clone()))
            );
            assert_eq!(
                action_for(&format!("ccolor:{id}")),
                Some(MenuAction::CustomColor(owned.clone()))
            );
            assert_eq!(
                action_for(&format!("icon:mouse:{id}")),
                Some(MenuAction::SetIcon(
                    owned.clone(),
                    IconChoice::Kind(DeviceKind::Mouse)
                ))
            );
            assert_eq!(
                action_for(&format!("icon:auto:{id}")),
                Some(MenuAction::SetIcon(owned.clone(), IconChoice::Auto))
            );
            assert_eq!(
                action_for(&format!("color:3E8EED:{id}")),
                Some(MenuAction::SetColor(
                    owned.clone(),
                    IconColor::Rgb(0x3E, 0x8E, 0xED)
                ))
            );
            assert_eq!(
                action_for(&format!("color:original:{id}")),
                Some(MenuAction::SetColor(owned, IconColor::Original))
            );
        }
    }

    /// Every id the menu builds parses back to an action for the same device.
    #[test]
    fn built_device_ids_parse() {
        let mut device = device("x", Level::Percent(50), true);
        device.id = MAC_ID.to_owned();
        let entries = entries(&[device], &Config::default(), LoginState::Disabled);
        let mut ids = Vec::new();
        let mut stack: Vec<&MenuEntry> = vec![first_device(&entries)];
        while let Some(entry) = stack.pop() {
            ids.extend(entry.id.as_deref());
            stack.extend(&entry.children);
        }
        // Show on strip, Show percentage, 8 icon and 10 colour entries.
        assert_eq!(ids.len(), 20);
        for id in ids {
            assert!(action_for(id).is_some(), "{id}");
            assert!(id.ends_with(MAC_ID), "{id}");
        }
    }

    #[test]
    fn check_battery_id_roundtrips() {
        for id in ["usb-373b-1216", MAC_ID] {
            assert_eq!(
                action_for(&format!("check:{id}")),
                Some(MenuAction::CheckBattery(id.to_owned()))
            );
        }
    }

    #[test]
    fn check_battery_only_for_unconfirmed_not_allowed_receivers() {
        let row = |id: &str| DeviceView {
            id: id.to_owned(),
            path: Path::Dongle,
            ..device(id, Level::NoData("x".into()), true)
        };
        let check_ids = |config: &Config| -> Vec<String> {
            let devices = [
                row("usb-391d-1a05"),
                row("usb-3837-6011"),
                device("Headset", Level::Percent(50), true),
            ];
            entries(&devices, config, LoginState::Disabled)
                .iter()
                .flat_map(|e| &e.children)
                .filter(|e| e.text == "Check battery now")
                .filter_map(|e| e.id.clone())
                .collect()
        };
        let ids = check_ids(&Config::default());
        assert_eq!(ids, ["check:usb-391d-1a05"]);
        assert_eq!(
            action_for(&ids[0]),
            Some(MenuAction::CheckBattery("usb-391d-1a05".into()))
        );

        let allowed = Config {
            allow: vec!["391d:1a05".into()],
            ..Config::default()
        };
        assert!(check_ids(&allowed).is_empty());
    }

    #[test]
    fn device_submenu_ticks_current() {
        let mut device = device("MX Master", Level::Percent(50), true);
        device.id = HEX_ID.to_owned();
        let mut config = Config::default();
        config.update_device(HEX_ID, "MX Master", |options| {
            *options = DisplayOptions {
                show_percent: false,
                icon: IconChoice::Kind(DeviceKind::Mouse),
                color: IconColor::Rgb(0x3E, 0x8E, 0xED),
            };
        });
        let entries = entries(&[device], &config, LoginState::Disabled);
        let submenu = first_device(&entries);

        assert_eq!(child(submenu, "Show on strip").checked, Some(true));
        assert_eq!(child(submenu, "Show percentage").checked, Some(false));
        assert_eq!(
            ticked_ids(child(submenu, "Icon")),
            [format!("icon:mouse:{HEX_ID}")]
        );
        let colors = child(submenu, "Icon colour");
        assert_eq!(ticked_ids(colors), [format!("color:3E8EED:{HEX_ID}")]);
        assert!(!child(colors, "Original").enabled);
        assert_eq!(child(colors, "Custom…").checked, Some(false));
    }

    #[test]
    fn custom_colour_and_svg_ticked() {
        let mut device = device("Atom", Level::Percent(50), true);
        device.id = MAC_ID.to_owned();
        let mut config = Config::default();
        config.update_device(MAC_ID, "Atom", |options| {
            options.icon = IconChoice::Svg("a.svg".into());
            options.color = IconColor::Rgb(0xFF, 0x88, 0x00);
        });
        let entries = entries(&[device], &config, LoginState::Disabled);
        let submenu = first_device(&entries);

        assert_eq!(
            ticked_ids(child(submenu, "Icon")),
            [format!("svg:{MAC_ID}")]
        );
        let colors = child(submenu, "Icon colour");
        assert!(child(colors, "Original").enabled);
        assert_eq!(ticked_ids(colors), [format!("ccolor:{MAC_ID}")]);
        assert_eq!(child(colors, "Custom… (#FF8800)").checked, Some(true));
    }

    #[test]
    fn custom_refresh_entry_label() {
        let config = Config {
            refresh_seconds: Some(5400),
            ..Config::default()
        };
        let entries = entries(&[], &config, LoginState::Disabled);
        let auto = entries.iter().find(|e| e.text == "Auto-refresh").unwrap();
        assert_eq!(ticked_ids(auto), [CUSTOM_REFRESH_ID]);
        assert_eq!(child(auto, "Custom… (1h30m)").checked, Some(true));

        let quick = Config::default();
        let entries = menu_entries(&[], &quick, LoginState::Disabled, SystemTime::UNIX_EPOCH);
        let auto = entries.iter().find(|e| e.text == "Auto-refresh").unwrap();
        assert_eq!(child(auto, "Custom…").checked, Some(false));
    }

    #[test]
    fn empty_menu_entries() {
        let entries = entries(&[], &Config::default(), LoginState::Disabled);
        assert_eq!(entries[0].text, EMPTY_MENU_TEXT);
        assert!(!entries[0].enabled);
    }

    #[test]
    fn menu_entries_ticks_one_choice() {
        let config = Config {
            refresh_seconds: Some(300),
            ..Config::default()
        };
        let entries = entries(&[], &config, LoginState::Disabled);
        let auto = entries.iter().find(|e| e.text == "Auto-refresh").unwrap();
        assert_eq!(ticked_ids(auto), ["auto:300"]);
        assert_eq!(child(auto, "5 min").checked, Some(true));
    }

    #[test]
    fn login_entry_states() {
        let find = |login| {
            entries(&[], &Config::default(), login)
                .into_iter()
                .find(|e| e.id.as_deref() == Some("login"))
                .unwrap()
        };
        let unavailable = find(LoginState::Unavailable);
        assert_eq!(unavailable.text, "Start at login (app bundle only)");
        assert!(!unavailable.enabled);
        let approval = find(LoginState::RequiresApproval);
        assert_eq!(approval.text, "Start at login (approve in System Settings)");
        assert!(approval.enabled);
    }

    #[test]
    fn disconnected_device_entry_is_disabled() {
        let devices = [
            device("Here", Level::Percent(60), true),
            device("Away", Level::Percent(40), false),
        ];
        let entries = entries(&devices, &Config::default(), LoginState::Disabled);
        // entries[0] is the group header.
        assert!(entries[1].enabled);
        assert!(!entries[2].enabled);
        assert!(!entries[2].text.contains("not connected"));
    }

    #[test]
    fn stale_device_entry_is_disabled_and_keeps_its_text() {
        let mut stale = device("Mouse", Level::Percent(70), true);
        stale.stale = true;
        let fresh = device("Mouse", Level::Percent(70), true);
        let entries_for = |d: DeviceView| entries(&[d], &Config::default(), LoginState::Disabled);
        let (stale_entries, fresh_entries) = (entries_for(stale), entries_for(fresh));
        let (stale_line, fresh_line) = (first_device(&stale_entries), first_device(&fresh_entries));
        assert!(!stale_line.enabled);
        assert!(fresh_line.enabled);
        assert!(stale_line.text.starts_with("Mouse — 70% · BT"));
    }

    #[test]
    fn hide_offline_radio_roundtrip() {
        assert_eq!(
            action_for("offline:off"),
            Some(MenuAction::SetHideOffline(None))
        );
        assert_eq!(
            action_for("offline:15"),
            Some(MenuAction::SetHideOffline(Some(15)))
        );
        assert_eq!(action_for("offline:7"), None);
        assert_eq!(action_for("disc"), None);

        let find = |minutes| {
            let config = Config {
                hide_offline_minutes: minutes,
                ..Config::default()
            };
            entries(&[], &config, LoginState::Disabled)
                .into_iter()
                .find(|e| e.text == "Hide offline after")
                .unwrap()
        };
        let default = find(Some(15));
        let labels: Vec<&str> = default.children.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(labels, ["Never", "5 min", "15 min", "30 min", "1 h"]);
        assert_eq!(ticked_ids(&default), ["offline:15"]);
        assert_eq!(ticked_ids(&find(None)), ["offline:off"]);
        assert_eq!(ticked_ids(&find(Some(60))), ["offline:60"]);
        // Every built id parses back to the choice it ticks.
        for entry in &default.children {
            assert!(action_for(entry.id.as_deref().unwrap()).is_some());
        }
    }

    fn kinded(name: &str, kind: DeviceKind) -> DeviceView {
        DeviceView {
            kind,
            ..device(name, Level::Percent(50), true)
        }
    }

    #[test]
    fn menu_groups_get_headers_and_separators() {
        let devices = [
            kinded("N9", DeviceKind::Headset),
            kinded("V9", DeviceKind::Headset),
            kinded("X1", DeviceKind::Mouse),
            kinded("Pad", DeviceKind::Gamepad),
        ];
        let entries = entries(&devices, &Config::default(), LoginState::Disabled);
        // (is header, is separator, text) of the leading entries up to the settings separator.
        let shape: Vec<(bool, bool, &str)> = entries
            .iter()
            .take(9)
            .map(|e| {
                (
                    e.id.is_none() && !e.enabled,
                    e.is_separator(),
                    e.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (true, false, "Headset"),
                (false, false, "N9 — 50% · BT"),
                (false, false, "V9 — 50% · BT"),
                (false, true, ""),
                (true, false, "Mouse"),
                (false, false, "X1 — 50% · BT"),
                (false, true, ""),
                (true, false, "Controller"),
                (false, false, "Pad — 50% · BT"),
            ]
        );
        // The existing separator between devices and settings stays.
        assert!(entries[9].is_separator());
        assert_eq!(entries[10].id.as_deref(), Some("refresh"));
    }

    #[test]
    fn single_group_has_one_header_and_no_inner_separator() {
        let devices = [kinded("N9", DeviceKind::Headset)];
        let entries = entries(&devices, &Config::default(), LoginState::Disabled);
        assert_eq!(entries[0].text, "Headset");
        assert!(!entries[0].enabled);
        assert!(!entries[1].is_separator());
        assert!(entries[2].is_separator());
    }

    #[test]
    fn menu_device_lines_filter() {
        let devices = [
            device("Connected no data", Level::NoData("x".into()), true),
            device("Connected", Level::Percent(60), true),
            device("Away with level", Level::Percent(40), false),
            device("Paired Mac", Level::NoData("x".into()), false),
        ];
        let lines: Vec<String> = entries(&devices, &Config::default(), LoginState::Disabled)
            .iter()
            .flat_map(|e| &e.children)
            .filter_map(|e| e.id.as_deref()?.strip_prefix("hide:bt:").map(str::to_owned))
            .collect();
        // A disconnected device with a reading stays (greyed); one without never lists.
        assert_eq!(lines, ["Connected no data", "Connected", "Away with level"]);
    }
}
