use std::collections::HashMap;
use std::process::ExitCode;
#[cfg(windows)]
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::SystemTime;

#[cfg(target_os = "macos")]
use strip_render::TextStyle;
use strip_render::{Bitmap, Theme};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
#[cfg(target_os = "macos")]
use tray_icon::menu::Menu;
use tray_icon::menu::MenuEvent;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use ui_model::{
    DeviceView, DisplayOptions, EMPTY_MENU_TEXT, IconChoice, IconColor, Segment, build_strip,
    sort_weakest_first,
};

use crate::alerts::{AlertState, notify_low_battery};
use crate::config::{self, Config};
use crate::interval::{format_interval, parse_interval};
use crate::login::{self, LoginState};
use crate::menu::{self, MenuAction, MenuEntry};
use crate::refresh::{self, Command};
use crate::svg_cache::SvgCache;
#[cfg(windows)]
use crate::taskbar::{self, StripContent, TaskbarHandle};
#[cfg(target_os = "macos")]
use crate::ui_font;
use crate::{dialog, instance, log, theme};

/// The tray icon reads at 22 pt; twice that gives sharp pixels on Retina bars.
#[cfg(target_os = "macos")]
const MAC_STRIP_HEIGHT_PX: u32 = 44;
/// Longest tooltip the Windows tray accepts, in UTF-16 units.
#[cfg(windows)]
const WINDOWS_TOOLTIP_MAX_UTF16: usize = 127;

const BLUETOOTH_PERMISSION_HINT: &str = "Allow Bluetooth in System Settings › Privacy & Security";

const REFRESH_PROMPT_TITLE: &str = "Custom auto-refresh";
const REFRESH_PROMPT: &str = "How often to read the batteries, from 5s to 24h: 45s, 2m30s, 1h30m.";
const COLOR_PROMPT_TITLE: &str = "Custom icon colour";
const COLOR_PROMPT: &str = "Type a colour as #RRGGBB, for example #FF8800.";
const COLOR_HINT: &str = "That is not a colour. Type #RRGGBB, for example #FF8800.";

pub enum UserEvent {
    Devices(Vec<DeviceView>),
    /// A menu check got a valid reading from this unconfirmed `VID:PID`.
    Allowed(u16, u16),
    Menu(MenuEvent),
    #[cfg(target_os = "macos")]
    ThemeChanged,
    #[cfg(windows)]
    Taskbar(taskbar::Signal),
    /// A left or right click released on the tray icon: opens the menu thread's menu.
    #[cfg(windows)]
    TrayClick,
}

impl UserEvent {
    /// The variant name, for the trace.
    fn name(&self) -> &'static str {
        match self {
            Self::Devices(_) => "Devices",
            Self::Allowed(..) => "Allowed",
            Self::Menu(_) => "Menu",
            #[cfg(target_os = "macos")]
            Self::ThemeChanged => "ThemeChanged",
            #[cfg(windows)]
            Self::Taskbar(taskbar::Signal::KeeperTick) => "Taskbar::KeeperTick",
            #[cfg(windows)]
            Self::Taskbar(taskbar::Signal::Click) => "Taskbar::Click",
            #[cfg(windows)]
            Self::TrayClick => "TrayClick",
        }
    }
}

/// Runs the event loop until Quit; never returns.
pub fn run() -> ExitCode {
    if instance::another_instance_running() {
        eprintln!("juicetray is already running");
        return ExitCode::SUCCESS;
    }
    // After the instance check, so a refused second copy never restarts the running one's log.
    log::init();

    let event_loop = new_event_loop();
    let proxy = event_loop.create_proxy();
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        // A closed event loop means the app is exiting.
        let _ = menu_proxy.send_event(UserEvent::Menu(event));
    }));
    #[cfg(windows)]
    forward_tray_clicks(proxy.clone());
    #[cfg(target_os = "macos")]
    theme::watch_changes(proxy.clone());

    let mut app: Option<App> = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // The tray icon can only be created once the loop runs.
            Event::NewEvents(StartCause::Init) => {
                let _span = log::span("App::new");
                match App::new(proxy.clone()) {
                    Ok(created) => app = Some(created),
                    Err(error) => {
                        eprintln!("start failed: {error:#}");
                        log::line(&format!("start failed: {error:#}"));
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
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
    /// The menu currently attached to the status item; Windows builds one per show instead.
    #[cfg(target_os = "macos")]
    current_menu: Menu,
    config: Config,
    devices: Vec<DeviceView>,
    has_read: bool,
    alerts: AlertState,
    worker: Sender<Command>,
    shown_strip: Option<(Vec<Segment>, Theme)>,
    shown_tooltip: String,
    shown_entries: Vec<MenuEntry>,
    svgs: SvgCache,
    /// The menu bar font, read once at start.
    #[cfg(target_os = "macos")]
    text: TextStyle,
    /// Kept alive while it is the app's (hidden) main menu.
    #[cfg(target_os = "macos")]
    _edit_menu: Option<Menu>,
    /// `None` when the taskbar thread, its window classes or keeper timer could not be set up.
    #[cfg(windows)]
    taskbar: Option<TaskbarHandle>,
    /// Strip, theme and tooltip last handed to the taskbar strip.
    #[cfg(windows)]
    shown_taskbar: Option<(Vec<Segment>, Theme, String)>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> anyhow::Result<Self> {
        let config = Config::load();
        // Windows attaches no menu: tray-icon would track it on this thread, where a stuck
        // menu stops the app; clicks open it on the menu thread instead.
        #[cfg(target_os = "macos")]
        let current_menu = menu::build_menu(&[]);
        let builder = TrayIconBuilder::new();
        #[cfg(target_os = "macos")]
        let builder = builder.with_menu(Box::new(current_menu.clone()));
        // Windows: the tray shows the app icon (resource 1); levels live on the strip.
        #[cfg(windows)]
        let builder = builder.with_icon(Icon::from_resource(1, None)?);
        let tray = builder.build()?;
        // The UI heartbeat timer is this thread's own, so the watchdog runs even without a strip.
        #[cfg(windows)]
        crate::watchdog::start();
        #[cfg(windows)]
        let taskbar = new_taskbar_strip(proxy.clone());
        let worker = refresh::spawn_worker(proxy, config.refresh_seconds)?;
        let mut app = Self {
            tray,
            #[cfg(target_os = "macos")]
            current_menu,
            config,
            devices: Vec::new(),
            has_read: false,
            alerts: AlertState::default(),
            worker,
            shown_strip: None,
            shown_tooltip: String::new(),
            shown_entries: Vec::new(),
            svgs: SvgCache::default(),
            #[cfg(target_os = "macos")]
            text: ui_font::menu_bar_text(MAC_STRIP_HEIGHT_PX),
            #[cfg(target_os = "macos")]
            _edit_menu: install_edit_menu(),
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
        // Every handled event proves the UI thread is pumping.
        #[cfg(windows)]
        crate::watchdog::beat(crate::watchdog::Watched::Ui);
        let _span = log::span(format!("handle {}", event.name()));
        match event {
            UserEvent::Devices(devices) => self.on_devices(devices),
            // Config writes stay on this thread, so a check never races a settings save.
            UserEvent::Allowed(vid, pid) => {
                if let Err(error) = self.config.allow_and_save(vid, pid) {
                    eprintln!("saving config failed: {error:#}");
                }
            }
            UserEvent::Menu(event) => {
                // muda flips check items on click; a forced rebuild restores the config's ticks.
                self.shown_entries.clear();
                if let Some(action) = menu::action_for(event.id.as_ref()) {
                    log::line(&format!("menu action {action:?}"));
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
            UserEvent::Taskbar(taskbar::Signal::Click) | UserEvent::TrayClick => self.show_menu(),
        }
        self.apply();
        ControlFlow::Wait
    }

    fn on_devices(&mut self, mut devices: Vec<DeviceView>) {
        log::line(&format!("devices received {}", devices.len()));
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
        // Hand edits change `allow` in the file; the menu's "Check battery now" items follow it.
        self.config.allow = Config::load().allow;
    }

    fn on_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Refresh => self.tell_worker(Command::RefreshNow),
            MenuAction::SetRefresh(seconds) => self.set_refresh(seconds),
            MenuAction::CustomRefresh => self.prompt_refresh(),
            MenuAction::PickSvg(id) => self.pick_svg(&id),
            MenuAction::CustomColor(id) => self.prompt_color(&id),
            MenuAction::CheckBattery(id) => self.tell_worker(Command::CheckReceiver(id)),
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

    fn set_refresh(&mut self, seconds: Option<u32>) {
        self.config.refresh_seconds = seconds;
        self.tell_worker(Command::SetInterval(seconds));
        self.save_config();
    }

    /// Asks for a custom interval, pre-filled with the current one, until valid or cancelled.
    fn prompt_refresh(&mut self) {
        let current = self
            .config
            .refresh_seconds
            .map(format_interval)
            .unwrap_or_default();
        let _span = log::span("dialog refresh prompt");
        if let Some(seconds) = dialog::prompt_parsed(
            REFRESH_PROMPT_TITLE,
            REFRESH_PROMPT,
            &current,
            parse_interval,
        ) {
            self.set_refresh(Some(seconds));
        }
    }

    /// Asks for a `#RRGGBB` icon colour, pre-filled with the device's custom one.
    fn prompt_color(&mut self, id: &str) {
        let current = match self.config.display_options(id).color {
            IconColor::Rgb(red, green, blue) => format!("#{}", config::hex(red, green, blue)),
            IconColor::Auto | IconColor::Original => String::new(),
        };
        let _span = log::span("dialog color prompt");
        if let Some(color) =
            dialog::prompt_parsed(COLOR_PROMPT_TITLE, COLOR_PROMPT, &current, parse_hex_color)
        {
            self.update_device(id, |options| options.color = color);
        }
    }

    /// Lets the owner pick an SVG file as the device's icon.
    fn pick_svg(&mut self, id: &str) {
        let picked = {
            let _span = log::span("dialog svg picker");
            dialog::pick_svg()
        };
        let Some(path) = picked else {
            return;
        };
        let path = match path.into_os_string().into_string() {
            Ok(path) => path,
            Err(raw) => {
                let shown = std::path::Path::new(&raw).display().to_string();
                eprintln!("SVG path {shown} is not valid Unicode; icon unchanged");
                return;
            }
        };
        // Picking a file again re-reads it, so a fixed file replaces its earlier failure.
        self.svgs.forget(&path);
        self.force_redraw();
        self.update_device(id, |options| options.icon = IconChoice::Svg(path));
    }

    /// Makes the next `apply` render the strip even when its segments look unchanged.
    fn force_redraw(&mut self) {
        self.shown_strip = None;
        #[cfg(windows)]
        {
            self.shown_taskbar = None;
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
        log::line("tell worker");
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
        let _span = log::span("apply");
        self.apply_strip();

        let tooltip = self.tooltip(SystemTime::now());
        if tooltip != self.shown_tooltip {
            let _span = log::span("tooltip set");
            if let Err(error) = self.tray.set_tooltip(Some(&tooltip)) {
                eprintln!("setting tooltip failed: {error}");
            }
            self.shown_tooltip = tooltip;
        }

        let entries = self.entries(SystemTime::now());
        // Only macOS keeps a built menu; Windows builds it from the entries on each show.
        if entries != self.shown_entries {
            #[cfg(target_os = "macos")]
            {
                let _span = log::span("menu rebuild");
                self.current_menu = menu::build_menu(&entries);
                self.tray
                    .set_menu(Some(Box::new(self.current_menu.clone())));
            }
            self.shown_entries = entries;
        }

        #[cfg(windows)]
        self.sync_taskbar();
    }

    /// Re-reads the theme and rebuilds the tray icon when the strip or theme changed.
    fn apply_strip(&mut self) {
        let _span = log::span("apply_strip");
        let theme = theme::current_theme(&self.tray);
        let devices = self.visible_devices();
        // Resolved up front: loading an SVG updates the cache, which the strip builder cannot.
        let options: HashMap<String, DisplayOptions> = devices
            .iter()
            .map(|device| {
                let options = self.svgs.resolve(self.config.display_options(&device.id));
                (device.id.clone(), options)
            })
            .collect();
        // Hiding by id or old name is decided in `visible_devices`, so the strip gets no names.
        let segments = build_strip(
            &devices,
            self.config.low_battery_percent,
            &[],
            self.config.show_disconnected,
            &|id| options.get(id).cloned().unwrap_or_default(),
        );
        let strip = (segments, theme);
        if self.shown_strip.as_ref() != Some(&strip) {
            self.set_icon(&strip.0, theme);
            self.shown_strip = Some(strip);
        }
    }

    fn set_icon(&self, segments: &[Segment], theme: Theme) {
        let _span = log::span("tray icon render and set");
        let Some(bitmap) = self.tray_bitmap(segments, theme) else {
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
        let _span = log::span("menu entries");
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
    /// Keeper tick (the taskbar thread has already re-docked its windows): re-reads the theme,
    /// which re-renders the tray icon too, and sends the strip again when it changed.
    fn keep_taskbar(&mut self) {
        let _span = log::span("keep_taskbar");
        self.apply_strip();
        self.sync_taskbar();
    }

    /// Hands the strip to the taskbar thread, only when strip, theme or tooltip differ from
    /// what it shows; each window renders there at its own taskbar's height and DPI.
    fn sync_taskbar(&mut self) {
        let (Some(taskbar), Some((segments, theme))) =
            (self.taskbar.as_ref(), self.shown_strip.as_ref())
        else {
            return;
        };
        // The strip's own tooltip has no tray length cap, so it lists every device.
        let strip_tooltip = ui_model::tooltip_compact(&self.visible_devices(), usize::MAX);
        let unchanged = self.shown_taskbar.as_ref().is_some_and(|(s, t, tooltip)| {
            s == segments && t == theme && *tooltip == strip_tooltip
        });
        if unchanged {
            return;
        }
        taskbar.show(StripContent {
            segments: segments.clone(),
            theme: *theme,
            icons: Arc::clone(self.svgs.icons()),
            tooltip: strip_tooltip.clone(),
        });
        self.shown_taskbar = Some((segments.clone(), *theme, strip_tooltip));
    }

    /// Opens the live menu (from a strip or tray click) on the menu thread, so this thread keeps
    /// running even when the menu's modal loop gets stuck on another process's hung window.
    fn show_menu(&self) {
        // Opening the menu also refreshes; per-device floors still limit what is queried.
        self.tell_worker(Command::RefreshNow);
        crate::context_menu::show(self.entries(SystemTime::now()));
    }

    /// Drops the taskbar thread's handle, which waits until every copy is gone and Explorer's
    /// containers are restored, so nothing stays shrunk once the loop exits.
    fn hide_taskbar(&mut self) {
        self.taskbar = None;
    }
}

/// Turns a left or right button release on the tray icon into `UserEvent::TrayClick`, the same
/// clicks tray-icon's own menu answered.
#[cfg(windows)]
fn forward_tray_clicks(proxy: EventLoopProxy<UserEvent>) {
    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left | MouseButton::Right,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::TrayClick);
        }
    }));
}

/// Without a strip window the tray icon still shows the weakest number.
#[cfg(windows)]
fn new_taskbar_strip(proxy: EventLoopProxy<UserEvent>) -> Option<TaskbarHandle> {
    use std::sync::Mutex;

    // The callback must be `Sync`; the proxy is only `Send`.
    let proxy = Mutex::new(proxy);
    TaskbarHandle::new(move |signal| {
        if let Ok(proxy) = proxy.lock() {
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::Taskbar(signal));
        }
    })
    .inspect_err(|error| eprintln!("taskbar strip unavailable: {error:#}"))
    .ok()
}

#[cfg(target_os = "macos")]
impl App {
    /// macOS shows the whole strip in the menu bar.
    fn tray_bitmap(&self, segments: &[Segment], theme: Theme) -> Option<Bitmap> {
        Some(strip_render::render_strip(
            segments,
            MAC_STRIP_HEIGHT_PX,
            theme,
            self.svgs.icons(),
            &self.text,
        ))
    }
}

#[cfg(windows)]
impl App {
    /// The Windows tray keeps the app icon set at start; levels show on the taskbar strip.
    fn tray_bitmap(&self, _segments: &[Segment], _theme: Theme) -> Option<Bitmap> {
        None
    }
}

/// Accessory apps show no menu bar, but key equivalents still go through the main menu: a
/// hidden Edit menu makes Cmd+X/C/V/A/Z work in dialog text fields.
#[cfg(target_os = "macos")]
fn install_edit_menu() -> Option<Menu> {
    use tray_icon::menu::{PredefinedMenuItem, Submenu};

    let edit = Submenu::with_items(
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ],
    );
    match edit.and_then(|edit| Menu::with_items(&[&edit])) {
        Ok(menu) => {
            menu.init_for_nsapp();
            Some(menu)
        }
        Err(error) => {
            eprintln!("edit menu unavailable, so no copy/paste in dialogs: {error}");
            None
        }
    }
}

/// Only a `#RRGGBB` colour (the `#` is optional) counts as a custom colour.
fn parse_hex_color(text: &str) -> Result<IconColor, String> {
    match config::parse_color(text.trim()) {
        Some(color @ IconColor::Rgb(..)) => Ok(color),
        _ => Err(COLOR_HINT.to_owned()),
    }
}
