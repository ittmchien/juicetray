use std::fmt::Display;
use std::str::FromStr;
use std::time::SystemTime;

use strip_render::Theme;
#[cfg(not(windows))]
use tray_icon::menu::CheckMenuItem;
#[cfg(windows)]
use tray_icon::menu::{Icon, IconMenuItem};
use tray_icon::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use ui_model::{
    DeviceKind, DeviceView, EMPTY_MENU_TEXT, IconChoice, IconColor, Path, group_label, menu_line,
    needs_charge, shown_age,
};

use crate::config::{Config, HIDE_OFFLINE_CHOICES, LOW_CHOICES, REFRESH_CHOICES, hex, parse_color};
use crate::interval::format_interval;
use crate::login::LoginState;
use crate::symbols::symbol_for_device;
use crate::update::UpdateStage;

const CHECK_UPDATES_ID: &str = "check-updates";
const AUTO_UPDATE_ID: &str = "update-auto";
const OPEN_UPDATE_ID: &str = "update-open";
const CHOICE_OFF: &str = "off";
const CHOICE_OFF_LABEL: &str = "Off";
const MINUTES_PER_HOUR: u32 = 60;
const CUSTOM_REFRESH_ID: &str = "auto:custom";
const CUSTOM_LOW_ID: &str = "low:custom";
const CUSTOM_OFFLINE_ID: &str = "offline:custom";
const REFRESH_ON_BATTERY_ID: &str = "refresh-battery";
const DETECT_NEW_DEVICES_ID: &str = "detect-new";
const ADD_DEVICE_ID: &str = "add-device";
/// Only Windows watches for new devices and has the Add device window, so only it offers them.
const DETECT_NEW_AVAILABLE: bool = cfg!(windows);
const ICON_AUTO: &str = "auto";
/// Menu id value of the system-symbol entry; the symbol itself follows the device's model.
const ICON_SYMBOL: &str = "symbol";
/// Only macOS ships the SF Symbols the AirPods entry draws from.
const SYMBOLS_AVAILABLE: bool = cfg!(target_os = "macos");
const SECONDS_PER_MINUTE: u32 = 60;
/// The submenu that holds every app option.
const SETTINGS_LABEL: &str = "Settings";
/// The icon sits on the taskbar on Windows and in the menu bar on macOS.
#[cfg(windows)]
const CUSTOM_LIGHT_LABEL: &str = "Light taskbar…";
#[cfg(windows)]
const CUSTOM_DARK_LABEL: &str = "Dark taskbar…";
#[cfg(not(windows))]
const CUSTOM_LIGHT_LABEL: &str = "Light menu bar…";
#[cfg(not(windows))]
const CUSTOM_DARK_LABEL: &str = "Dark menu bar…";
/// The `Custom` submenu's entry that sets one colour for both themes.
const CUSTOM_BOTH_LABEL: &str = "Both…";
/// The submenu in `Icon colour` that groups the custom colour pickers.
const CUSTOM_COLOR_LABEL: &str = "Custom";

/// Built-in icon choices in menu order; `None` is Auto (from the device kind).
const BUILT_IN_ICONS: [(&str, Option<DeviceKind>); 7] = [
    ("Auto", None),
    ("Headset", Some(DeviceKind::Headset)),
    ("Earbuds", Some(DeviceKind::Earbuds)),
    ("Mouse", Some(DeviceKind::Mouse)),
    ("Keyboard", Some(DeviceKind::Keyboard)),
    ("Gamepad", Some(DeviceKind::Gamepad)),
    ("Generic", Some(DeviceKind::Other)),
];

/// Icon colour palette in menu order: (name, deeper shade for the light taskbar, lighter shade
/// for the dark taskbar), each pair readable (contrast >= 3:1) on its own bar.
const PALETTE: [(&str, IconColor, IconColor); 7] = [
    (
        "Red",
        IconColor::Rgb(0xD6, 0x3A, 0x3F),
        IconColor::Rgb(0xF2, 0x68, 0x6D),
    ),
    (
        "Orange",
        IconColor::Rgb(0xD9, 0x5A, 0x06),
        IconColor::Rgb(0xFF, 0x8A, 0x3D),
    ),
    (
        "Yellow",
        IconColor::Rgb(0xA8, 0x75, 0x00),
        IconColor::Rgb(0xFF, 0xCB, 0x47),
    ),
    (
        "Green",
        IconColor::Rgb(0x2A, 0x8F, 0x5E),
        IconColor::Rgb(0x4D, 0xBE, 0x8A),
    ),
    (
        "Blue",
        IconColor::Rgb(0x2B, 0x7F, 0xE0),
        IconColor::Rgb(0x6A, 0xA8, 0xF5),
    ),
    (
        "Purple",
        IconColor::Rgb(0x83, 0x40, 0xBE),
        IconColor::Rgb(0xB0, 0x7F, 0xE0),
    ),
    (
        "Pink",
        IconColor::Rgb(0xCC, 0x33, 0x92),
        IconColor::Rgb(0xEA, 0x6F, 0xBD),
    ),
];

/// Device actions carry the device id, which may itself contain `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Refresh,
    SetRefresh(Option<u32>),
    CustomRefresh,
    SetLow(Option<u8>),
    CustomLow,
    ToggleHidden(String),
    TogglePercent(String),
    ToggleLabel(String),
    SetIcon(String, IconChoice),
    /// The system symbol that matches the device's model becomes its icon.
    SetSymbolIcon(String),
    PickSvg(String),
    /// The (light taskbar, dark taskbar) icon colours.
    SetColors(String, IconColor, IconColor),
    /// Ask for a colour: `None` sets both taskbar themes, `Some` only that one.
    CustomColor(String, Option<Theme>),
    /// Minutes before an offline device leaves the strip; `None` = never.
    SetHideOffline(Option<u32>),
    CustomHideOffline,
    ToggleStartAtLogin,
    /// New-device popups on or off; plug-in refreshes of queried devices stay.
    ToggleDetectNewDevices,
    /// Opens the Add device window, which warns first, then detects the device.
    AddDevice,
    /// Turning it on is confirmed first; turning it off is not.
    ToggleRefreshOnBattery,
    /// Check GitHub Releases now and say the result (macOS opens the releases page instead).
    CheckForUpdates,
    ToggleCheckUpdates,
    /// Offer the update the menu announces: install it, open its download page or leave it.
    OpenUpdate,
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
        REFRESH_ON_BATTERY_ID => return Some(MenuAction::ToggleRefreshOnBattery),
        DETECT_NEW_DEVICES_ID => return Some(MenuAction::ToggleDetectNewDevices),
        ADD_DEVICE_ID => return Some(MenuAction::AddDevice),
        CHECK_UPDATES_ID => return Some(MenuAction::CheckForUpdates),
        AUTO_UPDATE_ID => return Some(MenuAction::ToggleCheckUpdates),
        OPEN_UPDATE_ID => return Some(MenuAction::OpenUpdate),
        "quit" => return Some(MenuAction::Quit),
        CUSTOM_REFRESH_ID => return Some(MenuAction::CustomRefresh),
        CUSTOM_LOW_ID => return Some(MenuAction::CustomLow),
        CUSTOM_OFFLINE_ID => return Some(MenuAction::CustomHideOffline),
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
        "label" => Some(MenuAction::ToggleLabel(device)),
        "svg" => Some(MenuAction::PickSvg(device)),
        "ccolor" => Some(MenuAction::CustomColor(device, None)),
        "ccolor-light" => Some(MenuAction::CustomColor(device, Some(Theme::Light))),
        "ccolor-dark" => Some(MenuAction::CustomColor(device, Some(Theme::Dark))),
        "icon" => {
            let (value, device) = rest.split_once(':')?;
            if value == ICON_SYMBOL {
                return Some(MenuAction::SetSymbolIcon(device.to_owned()));
            }
            let icon = if value == ICON_AUTO {
                IconChoice::Auto
            } else {
                IconChoice::Kind(value.parse().ok()?)
            };
            Some(MenuAction::SetIcon(device.to_owned(), icon))
        }
        // `color:<light>:<dark>:<device id>`, each colour `auto`, `original` or `RRGGBB`.
        "color" => {
            let (light, rest) = rest.split_once(':')?;
            let (dark, device) = rest.split_once(':')?;
            Some(MenuAction::SetColors(
                device.to_owned(),
                parse_color(light)?,
                parse_color(dark)?,
            ))
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

/// The whole menu as plain data, rebuilt after every result or action: devices first, then the
/// actions. `has_battery` shows the battery-refresh check, which means nothing on a machine
/// without a battery; `update_stage` (an install under way) leads the menu and greys the check.
pub fn menu_entries(
    devices: &[DeviceView],
    config: &Config,
    login: LoginState,
    has_battery: bool,
    update_stage: Option<&UpdateStage>,
    now: SystemTime,
) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    if let Some(stage) = update_stage {
        entries.extend([MenuEntry::label(stage.text()), MenuEntry::separator()]);
    }
    let devices = device_entries(devices, config, now);
    if devices.is_empty() {
        entries.push(MenuEntry::label(EMPTY_MENU_TEXT));
    }
    entries.extend(devices);
    entries.extend([
        MenuEntry::separator(),
        MenuEntry::item("refresh", "Refresh now"),
    ]);
    if DETECT_NEW_AVAILABLE {
        entries.push(MenuEntry::item(ADD_DEVICE_ID, "Add device…"));
    }
    // The main menu keeps the devices and everyday commands; every option lives under Settings.
    entries.extend([
        MenuEntry::submenu(SETTINGS_LABEL, settings_entries(config, login, has_battery)),
        MenuEntry::separator(),
        // The running version sits on the update line; a second check waits for the install.
        MenuEntry {
            enabled: update_stage.is_none(),
            ..MenuEntry::item(
                CHECK_UPDATES_ID,
                format!("Check for updates… (v{})", env!("CARGO_PKG_VERSION")),
            )
        },
        MenuEntry::item("quit", "Quit"),
    ]);
    entries
}

/// The Settings submenu: refreshing, alerts, detection and start-up, then updates.
fn settings_entries(config: &Config, login: LoginState, has_battery: bool) -> Vec<MenuEntry> {
    let mut entries = vec![MenuEntry::submenu(
        "Auto-refresh",
        refresh_entries(config.refresh_seconds),
    )];
    if has_battery {
        entries.push(MenuEntry::check(
            REFRESH_ON_BATTERY_ID.to_owned(),
            "Refresh on battery power",
            config.refresh_on_battery,
        ));
    }
    entries.extend([
        MenuEntry::separator(),
        MenuEntry::submenu(
            "Low-battery alert",
            choice_entries_with_custom(
                "low",
                &LOW_CHOICES,
                config.low_battery_percent,
                CHOICE_OFF_LABEL,
                CUSTOM_LOW_ID,
                |percent| format!("{percent}%"),
            ),
        ),
        MenuEntry::submenu(
            "Hide offline after",
            choice_entries_with_custom(
                "offline",
                &HIDE_OFFLINE_CHOICES,
                config.hide_offline_minutes,
                "Never",
                CUSTOM_OFFLINE_ID,
                format_minutes,
            ),
        ),
        MenuEntry::separator(),
    ]);
    if DETECT_NEW_AVAILABLE {
        entries.push(MenuEntry::check(
            DETECT_NEW_DEVICES_ID.to_owned(),
            "Detect new devices",
            config.detect_new_devices,
        ));
    }
    entries.extend([
        login_entry(login),
        MenuEntry::separator(),
        MenuEntry::check(
            AUTO_UPDATE_ID.to_owned(),
            "Check for updates automatically",
            config.check_updates,
        ),
    ]);
    entries
}

/// The line that announces a newer release; the caller puts it at the top of the menu.
pub fn update_available_entries(version: &str) -> [MenuEntry; 2] {
    [
        MenuEntry::item(OPEN_UPDATE_ID, format!("Update available: v{version}…")),
        MenuEntry::separator(),
    ]
}

/// Device lines indented under a disabled group header, a separator between groups; `devices`
/// arrive already grouped by kind, so a header marks each change of kind.
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
        let mut entry = device_entry(device, config, now);
        entry.text.insert_str(0, GROUP_INDENT);
        entries.push(entry);
    }
    entries
}

/// The device line opens a submenu: an info line, the strip switches, then icon and colour;
/// all but "Show on strip" grey out while the device is hidden from the strip.
fn device_entry(device: &DeviceView, config: &Config, now: SystemTime) -> MenuEntry {
    let id = &device.id;
    let options = config.display_options(id);
    let is_svg = matches!(options.icon, IconChoice::Svg(_));
    // The AirPods entry only appears for devices whose name maps to a system symbol.
    let has_symbol = symbol_for_device(&device.name, device.kind).is_some();
    let shown = !config.is_hidden(id, &device.name);
    let when_shown = |entry: MenuEntry| MenuEntry {
        enabled: shown,
        ..entry
    };
    let children = vec![
        MenuEntry::label(device_info(device, now)),
        MenuEntry::separator(),
        MenuEntry::check(format!("hide:{id}"), "Show on strip", shown),
        when_shown(MenuEntry::check(
            format!("pct:{id}"),
            "Show percentage",
            options.show_percent,
        )),
        when_shown(MenuEntry::check(
            format!("label:{id}"),
            "Show name under icon",
            options.show_label,
        )),
        MenuEntry::separator(),
        when_shown(MenuEntry::submenu(
            "Icon",
            icon_entries(id, &options.icon, has_symbol),
        )),
        when_shown(MenuEntry::submenu(
            "Icon colour",
            color_entries(id, (options.color_light, options.color_dark), is_svg),
        )),
    ];
    // A native menu cannot colour or grey a clickable line, so states are said in words.
    let mut line = menu_line(device, now);
    if needs_charge(device, config.low_battery_percent) {
        line.push_str(NEEDS_CHARGE_TEXT);
    }
    if device.is_offline() {
        line.push_str(DISCONNECTED_TEXT);
    }
    MenuEntry::submenu(line, children)
}

/// `Bluetooth · read 5m ago`: how the device connects and, when known, its reading's age; only a
/// connected device's reading time is reliable, as on the device line.
fn device_info(device: &DeviceView, now: SystemTime) -> String {
    let path = match device.path {
        Path::Bluetooth => "Bluetooth",
        Path::Dongle => "2.4 GHz receiver",
        Path::Cable => "USB cable",
    };
    // The same age rule as the device line, so the menu only changes when the age is worth saying.
    shown_age(device, now).map_or_else(|| path.to_owned(), |age| format!("{path} · read {age}"))
}

/// Tail of a device line at or below the low-battery threshold.
const NEEDS_CHARGE_TEXT: &str = " · needs charge";

/// Tail of an offline device line, which stays clickable so its settings still open.
const DISCONNECTED_TEXT: &str = " · disconnected";

/// Lead of a device line under its group header (two em spaces: menus keep them, unlike tabs).
const GROUP_INDENT: &str = "\u{2003}\u{2003}";

/// Built-in icons, `AirPods` on a device with a matching system symbol (or one already using a
/// symbol, so the tick shows), then `Custom SVG…`, the current choice ticked.
fn icon_entries(id: &str, current: &IconChoice, has_symbol: bool) -> Vec<MenuEntry> {
    let mut entries: Vec<MenuEntry> = BUILT_IN_ICONS
        .iter()
        .map(|&(label, kind)| {
            let (value, choice) = kind.map_or((ICON_AUTO, IconChoice::Auto), |kind| {
                (kind.as_str(), IconChoice::Kind(kind))
            });
            MenuEntry::check(format!("icon:{value}:{id}"), label, *current == choice)
        })
        .collect();
    let is_symbol = matches!(current, IconChoice::Symbol(_));
    if SYMBOLS_AVAILABLE && (has_symbol || is_symbol) {
        entries.push(MenuEntry::check(
            format!("icon:{ICON_SYMBOL}:{id}"),
            "AirPods",
            is_symbol,
        ));
    }
    entries.push(MenuEntry::check(
        format!("svg:{id}"),
        "Custom SVG…",
        matches!(current, IconChoice::Svg(_)),
    ));
    entries
}

/// Menu id value of a colour: `auto`, `original` or `RRGGBB`.
fn color_value(color: IconColor) -> String {
    match color {
        IconColor::Auto => "auto".to_owned(),
        IconColor::Original => "original".to_owned(),
        IconColor::Rgb(red, green, blue) => hex(red, green, blue),
    }
}

/// `Icon colour` entries for the (light, dark) pair: Auto, Original (SVG icons only), the
/// palette, then the `Custom` submenu of pickers; a custom colour shows its hex and is ticked.
fn color_entries(id: &str, current: (IconColor, IconColor), is_svg: bool) -> Vec<MenuEntry> {
    let (light, dark) = current;
    let pair_entry = |label: &str, pair: (IconColor, IconColor)| {
        MenuEntry::check(
            format!("color:{}:{}:{id}", color_value(pair.0), color_value(pair.1)),
            label,
            current == pair,
        )
    };
    let mut entries = vec![
        pair_entry("Auto", (IconColor::Auto, IconColor::Auto)),
        MenuEntry {
            enabled: is_svg,
            ..pair_entry("Original", (IconColor::Original, IconColor::Original))
        },
    ];
    entries.extend(
        PALETTE
            .iter()
            .map(|&(name, light, dark)| pair_entry(name, (light, dark))),
    );
    entries.push(MenuEntry::separator());
    // A palette pair is never two equal colours, so equal colours are one custom pick.
    let is_pair = PALETTE.iter().any(|&(_, l, d)| (l, d) == current);
    let custom = |prefix: &str, label: &str, color: IconColor, ticked: bool| {
        let shown = match color {
            IconColor::Rgb(red, green, blue) if ticked => {
                format!("{label} (#{})", hex(red, green, blue))
            }
            _ => label.to_owned(),
        };
        MenuEntry::check(format!("{prefix}:{id}"), shown, ticked)
    };
    let same_rgb = light == dark && matches!(light, IconColor::Rgb(..));
    let split = light != dark && !is_pair;
    let is_rgb = |color| matches!(color, IconColor::Rgb(..));
    entries.push(MenuEntry::submenu(
        CUSTOM_COLOR_LABEL,
        vec![
            custom("ccolor", CUSTOM_BOTH_LABEL, light, same_rgb),
            custom(
                "ccolor-light",
                CUSTOM_LIGHT_LABEL,
                light,
                split && is_rgb(light),
            ),
            custom(
                "ccolor-dark",
                CUSTOM_DARK_LABEL,
                dark,
                split && is_rgb(dark),
            ),
        ],
    ));
    entries
}

/// Auto-refresh quick choices, then `Custom…`.
fn refresh_entries(current: Option<u32>) -> Vec<MenuEntry> {
    choice_entries_with_custom(
        "auto",
        &REFRESH_CHOICES,
        current,
        CHOICE_OFF_LABEL,
        CUSTOM_REFRESH_ID,
        |seconds| {
            if REFRESH_CHOICES.contains(&Some(seconds)) {
                format!("{} min", seconds / SECONDS_PER_MINUTE)
            } else {
                format_interval(seconds)
            }
        },
    )
}

/// Quick choices, then `Custom…` showing and ticking a value that is not one of them.
fn choice_entries_with_custom<T: Copy + PartialEq + Display>(
    prefix: &str,
    choices: &[Option<T>],
    current: Option<T>,
    none_label: &str,
    custom_id: &str,
    label: impl Fn(T) -> String,
) -> Vec<MenuEntry> {
    let mut entries = choice_entries(prefix, choices, current, none_label, &label);
    let custom = current.filter(|_| !choices.contains(&current));
    entries.push(MenuEntry::check(
        custom_id.to_owned(),
        custom.map_or_else(
            || "Custom…".to_owned(),
            |value| format!("Custom… ({})", label(value)),
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
    let ink = tick_ink();
    for entry in entries {
        if let Err(error) = menu.append(to_item(entry, ink).as_ref()) {
            eprintln!("menu item failed: {error}");
        }
    }
    menu
}

/// Grey level of the tick bitmap for the apps theme, read once per menu build; only Windows
/// draws the tick itself.
fn tick_ink() -> u8 {
    #[cfg(windows)]
    {
        match crate::theme::apps_theme() {
            Theme::Dark => 0xF0,
            Theme::Light => 0x20,
        }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

fn to_item(entry: &MenuEntry, ink: u8) -> Box<dyn IsMenuItem> {
    if entry.is_separator() {
        return Box::new(PredefinedMenuItem::separator());
    }
    if !entry.children.is_empty() {
        let submenu = Submenu::new(&entry.text, entry.enabled);
        for child in &entry.children {
            if let Err(error) = submenu.append(to_item(child, ink).as_ref()) {
                eprintln!("menu item failed: {error}");
            }
        }
        return Box::new(submenu);
    }
    match (&entry.id, entry.checked) {
        (Some(id), Some(checked)) => check_item(id, entry, checked, ink),
        (Some(id), None) => Box::new(MenuItem::with_id(
            id.as_str(),
            &entry.text,
            entry.enabled,
            None,
        )),
        (None, _) => Box::new(MenuItem::new(&entry.text, entry.enabled, None)),
    }
}

/// A ticked item shows a thin tick as its bitmap: the native Windows 10 dark-menu check mark is
/// coarse, and a bitmap keeps the menu native (owner-drawn items turn the dark menu light).
#[cfg(windows)]
fn check_item(id: &str, entry: &MenuEntry, checked: bool, ink: u8) -> Box<dyn IsMenuItem> {
    let icon = checked
        .then(|| Icon::from_rgba(tick_rgba(TICK_PX, ink), TICK_PX, TICK_PX))
        .and_then(|icon| {
            icon.inspect_err(|error| eprintln!("tick icon failed: {error}"))
                .ok()
        });
    Box::new(IconMenuItem::with_id(
        id,
        &entry.text,
        entry.enabled,
        icon,
        None,
    ))
}

#[cfg(not(windows))]
fn check_item(id: &str, entry: &MenuEntry, checked: bool, _ink: u8) -> Box<dyn IsMenuItem> {
    Box::new(CheckMenuItem::with_id(
        id,
        &entry.text,
        entry.enabled,
        checked,
        None,
    ))
}

/// Side of the tick bitmap in pixels.
#[cfg(windows)]
const TICK_PX: u32 = 16;

/// RGBA pixels of an anti-aliased thin tick of grey level `ink` in a `size`-pixel square.
#[cfg(any(windows, test))]
fn tick_rgba(size: u32, ink: u8) -> Vec<u8> {
    let unit = size as f32 / 16.0;
    let [a, b, c] = [(3.5, 8.5), (6.5, 11.5), (12.5, 4.5)].map(|(x, y)| (x * unit, y * unit));
    let half_width = 0.6 * unit;
    (0..size * size)
        .flat_map(|index| {
            let point = ((index % size) as f32 + 0.5, (index / size) as f32 + 0.5);
            let distance = segment_distance(point, a, b).min(segment_distance(point, b, c));
            // Coverage falls off over one pixel at the stroke's edges.
            let coverage = (half_width + 0.5 - distance).clamp(0.0, 1.0);
            [ink, ink, ink, (coverage * 255.0).round() as u8]
        })
        .collect()
}

/// Distance from `p` to the segment `a`–`b`.
#[cfg(any(windows, test))]
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (x, y) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    (x * x + y * y).sqrt()
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

    /// The menu with no install under way, so tests index from the devices that lead it.
    fn entries(devices: &[DeviceView], config: &Config, login: LoginState) -> Vec<MenuEntry> {
        let all = menu_entries(devices, config, login, false, None, SystemTime::UNIX_EPOCH);
        // Settings items inline, so lookups find them beside the devices.
        all.into_iter()
            .flat_map(|e| {
                if e.text == SETTINGS_LABEL {
                    e.children
                } else {
                    vec![e]
                }
            })
            .collect()
    }

    /// The Settings submenu's entries.
    fn settings_of(all: Vec<MenuEntry>) -> Vec<MenuEntry> {
        all.into_iter()
            .find(|e| e.text == SETTINGS_LABEL)
            .expect("a Settings submenu")
            .children
    }

    /// (id, text) of each main-menu entry; a separator reads `(None, "")`.
    fn shape(all: &[MenuEntry]) -> Vec<(Option<&str>, &str)> {
        all.iter()
            .map(|e| (e.id.as_deref(), e.text.trim_start()))
            .collect()
    }

    /// Devices lead the menu; Refresh now, Add device (Windows) and Settings follow, then the
    /// update check and Quit.
    #[test]
    fn devices_lead_and_actions_follow() {
        let devices = [kinded("N9", DeviceKind::Headset)];
        let all = menu_entries(
            &devices,
            &Config::default(),
            LoginState::Disabled,
            false,
            None,
            SystemTime::UNIX_EPOCH,
        );
        let check = format!("Check for updates… (v{})", env!("CARGO_PKG_VERSION"));
        let mut expected = vec![
            (None, "Headset"),
            (None, "N9 — BT · 50%"),
            (None, ""),
            (Some("refresh"), "Refresh now"),
        ];
        if cfg!(windows) {
            expected.push((Some(ADD_DEVICE_ID), "Add device…"));
        }
        expected.extend([
            (None, SETTINGS_LABEL),
            (None, ""),
            (Some(CHECK_UPDATES_ID), check.as_str()),
            (Some("quit"), "Quit"),
        ]);
        assert_eq!(shape(&all), expected);
        assert!(all.iter().all(|e| e.enabled || e.id.is_none()));

        // With no device the empty label takes the devices' place.
        let empty = menu_entries(
            &[],
            &Config::default(),
            LoginState::Disabled,
            false,
            None,
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(empty[0].text, EMPTY_MENU_TEXT);
        assert!(empty[1].is_separator());
        assert_eq!(empty[2].id.as_deref(), Some("refresh"));
    }

    /// An install under way leads the menu with its step and greys "Check for updates…".
    #[test]
    fn update_progress_leads_and_disables_the_check() {
        let stage = UpdateStage::Installing("0.5.0".into());
        let all = menu_entries(
            &[],
            &Config::default(),
            LoginState::Disabled,
            false,
            Some(&stage),
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(all[0].text, "Installing v0.5.0…");
        assert!(!all[0].enabled && all[0].id.is_none());
        assert!(all[1].is_separator());
        assert_eq!(all[2].text, EMPTY_MENU_TEXT);
        let check = all
            .iter()
            .find(|e| e.id.as_deref() == Some(CHECK_UPDATES_ID))
            .unwrap();
        assert!(!check.enabled);
        #[cfg(target_os = "macos")]
        assert_eq!(
            UpdateStage::Homebrew("0.5.0".into()).text(),
            "Updating to v0.5.0 via Homebrew…"
        );
        #[cfg(windows)]
        assert_eq!(
            UpdateStage::Downloading("0.5.0".into()).text(),
            "Downloading v0.5.0…"
        );
    }

    #[test]
    fn settings_groups_are_separated() {
        let settings = settings_of(menu_entries(
            &[],
            &Config::default(),
            LoginState::Disabled,
            true,
            None,
            SystemTime::UNIX_EPOCH,
        ));
        let texts: Vec<&str> = settings.iter().map(|e| e.text.as_str()).collect();
        let mut expected = vec![
            "Auto-refresh",
            "Refresh on battery power",
            "",
            "Low-battery alert",
            "Hide offline after",
            "",
        ];
        if cfg!(windows) {
            expected.push("Detect new devices");
        }
        expected.extend(["Start at login", "", "Check for updates automatically"]);
        assert_eq!(texts, expected);
    }

    #[test]
    fn battery_refresh_item_shows_only_with_a_battery() {
        let on = Config {
            refresh_on_battery: true,
            ..Config::default()
        };
        let find = |config: &Config, has_battery| {
            settings_of(menu_entries(
                &[],
                config,
                LoginState::Disabled,
                has_battery,
                None,
                SystemTime::UNIX_EPOCH,
            ))
            .into_iter()
            .find(|e| e.id.as_deref() == Some(REFRESH_ON_BATTERY_ID))
        };
        assert!(find(&on, false).is_none());
        let item = find(&on, true).unwrap();
        assert_eq!(item.text, "Refresh on battery power");
        assert_eq!(item.checked, Some(true));
        assert_eq!(find(&Config::default(), true).unwrap().checked, Some(false));
        assert_eq!(
            action_for(REFRESH_ON_BATTERY_ID),
            Some(MenuAction::ToggleRefreshOnBattery)
        );
    }

    #[test]
    fn battery_refresh_item_sits_right_after_auto_refresh() {
        let entries = settings_of(menu_entries(
            &[],
            &Config::default(),
            LoginState::Disabled,
            true,
            None,
            SystemTime::UNIX_EPOCH,
        ));
        let auto = entries
            .iter()
            .position(|e| e.text == "Auto-refresh")
            .unwrap();
        assert_eq!(entries[auto + 1].id.as_deref(), Some(REFRESH_ON_BATTERY_ID));
    }

    #[test]
    fn add_device_sits_above_settings_and_detect_precedes_login_on_windows() {
        let build = |config: &Config| {
            menu_entries(
                &[],
                config,
                LoginState::Disabled,
                false,
                None,
                SystemTime::UNIX_EPOCH,
            )
        };
        let all = build(&Config::default());
        let settings = all.iter().position(|e| e.text == SETTINGS_LABEL).unwrap();
        // The Add device window sits between Refresh now and Settings, on Windows only.
        let add = all
            .iter()
            .position(|e| e.id.as_deref() == Some(ADD_DEVICE_ID));
        assert_eq!(add.is_some(), cfg!(windows));
        if let Some(add) = add {
            assert_eq!(add + 1, settings);
            assert_eq!(all[add].text, "Add device…");
            assert_eq!(all[add - 1].id.as_deref(), Some("refresh"));
        }

        let entries = settings_of(all);
        let detect = entries
            .iter()
            .position(|e| e.id.as_deref() == Some(DETECT_NEW_DEVICES_ID));
        assert_eq!(detect.is_some(), cfg!(windows));
        if let Some(detect) = detect {
            assert_eq!(entries[detect].text, "Detect new devices");
            assert_eq!(entries[detect].checked, Some(true));
            assert!(entries[detect - 1].is_separator());
            assert_eq!(entries[detect + 1].id.as_deref(), Some("login"));
            let off = Config {
                detect_new_devices: false,
                ..Config::default()
            };
            let unticked = settings_of(build(&off))
                .into_iter()
                .find(|e| e.id.as_deref() == Some(DETECT_NEW_DEVICES_ID))
                .unwrap();
            assert_eq!(unticked.checked, Some(false));
        }
    }

    #[test]
    fn update_items_sit_above_quit_and_map_to_actions() {
        let build = |config: &Config| {
            menu_entries(
                &[],
                config,
                LoginState::Disabled,
                false,
                None,
                SystemTime::UNIX_EPOCH,
            )
        };
        let entries = build(&Config::default());
        let ids: Vec<Option<&str>> = entries.iter().map(|e| e.id.as_deref()).collect();
        let check = ids.iter().position(|id| *id == Some("check-updates"));
        let quit = ids.iter().position(|id| *id == Some("quit")).unwrap();
        // A separator, "Check for updates…" (enabled with no install under way), then Quit.
        assert_eq!(check, Some(quit - 1));
        assert!(entries[quit - 2].is_separator());
        assert!(entries[quit - 1].enabled);
        assert_eq!(
            entries[quit - 1].text,
            format!("Check for updates… (v{})", env!("CARGO_PKG_VERSION"))
        );

        // The automatic-check switch follows the setting, on by default.
        let auto = |config: &Config| {
            settings_of(build(config))
                .into_iter()
                .find(|e| e.id.as_deref() == Some("update-auto"))
                .unwrap()
        };
        let on = auto(&Config::default());
        assert_eq!(on.text, "Check for updates automatically");
        assert_eq!(on.checked, Some(true));
        let off = Config {
            check_updates: false,
            ..Config::default()
        };
        assert_eq!(auto(&off).checked, Some(false));

        assert_eq!(
            action_for("check-updates"),
            Some(MenuAction::CheckForUpdates)
        );
        assert_eq!(
            action_for("update-auto"),
            Some(MenuAction::ToggleCheckUpdates)
        );
        assert_eq!(action_for("update-open"), Some(MenuAction::OpenUpdate));
    }

    #[test]
    fn update_available_line_is_one_clickable_item_and_a_separator() {
        let [line, separator] = update_available_entries("0.2.0");
        assert_eq!(line.id.as_deref(), Some("update-open"));
        assert_eq!(line.text, "Update available: v0.2.0…");
        assert!(line.enabled && line.children.is_empty());
        assert!(separator.is_separator());
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
        assert_eq!(action_for("low:custom"), Some(MenuAction::CustomLow));
        assert_eq!(action_for("low:7"), None);
        assert_eq!(
            action_for("offline:custom"),
            Some(MenuAction::CustomHideOffline)
        );
        assert_eq!(action_for("login"), Some(MenuAction::ToggleStartAtLogin));
        assert_eq!(
            action_for("detect-new"),
            Some(MenuAction::ToggleDetectNewDevices)
        );
        assert_eq!(action_for("add-device"), Some(MenuAction::AddDevice));
        assert_eq!(action_for("auto:7"), None);
        assert_eq!(action_for("icon:toaster:bt:x"), None);
        assert_eq!(action_for("color:zzz:auto:bt:x"), None);
        assert_eq!(action_for("color:3E8EED:bt:x"), None);
        assert_eq!(action_for("color:auto:bt:x"), None);
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
                action_for(&format!("label:{id}")),
                Some(MenuAction::ToggleLabel(owned.clone()))
            );
            assert_eq!(
                action_for(&format!("ccolor:{id}")),
                Some(MenuAction::CustomColor(owned.clone(), None))
            );
            assert_eq!(
                action_for(&format!("ccolor-light:{id}")),
                Some(MenuAction::CustomColor(owned.clone(), Some(Theme::Light)))
            );
            assert_eq!(
                action_for(&format!("ccolor-dark:{id}")),
                Some(MenuAction::CustomColor(owned.clone(), Some(Theme::Dark)))
            );
            assert_eq!(
                action_for(&format!("icon:mouse:{id}")),
                Some(MenuAction::SetIcon(
                    owned.clone(),
                    IconChoice::Kind(DeviceKind::Mouse)
                ))
            );
            assert_eq!(
                action_for(&format!("icon:symbol:{id}")),
                Some(MenuAction::SetSymbolIcon(owned.clone()))
            );
            assert_eq!(
                action_for(&format!("icon:auto:{id}")),
                Some(MenuAction::SetIcon(owned.clone(), IconChoice::Auto))
            );
            assert_eq!(
                action_for(&format!("color:2B7FE0:6AA8F5:{id}")),
                Some(MenuAction::SetColors(
                    owned.clone(),
                    IconColor::Rgb(0x2B, 0x7F, 0xE0),
                    IconColor::Rgb(0x6A, 0xA8, 0xF5)
                ))
            );
            assert_eq!(
                action_for(&format!("color:original:original:{id}")),
                Some(MenuAction::SetColors(
                    owned,
                    IconColor::Original,
                    IconColor::Original
                ))
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
        // Show on strip, Show percentage, Show name under icon, 7 built-in icons + Custom SVG (no
        // AirPods entry: the name maps to no symbol) and 12 colour entries (3 under Custom).
        assert_eq!(ids.len(), 23);
        for id in ids {
            assert!(action_for(id).is_some(), "{id}");
            assert!(id.ends_with(MAC_ID), "{id}");
        }
    }

    /// The `Icon` submenu of `name` whose icon is `icon`.
    fn icon_menu(name: &str, icon: IconChoice) -> MenuEntry {
        let mut device = device(name, Level::Percent(50), true);
        device.id = HEX_ID.to_owned();
        let mut config = Config::default();
        config.update_device(HEX_ID, name, |options| options.icon = icon);
        let entries = entries(&[device], &config, LoginState::Disabled);
        child(first_device(&entries), "Icon").clone()
    }

    /// The AirPods entry shows only for a device with a matching symbol, or while a symbol is set.
    #[test]
    fn airpods_entry_follows_device_symbol() {
        let has_entry = |menu: &MenuEntry| menu.children.iter().any(|e| e.text == "AirPods");
        let on_macos = cfg!(target_os = "macos");

        assert_eq!(
            has_entry(&icon_menu("AirPods Pro", IconChoice::Auto)),
            on_macos
        );
        assert!(!has_entry(&icon_menu("MX Master", IconChoice::Auto)));

        let ticked = icon_menu("MX Master", IconChoice::Symbol("airpods.pro".to_owned()));
        assert_eq!(has_entry(&ticked), on_macos);
        if on_macos {
            assert_eq!(ticked_ids(&ticked), [format!("icon:symbol:{HEX_ID}")]);
        }
    }

    #[test]
    fn device_submenus_have_no_battery_check() {
        // Unverified devices are added through the Add window, never queried from a row.
        assert_eq!(action_for("check:usb-391d-1a05"), None);
        let row = DeviceView {
            id: "usb-391d-1a05".to_owned(),
            path: Path::Dongle,
            ..device("usb-391d-1a05", Level::NoData("x".into()), true)
        };
        let entries = entries(&[row], &Config::default(), LoginState::Disabled);
        let mut stack: Vec<&MenuEntry> = entries.iter().collect();
        while let Some(entry) = stack.pop() {
            assert_ne!(entry.text, "Check battery now");
            assert!(
                !entry
                    .id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("check:")),
                "{:?}",
                entry.id
            );
            stack.extend(&entry.children);
        }
    }

    const BLUE: (IconColor, IconColor) = (
        IconColor::Rgb(0x2B, 0x7F, 0xE0),
        IconColor::Rgb(0x6A, 0xA8, 0xF5),
    );

    /// The `Icon colour` submenu of a device whose colours are `colors`.
    fn colour_menu(colors: (IconColor, IconColor), icon: IconChoice) -> MenuEntry {
        let mut device = device("MX Master", Level::Percent(50), true);
        device.id = HEX_ID.to_owned();
        let mut config = Config::default();
        config.update_device(HEX_ID, "MX Master", |options| {
            options.icon = icon;
            (options.color_light, options.color_dark) = colors;
        });
        let entries = entries(&[device], &config, LoginState::Disabled);
        child(first_device(&entries), "Icon colour").clone()
    }

    /// The `Custom` submenu of an `Icon colour` submenu.
    fn custom_of(colour: &MenuEntry) -> &MenuEntry {
        child(colour, CUSTOM_COLOR_LABEL)
    }

    fn ticked_labels(entry: &MenuEntry) -> Vec<&str> {
        let ticked = entry.children.iter().filter(|e| e.checked == Some(true));
        ticked.map(|e| e.text.as_str()).collect()
    }

    #[test]
    fn device_submenu_ticks_current() {
        let mut device = device("MX Master", Level::Percent(50), true);
        device.id = HEX_ID.to_owned();
        let mut config = Config::default();
        config.update_device(HEX_ID, "MX Master", |options| {
            *options = DisplayOptions {
                show_percent: false,
                show_label: false,
                icon: IconChoice::Kind(DeviceKind::Mouse),
                color_light: BLUE.0,
                color_dark: BLUE.1,
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
        assert_eq!(child(submenu, "Show name under icon").checked, Some(false));
        let colour = child(submenu, "Icon colour");
        assert_eq!(
            ticked_ids(colour),
            [format!("color:2B7FE0:6AA8F5:{HEX_ID}")]
        );
        assert_eq!(ticked_labels(colour), ["Blue"]);
        assert!(!child(colour, "Original").enabled);
    }

    /// Info line, strip switches, then icon and colour; all but "Show on strip" grey out while
    /// the device is hidden.
    #[test]
    fn device_submenu_layout_and_hidden_state() {
        let mut device = device("MX Master", Level::Percent(50), true);
        device.id = HEX_ID.to_owned();
        let submenu = |config: &Config| {
            first_device(&entries(
                std::slice::from_ref(&device),
                config,
                LoginState::Disabled,
            ))
            .clone()
        };
        let shown = submenu(&Config::default());
        let texts: Vec<&str> = shown.children.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Bluetooth",
                "",
                "Show on strip",
                "Show percentage",
                "Show name under icon",
                "",
                "Icon",
                "Icon colour",
            ]
        );
        assert!(!shown.children[0].enabled && shown.children[0].id.is_none());
        assert!(shown.children[2..].iter().all(|e| e.enabled));

        let mut config = Config::default();
        config.toggle_hidden(HEX_ID, "MX Master");
        let hidden = submenu(&config);
        assert_eq!(child(&hidden, "Show on strip").checked, Some(false));
        assert!(child(&hidden, "Show on strip").enabled);
        for text in [
            "Show percentage",
            "Show name under icon",
            "Icon",
            "Icon colour",
        ] {
            assert!(!child(&hidden, text).enabled, "{text}");
        }
    }

    /// The info line names the path in full and omits the age when it is not known.
    #[test]
    fn device_info_line() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3 * 3600);
        let mut mouse = device("Mouse", Level::Percent(40), true);
        mouse.path = Path::Dongle;
        mouse.updated_at = SystemTime::UNIX_EPOCH;
        assert_eq!(device_info(&mouse, now), "2.4 GHz receiver · read 3h ago");
        mouse.connected = false;
        assert_eq!(device_info(&mouse, now), "2.4 GHz receiver");
        let mut cable = device("Pad", Level::NoData("x".into()), true);
        cable.path = Path::Cable;
        assert_eq!(device_info(&cable, now), "USB cable");
    }

    #[test]
    fn generic_icon_keeps_the_other_kind_id() {
        let menu = icon_menu("MX Master", IconChoice::Kind(DeviceKind::Other));
        let generic = child(&menu, "Generic");
        assert_eq!(generic.checked, Some(true));
        assert_eq!(
            generic.id.as_deref(),
            Some(format!("icon:{}:{HEX_ID}", DeviceKind::Other.as_str()).as_str())
        );
        assert!(!menu.children.iter().any(|e| e.text == "Bluetooth"));
    }

    #[test]
    fn colour_menu_layout() {
        let colour = colour_menu((IconColor::Auto, IconColor::Auto), IconChoice::Auto);
        let labels: Vec<&str> = colour.children.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Auto",
                "Original",
                "Red",
                "Orange",
                "Yellow",
                "Green",
                "Blue",
                "Purple",
                "Pink",
                "",
                CUSTOM_COLOR_LABEL,
            ]
        );
        assert!(colour.children[9].is_separator());
        assert_eq!(ticked_labels(&colour), ["Auto"]);
        let custom = custom_of(&colour);
        let labels: Vec<&str> = custom.children.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(labels, ["Both…", CUSTOM_LIGHT_LABEL, CUSTOM_DARK_LABEL]);
        assert!(ticked_labels(custom).is_empty());
    }

    #[test]
    fn colour_menu_ticks_original_for_svg() {
        let both = (IconColor::Original, IconColor::Original);
        let colour = colour_menu(both, IconChoice::Svg("a.svg".into()));
        assert!(child(&colour, "Original").enabled);
        assert_eq!(ticked_labels(&colour), ["Original"]);
    }

    #[test]
    fn colour_menu_ticks_each_palette_pair() {
        for &(name, light, dark) in &PALETTE {
            let colour = colour_menu((light, dark), IconChoice::Auto);
            assert_eq!(ticked_labels(&colour), [name]);
        }
    }

    #[test]
    fn colour_menu_ticks_custom_both() {
        let orange = IconColor::Rgb(0xFF, 0x88, 0x00);
        let colour = colour_menu((orange, orange), IconChoice::Auto);
        assert!(ticked_ids(&colour).is_empty());
        let custom = custom_of(&colour);
        assert_eq!(ticked_ids(custom), [format!("ccolor:{HEX_ID}")]);
        assert_eq!(ticked_labels(custom), ["Both… (#FF8800)"]);
    }

    #[test]
    fn colour_menu_ticks_per_theme_customs() {
        let orange = IconColor::Rgb(0xFF, 0x88, 0x00);
        let teal = IconColor::Rgb(0x00, 0x88, 0x88);
        let light_only = colour_menu((orange, IconColor::Auto), IconChoice::Auto);
        let light_only = custom_of(&light_only);
        assert_eq!(
            ticked_labels(light_only),
            [format!("{CUSTOM_LIGHT_LABEL} (#FF8800)")]
        );
        assert_eq!(ticked_ids(light_only), [format!("ccolor-light:{HEX_ID}")]);
        let both = colour_menu((orange, teal), IconChoice::Auto);
        assert_eq!(
            ticked_labels(custom_of(&both)),
            [
                format!("{CUSTOM_LIGHT_LABEL} (#FF8800)"),
                format!("{CUSTOM_DARK_LABEL} (#008888)")
            ]
        );
        let dark_only = colour_menu((IconColor::Auto, teal), IconChoice::Auto);
        assert_eq!(
            ticked_ids(custom_of(&dark_only)),
            [format!("ccolor-dark:{HEX_ID}")]
        );
    }

    /// WCAG 2.x relative luminance of an sRGB colour.
    fn luminance(color: IconColor) -> f64 {
        let IconColor::Rgb(red, green, blue) = color else {
            panic!("palette holds only RGB colours");
        };
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
    }

    fn contrast(a: IconColor, b: IconColor) -> f64 {
        let (hi, lo) = (
            luminance(a).max(luminance(b)),
            luminance(a).min(luminance(b)),
        );
        (hi + 0.05) / (lo + 0.05)
    }

    /// Each palette shade reads (>= 3:1) on the taskbar it is chosen for.
    #[test]
    fn palette_pairs_meet_contrast_on_their_bar() {
        let light_bar = IconColor::Rgb(0xEE, 0xEE, 0xEE);
        let dark_bar = IconColor::Rgb(0x1F, 0x1F, 0x1F);
        for &(name, light, dark) in &PALETTE {
            assert!(contrast(light, light_bar) >= 3.0, "{name} light");
            assert!(contrast(dark, dark_bar) >= 3.0, "{name} dark");
            assert_ne!(
                light, dark,
                "{name} must differ so it is never a custom pick"
            );
        }
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
        let quick_entries = settings_of(menu_entries(
            &[],
            &quick,
            LoginState::Disabled,
            false,
            None,
            SystemTime::UNIX_EPOCH,
        ));
        let auto = quick_entries
            .iter()
            .find(|e| e.text == "Auto-refresh")
            .unwrap();
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
    fn disconnected_device_line_is_marked_offline_but_clickable() {
        let devices = [
            device("Here", Level::Percent(60), true),
            device("Away", Level::Percent(40), false),
        ];
        let entries = entries(&devices, &Config::default(), LoginState::Disabled);
        // entries[0] is the group header; both lines stay in their kind group.
        assert!(entries[1].enabled && entries[1].text.starts_with(GROUP_INDENT));
        let away = &entries[2];
        assert!(away.enabled && !away.children.is_empty());
        assert!(away.text.starts_with(GROUP_INDENT));
        assert!(away.text[GROUP_INDENT.len()..].starts_with("Away — BT · 40%"));
        assert!(away.text.ends_with(DISCONNECTED_TEXT));
        assert!(!entries[1].text.contains(DISCONNECTED_TEXT));
    }

    #[test]
    fn stale_device_line_is_marked_offline_and_keeps_its_text() {
        let mut stale = device("Mouse", Level::Percent(70), true);
        stale.stale = true;
        let fresh = device("Mouse", Level::Percent(70), true);
        let entries_for = |d: DeviceView| entries(&[d], &Config::default(), LoginState::Disabled);
        let (stale_entries, fresh_entries) = (entries_for(stale), entries_for(fresh));
        let (stale_line, fresh_line) = (first_device(&stale_entries), first_device(&fresh_entries));
        assert!(stale_line.enabled && stale_line.text.ends_with(DISCONNECTED_TEXT));
        assert!(fresh_line.enabled && !fresh_line.text.contains(DISCONNECTED_TEXT));
        let text = stale_line.text.strip_prefix(GROUP_INDENT).unwrap();
        assert!(text.starts_with("Mouse — BT · 70%"));
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
        assert_eq!(
            labels,
            ["Never", "5 min", "15 min", "30 min", "1 h", "Custom…"]
        );
        assert_eq!(ticked_ids(&default), ["offline:15"]);
        assert_eq!(ticked_ids(&find(None)), ["offline:off"]);
        assert_eq!(ticked_ids(&find(Some(60))), ["offline:60"]);
        // Every built id parses back to the choice it ticks.
        for entry in &default.children {
            assert!(action_for(entry.id.as_deref().unwrap()).is_some());
        }
    }

    #[test]
    fn custom_low_and_offline_entries_tick_and_label() {
        let submenu = |config: &Config, title: &str| {
            entries(&[], config, LoginState::Disabled)
                .into_iter()
                .find(|e| e.text == title)
                .unwrap()
        };
        let config = Config {
            low_battery_percent: Some(7),
            hide_offline_minutes: Some(90),
            ..Config::default()
        };
        let low = submenu(&config, "Low-battery alert");
        assert_eq!(ticked_ids(&low), [CUSTOM_LOW_ID]);
        assert_eq!(child(&low, "Custom… (7%)").checked, Some(true));
        let offline = submenu(&config, "Hide offline after");
        assert_eq!(ticked_ids(&offline), [CUSTOM_OFFLINE_ID]);
        assert_eq!(child(&offline, "Custom… (90 min)").checked, Some(true));
        // Whole hours outside the quick choices read as hours.
        let two_hours = Config {
            hide_offline_minutes: Some(120),
            ..Config::default()
        };
        let offline = submenu(&two_hours, "Hide offline after");
        assert_eq!(child(&offline, "Custom… (2 h)").checked, Some(true));

        // A quick choice ticks itself, never Custom…; Off ticks Off.
        for config in [
            Config::default(),
            Config {
                low_battery_percent: None,
                hide_offline_minutes: None,
                ..Config::default()
            },
        ] {
            for title in ["Low-battery alert", "Hide offline after"] {
                let menu = submenu(&config, title);
                assert_eq!(child(&menu, "Custom…").checked, Some(false), "{title}");
                assert_eq!(ticked_ids(&menu).len(), 1, "{title}");
            }
        }
        // Every built id parses back to an action.
        for title in ["Low-battery alert", "Hide offline after"] {
            for entry in &submenu(&config, title).children {
                assert!(action_for(entry.id.as_deref().unwrap()).is_some());
            }
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
                    e.text.trim_start(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (true, false, "Headset"),
                (false, false, "N9 — BT · 50%"),
                (false, false, "V9 — BT · 50%"),
                (false, true, ""),
                (true, false, "Mouse"),
                (false, false, "X1 — BT · 50%"),
                (false, true, ""),
                (true, false, "Controller"),
                (false, false, "Pad — BT · 50%"),
            ]
        );
        // A separator ends the devices; Refresh now, Add device (Windows), then Settings items.
        assert!(entries[9].is_separator());
        assert_eq!(entries[10].id.as_deref(), Some("refresh"));
        let next = if cfg!(windows) {
            assert_eq!(entries[11].id.as_deref(), Some(ADD_DEVICE_ID));
            12
        } else {
            11
        };
        assert_eq!(entries[next].text, "Auto-refresh");
    }

    #[test]
    fn tick_is_a_thin_stroke() {
        let rgba = tick_rgba(16, 0xF0);
        let alpha = |x: u32, y: u32| rgba[((y * 16 + x) * 4 + 3) as usize];
        assert_eq!(rgba.len(), 16 * 16 * 4);
        assert!(alpha(6, 11) > 200, "the tick's corner is drawn");
        assert_eq!(alpha(1, 1), 0, "the corner of the square is empty");
        assert_eq!(alpha(10, 12), 0, "below the long stroke is empty");
        assert!(rgba.chunks(4).all(|px| px[0] == 0xF0));
    }

    #[test]
    fn low_device_line_says_needs_charge() {
        let config = Config {
            low_battery_percent: Some(20),
            ..Config::default()
        };
        let mut charging = device("Charging", Level::Percent(10), true);
        charging.charging = true;
        let devices = [
            device("Low", Level::Percent(20), true),
            device("Fine", Level::Percent(21), true),
            device("Away", Level::Percent(5), false),
            charging,
        ];
        let entries = entries(&devices, &config, LoginState::Disabled);
        let line = |name: &str| {
            let prefix = format!("{name} — ");
            let entry = entries
                .iter()
                .find(|e| e.text.trim_start().contains(&prefix));
            entry.unwrap().text.clone()
        };
        assert!(line("Low").ends_with(NEEDS_CHARGE_TEXT));
        for quiet in ["Fine", "Away", "Charging"] {
            assert!(!line(quiet).contains("needs charge"), "{quiet}");
        }
    }

    #[test]
    fn single_group_has_one_header_and_no_inner_separator() {
        let devices = [kinded("N9", DeviceKind::Headset)];
        let entries = entries(&devices, &Config::default(), LoginState::Disabled);
        assert_eq!(entries[0].text, "Headset");
        assert!(!entries[0].enabled);
        assert!(!entries[1].is_separator());
        assert!(entries[1].text.starts_with(GROUP_INDENT));
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
        // A disconnected device with a reading stays (marked offline); one without never lists.
        assert_eq!(lines, ["Connected no data", "Connected", "Away with level"]);
    }
}
