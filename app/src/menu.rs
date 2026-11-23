use std::fmt::Display;
use std::str::FromStr;
use std::time::SystemTime;

use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use ui_model::{DeviceView, EMPTY_MENU_TEXT, menu_line};

use crate::config::{Config, LOW_CHOICES, REFRESH_CHOICES};
use crate::login::LoginState;

const CHOICE_OFF: &str = "off";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Refresh,
    SetRefresh(Option<u32>),
    SetLow(Option<u8>),
    ToggleHidden(String),
    ToggleShowDisconnected,
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

    fn submenu(text: &str, children: Vec<MenuEntry>) -> Self {
        Self {
            children,
            ..Self::new(None, text)
        }
    }

    fn is_separator(&self) -> bool {
        self.id.is_none() && self.text.is_empty()
    }
}

/// Maps a clicked menu id back to its action; unknown ids give `None`.
pub fn action_for(id: &str) -> Option<MenuAction> {
    match id {
        "refresh" => Some(MenuAction::Refresh),
        "disc" => Some(MenuAction::ToggleShowDisconnected),
        "login" => Some(MenuAction::ToggleStartAtLogin),
        "quit" => Some(MenuAction::Quit),
        _ => {
            if let Some(rest) = id.strip_prefix("auto:") {
                choice_from_id(rest, &REFRESH_CHOICES).map(MenuAction::SetRefresh)
            } else if let Some(rest) = id.strip_prefix("low:") {
                choice_from_id(rest, &LOW_CHOICES).map(MenuAction::SetLow)
            } else {
                id.strip_prefix("hide:")
                    .map(|name| MenuAction::ToggleHidden(name.to_owned()))
            }
        }
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
/// reading when the user asked to see them (a paired Mac or phone has none, so never shows).
pub fn listed_devices<'a>(
    devices: &'a [DeviceView],
    config: &'a Config,
) -> impl Iterator<Item = &'a DeviceView> {
    devices.iter().filter(|device| {
        device.connected || (config.show_disconnected && device.shown_percent().is_some())
    })
}

/// The whole menu as plain data, rebuilt after every result or action.
pub fn menu_entries(
    devices: &[DeviceView],
    config: &Config,
    login: LoginState,
    now: SystemTime,
) -> Vec<MenuEntry> {
    let mut entries: Vec<MenuEntry> = listed_devices(devices, config)
        .map(|device| {
            MenuEntry::check(
                format!("hide:{}", device.name),
                menu_line(device, now),
                !config.hidden.contains(&device.name),
            )
        })
        .collect();
    if entries.is_empty() {
        entries.push(MenuEntry::label(EMPTY_MENU_TEXT));
    }
    entries.extend([
        MenuEntry::separator(),
        MenuEntry::item("refresh", "Refresh now"),
        MenuEntry::submenu(
            "Auto-refresh",
            choice_entries(
                "auto",
                &REFRESH_CHOICES,
                config.refresh_minutes,
                |minutes| format!("{minutes} min"),
            ),
        ),
        MenuEntry::submenu(
            "Low-battery alert",
            choice_entries("low", &LOW_CHOICES, config.low_battery_percent, |percent| {
                format!("{percent}%")
            }),
        ),
        MenuEntry::check(
            "disc".to_owned(),
            "Show disconnected devices",
            config.show_disconnected,
        ),
        login_entry(login),
        MenuEntry::separator(),
        MenuEntry::item("quit", "Quit"),
    ]);
    entries
}

/// One ticked entry per submenu; muda has no radio items, so the menu is rebuilt instead.
fn choice_entries<T: Copy + PartialEq + Display>(
    prefix: &str,
    choices: &[Option<T>],
    current: Option<T>,
    label: impl Fn(T) -> String,
) -> Vec<MenuEntry> {
    choices
        .iter()
        .map(|&choice| {
            MenuEntry::check(
                choice_id(prefix, choice),
                choice.map_or_else(|| "Off".to_owned(), &label),
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
    use ui_model::{DeviceKind, Level, Path};

    use super::*;

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
        }
    }

    fn entries(devices: &[DeviceView], config: &Config, login: LoginState) -> Vec<MenuEntry> {
        menu_entries(devices, config, login, SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn menu_ids_roundtrip() {
        assert_eq!(action_for("auto:off"), Some(MenuAction::SetRefresh(None)));
        assert_eq!(action_for("auto:5"), Some(MenuAction::SetRefresh(Some(5))));
        assert_eq!(action_for("low:20"), Some(MenuAction::SetLow(Some(20))));
        assert_eq!(
            action_for("hide:FIIL Atom"),
            Some(MenuAction::ToggleHidden("FIIL Atom".into()))
        );
        assert_eq!(action_for("login"), Some(MenuAction::ToggleStartAtLogin));
        assert_eq!(action_for("auto:7"), None);
        assert_eq!(action_for("nope"), None);
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
            refresh_minutes: Some(5),
            ..Config::default()
        };
        let entries = entries(&[], &config, LoginState::Disabled);
        let auto = entries.iter().find(|e| e.text == "Auto-refresh").unwrap();
        let ticked: Vec<_> = auto
            .children
            .iter()
            .filter(|child| child.checked == Some(true))
            .collect();
        assert_eq!(ticked.len(), 1);
        assert_eq!(ticked[0].id.as_deref(), Some("auto:5"));
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
    fn menu_device_lines_filter() {
        let devices = [
            device("Connected no data", Level::NoData("x".into()), true),
            device("Connected", Level::Percent(60), true),
            device("Away with level", Level::Percent(40), false),
            device("Paired Mac", Level::NoData("x".into()), false),
        ];
        let lines = |show_disconnected| -> Vec<String> {
            let config = Config {
                show_disconnected,
                ..Config::default()
            };
            entries(&devices, &config, LoginState::Disabled)
                .into_iter()
                .filter_map(|e| e.id?.strip_prefix("hide:").map(str::to_owned))
                .collect()
        };
        assert_eq!(lines(false), ["Connected no data", "Connected"]);
        assert_eq!(
            lines(true),
            ["Connected no data", "Connected", "Away with level"]
        );
    }
}
