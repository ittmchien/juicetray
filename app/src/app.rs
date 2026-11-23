use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::mpsc::Sender;
use std::time::SystemTime;

use strip_render::{Bitmap, TextStyle, Theme};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{Menu, MenuEvent};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use ui_model::{
    DeviceView, DisplayOptions, EMPTY_MENU_TEXT, Segment, build_strip, sort_weakest_first,
};

use crate::alerts::{AlertState, notify_low_battery};
use crate::config::Config;
use crate::login::{self, LoginState};
use crate::menu::{self, MenuAction, MenuEntry};
use crate::refresh::{self, Command};
#[cfg(windows)]
use crate::taskbar::{self, TaskbarStrip};
use crate::{instance, theme};

/// The tray icon reads at 22 pt; twice that gives sharp pixels on Retina bars.
#[cfg(target_os = "macos")]
const MAC_STRIP_HEIGHT_PX: u32 = 44;
/// Square tray icon holding the weakest number.
#[cfg(windows)]
const WINDOWS_ICON_PX: u32 = 32;
/// Longest tooltip the Windows tray accepts, in UTF-16 units.
#[cfg(windows)]
const WINDOWS_TOOLTIP_MAX_UTF16: usize = 127;

const BLUETOOTH_PERMISSION_HINT: &str = "Allow Bluetooth in System Settings › Privacy & Security";

pub enum UserEvent {
    Devices(Vec<DeviceView>),
    Menu(MenuEvent),
    #[cfg(target_os = "macos")]
    ThemeChanged,
    #[cfg(windows)]
    Taskbar(taskbar::Signal),
}

/// Runs the event loop until Quit; never returns.
pub fn run() -> ExitCode {
    if instance::another_instance_running() {
        eprintln!("juicetray is already running");
        return ExitCode::SUCCESS;
    }

    let event_loop = new_event_loop();
    let proxy = event_loop.create_proxy();
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        // A closed event loop means the app is exiting.
        let _ = menu_proxy.send_event(UserEvent::Menu(event));
    }));
    #[cfg(target_os = "macos")]
    theme::watch_changes(proxy.clone());

    let mut app: Option<App> = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // The tray icon can only be created once the loop runs.
            Event::NewEvents(StartCause::Init) => match App::new(proxy.clone()) {
                Ok(created) => app = Some(created),
                Err(error) => {
                    eprintln!("start failed: {error:#}");
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::UserEvent(event) => {
                if let Some(app) = app.as_mut() {
                    *control_flow = app.handle(event);
                }
            }
            _ => {}
        }
    })
}

/// macOS runs as an accessory app: menu-bar item only, no Dock icon.
#[cfg(target_os = "macos")]
fn new_event_loop() -> EventLoop<UserEvent> {
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    event_loop
}

#[cfg(windows)]
fn new_event_loop() -> EventLoop<UserEvent> {
    EventLoopBuilder::<UserEvent>::with_user_event().build()
}

/// Everything the tray icon shows, plus the last values pushed to the OS.
struct App {
    tray: TrayIcon,
    /// The menu currently attached to the tray; the Windows strip opens the same one.
    current_menu: Menu,
    config: Config,
    devices: Vec<DeviceView>,
    has_read: bool,
    alerts: AlertState,
    worker: Sender<Command>,
    shown_strip: Option<(Vec<Segment>, Theme)>,
    shown_tooltip: String,
    shown_entries: Vec<MenuEntry>,
    /// `None` when the strip window class or keeper timer could not be set up.
    #[cfg(windows)]
    taskbar: Option<TaskbarStrip>,
    /// Strip, theme, bitmap height and tooltip last handed to the taskbar strip.
    #[cfg(windows)]
    shown_taskbar: Option<(Vec<Segment>, Theme, u32, String)>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> anyhow::Result<Self> {
        let config = Config::load();
        let current_menu = menu::build_menu(&[]);
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(current_menu.clone()))
            .build()?;
        #[cfg(windows)]
        let taskbar = new_taskbar_strip(proxy.clone());
        let worker = refresh::spawn_worker(proxy, config.refresh_seconds)?;
        let mut app = Self {
            tray,
            current_menu,
            config,
            devices: Vec::new(),
            has_read: false,
            alerts: AlertState::default(),
            worker,
            shown_strip: None,
            shown_tooltip: String::new(),
            shown_entries: Vec::new(),
            #[cfg(windows)]
            taskbar,
            #[cfg(windows)]
            shown_taskbar: None,
        };
        app.apply();
        Ok(app)
    }

    /// Handles one event and says whether the loop keeps running.
    fn handle(&mut self, event: UserEvent) -> ControlFlow {
        match event {
            UserEvent::Devices(devices) => self.on_devices(devices),
            UserEvent::Menu(event) => {
                // muda flips check items on click; a forced rebuild restores the config's ticks.
                self.shown_entries.clear();
                if let Some(action) = menu::action_for(event.id.as_ref()) {
                    if action == MenuAction::Quit {
                        #[cfg(windows)]
                        self.hide_taskbar();
                        return ControlFlow::Exit;
                    }
                    self.on_action(action);
                }
            }
            // `apply` below re-reads the theme.
            #[cfg(target_os = "macos")]
            UserEvent::ThemeChanged => {}
            // The keeper tick only needs the strip side; the menu and tooltip cannot have changed.
            #[cfg(windows)]
            UserEvent::Taskbar(taskbar::Signal::KeeperTick) => {
                self.keep_taskbar();
                return ControlFlow::Wait;
            }
            #[cfg(windows)]
            UserEvent::Taskbar(taskbar::Signal::Click) => self.show_menu_for_strip(),
        }
        self.apply();
        ControlFlow::Wait
    }

    fn on_devices(&mut self, mut devices: Vec<DeviceView>) {
        sort_weakest_first(&mut devices);
        // Every device is checked so its last reading stays current even while hidden.
        for device in &devices {
            let crossed = self.alerts.check(device, self.config.low_battery_percent);
            if crossed && !self.config.is_hidden(&device.id, &device.name) {
                notify_low_battery(device);
            }
        }
        self.devices = devices;
        self.has_read = true;
    }

    fn on_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Refresh => self.tell_worker(Command::RefreshNow),
            MenuAction::SetRefresh(seconds) => {
                self.config.refresh_seconds = seconds;
                self.tell_worker(Command::SetInterval(seconds));
                self.save_config();
            }
            // Dialog-driven actions are wired with the dialogs.
            MenuAction::CustomRefresh | MenuAction::PickSvg(_) | MenuAction::CustomColor(_) => {}
            MenuAction::SetLow(percent) => {
                self.config.low_battery_percent = percent;
                self.save_config();
            }
            MenuAction::ToggleHidden(id) => {
                let name = self.device_name(&id);
                self.config.toggle_hidden(&id, &name);
                self.save_config();
            }
            MenuAction::TogglePercent(id) => {
                self.update_device(&id, |options| options.show_percent = !options.show_percent);
            }
            MenuAction::SetIcon(id, icon) => self.update_device(&id, |options| options.icon = icon),
            MenuAction::SetColor(id, color) => {
                self.update_device(&id, |options| options.color = color);
            }
            MenuAction::ToggleShowDisconnected => {
                self.config.show_disconnected = !self.config.show_disconnected;
                self.save_config();
            }
            MenuAction::ToggleStartAtLogin => {
                let enable = login::login_state() != LoginState::Enabled;
                if let Err(error) = login::set_start_at_login(enable) {
                    eprintln!("start at login failed: {error:#}");
                }
            }
            // Handled by the caller, which stops the loop.
            MenuAction::Quit => {}
        }
    }

    /// The device's current name; empty when it vanished since the menu was built.
    fn device_name(&self, id: &str) -> String {
        self.devices
            .iter()
            .find(|device| device.id == id)
            .map(|device| device.name.clone())
            .unwrap_or_default()
    }

    fn update_device(&mut self, id: &str, change: impl FnOnce(&mut DisplayOptions)) {
        let name = self.device_name(id);
        self.config.update_device(id, &name, change);
        self.save_config();
    }

    fn tell_worker(&self, command: Command) {
        if self.worker.send(command).is_err() {
            eprintln!("refresh worker has stopped");
        }
    }

    fn save_config(&self) {
        if let Err(error) = self.config.save() {
            eprintln!("saving config failed: {error:#}");
        }
    }

    /// Pushes strip, tooltip and menu to the OS, each only when it differs from what is shown.
    fn apply(&mut self) {
        self.apply_strip();

        let tooltip = self.tooltip(SystemTime::now());
        if tooltip != self.shown_tooltip {
            if let Err(error) = self.tray.set_tooltip(Some(&tooltip)) {
                eprintln!("setting tooltip failed: {error}");
            }
            self.shown_tooltip = tooltip;
        }

        let entries = self.entries(SystemTime::now());
        if entries != self.shown_entries {
            self.current_menu = menu::build_menu(&entries);
            self.tray
                .set_menu(Some(Box::new(self.current_menu.clone())));
            self.shown_entries = entries;
        }

        #[cfg(windows)]
        self.sync_taskbar();
    }

    /// Re-reads the theme and rebuilds the tray icon when the strip or theme changed.
    fn apply_strip(&mut self) {
        let theme = theme::current_theme(&self.tray);
        // Hiding by id or old name is decided here, so the strip gets no name list.
        let segments = build_strip(
            &self.visible_devices(),
            self.config.low_battery_percent,
            &[],
            self.config.show_disconnected,
            &|id| self.config.display_options(id),
        );
        let strip = (segments, theme);
        if self.shown_strip.as_ref() != Some(&strip) {
            self.set_icon(&strip.0, theme);
            self.shown_strip = Some(strip);
        }
    }

    fn set_icon(&self, segments: &[Segment], theme: Theme) {
        let Some(bitmap) = tray_bitmap(segments, theme) else {
            return;
        };
        let icon = match Icon::from_rgba(bitmap.rgba, bitmap.width, bitmap.height) {
            Ok(icon) => icon,
            Err(error) => {
                eprintln!("building tray icon failed: {error}");
                return;
            }
        };
        if let Err(error) = self.tray.set_icon(Some(icon)) {
            eprintln!("setting tray icon failed: {error}");
        }
    }

    /// The menu, plus a permission hint when a bundled app reads no device at all.
    fn entries(&self, now: SystemTime) -> Vec<MenuEntry> {
        let mut entries =
            menu::menu_entries(&self.devices, &self.config, login::login_state(), now);
        if self.has_read && self.devices.is_empty() && instance::is_bundled() {
            entries.insert(1, MenuEntry::label(BLUETOOTH_PERMISSION_HINT));
        }
        entries
    }

    /// The devices the strip shows: listed in the menu and not hidden.
    fn visible_devices(&self) -> Vec<DeviceView> {
        menu::listed_devices(&self.devices, &self.config)
            .filter(|device| !self.config.is_hidden(&device.id, &device.name))
            .cloned()
            .collect()
    }

    #[cfg(target_os = "macos")]
    fn tooltip(&self, now: SystemTime) -> String {
        use ui_model::menu_line;

        let lines: Vec<String> = self
            .visible_devices()
            .iter()
            .map(|device| menu_line(device, now))
            .collect();
        if lines.is_empty() {
            EMPTY_MENU_TEXT.to_owned()
        } else {
            lines.join("\n")
        }
    }

    #[cfg(windows)]
    fn tooltip(&self, _now: SystemTime) -> String {
        let text = ui_model::tooltip_compact(&self.visible_devices(), WINDOWS_TOOLTIP_MAX_UTF16);
        if text.is_empty() {
            EMPTY_MENU_TEXT.to_owned()
        } else {
            text
        }
    }
}

#[cfg(windows)]
impl App {
    /// Re-reads the theme (which re-renders the tray icon too), then renders the taskbar strip
    /// again when it changed and re-docks the window.
    fn keep_taskbar(&mut self) {
        self.apply_strip();
        self.sync_taskbar();
        if let Some(taskbar) = self.taskbar.as_mut() {
            taskbar.keep();
        }
    }

    /// Renders the strip at the taskbar's current height, only when strip, theme, height or
    /// tooltip differ from what the taskbar window shows.
    fn sync_taskbar(&mut self) {
        let (Some(taskbar), Some((segments, theme))) =
            (self.taskbar.as_mut(), self.shown_strip.as_ref())
        else {
            return;
        };
        // Explorer is missing for now; the keeper recreates the window once it is back.
        let Some(height) = TaskbarStrip::strip_height() else {
            return;
        };
        let unchanged = self
            .shown_taskbar
            .as_ref()
            .is_some_and(|(s, t, h, tooltip)| {
                s == segments && t == theme && *h == height && *tooltip == self.shown_tooltip
            });
        if unchanged {
            return;
        }
        // Built-in icons and the embedded font: no custom SVGs or OS font are loaded here.
        let bitmap = strip_render::render_strip(
            segments,
            height,
            *theme,
            &HashMap::new(),
            &TextStyle::fallback(height),
        );
        taskbar.show(&bitmap, &self.shown_tooltip);
        self.shown_taskbar = Some((segments.clone(), *theme, height, self.shown_tooltip.clone()));
    }

    /// Opens the live menu from a strip click, anchored to the tray icon's own window.
    fn show_menu_for_strip(&self) {
        use tray_icon::menu::ContextMenu;

        // SAFETY: the tray icon's hidden top-level window stays valid as long as `self.tray`.
        let _ = unsafe {
            self.current_menu
                .show_context_menu_for_hwnd(self.tray.window_handle() as isize, None)
        };
    }

    fn hide_taskbar(&mut self) {
        if let Some(taskbar) = self.taskbar.as_mut() {
            taskbar.hide();
        }
    }
}

/// Without a strip window the tray icon still shows the weakest number.
#[cfg(windows)]
fn new_taskbar_strip(proxy: EventLoopProxy<UserEvent>) -> Option<TaskbarStrip> {
    use std::sync::Mutex;

    // The callback must be `Sync`; the proxy is only `Send`.
    let proxy = Mutex::new(proxy);
    let created = TaskbarStrip::new(move |signal| {
        if let Ok(proxy) = proxy.lock() {
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::Taskbar(signal));
        }
    });
    match created {
        Ok(strip) => Some(strip),
        Err(error) => {
            eprintln!("taskbar strip unavailable: {error:#}");
            None
        }
    }
}

/// macOS shows the whole strip in the menu bar.
#[cfg(target_os = "macos")]
fn tray_bitmap(segments: &[Segment], theme: Theme) -> Option<Bitmap> {
    // Built-in icons and the embedded font: no custom SVGs or OS font are loaded here.
    Some(strip_render::render_strip(
        segments,
        MAC_STRIP_HEIGHT_PX,
        theme,
        &HashMap::new(),
        &TextStyle::fallback(MAC_STRIP_HEIGHT_PX),
    ))
}

/// The Windows tray icon is one square with the weakest device's number.
#[cfg(windows)]
fn tray_bitmap(segments: &[Segment], theme: Theme) -> Option<Bitmap> {
    segments
        .first()
        .map(|weakest| strip_render::render_square(weakest, WINDOWS_ICON_PX, theme))
}
