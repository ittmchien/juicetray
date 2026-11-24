use std::collections::HashMap;
use std::process::ExitCode;
#[cfg(windows)]
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};

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
    sort_grouped,
};

#[cfg(windows)]
use crate::alerts::notify_update_available;
use crate::alerts::{AlertState, notify_low_battery};
use crate::config::{Config, parse_hide_offline_minutes, parse_low_percent};
use crate::interval::{format_interval, parse_interval};
use crate::login::{self, LoginState};
use crate::menu::{self, MenuAction, MenuEntry};
use crate::pause::{self, PowerSource, PowerState};
use crate::refresh::{self, Command};
use crate::svg_cache::SvgCache;
#[cfg(windows)]
use crate::taskbar::{self, StripContent, TaskbarHandle};
#[cfg(target_os = "macos")]
use crate::ui_font;
#[cfg(windows)]
use crate::update::check::{UpdateCheck, UpdateInfo};
use crate::{dialog, instance, log, theme, update};

/// The tray icon reads at 22 pt; twice that gives sharp pixels on Retina bars.
#[cfg(target_os = "macos")]
const MAC_STRIP_HEIGHT_PX: u32 = 44;
/// Longest tooltip the Windows tray accepts, in UTF-16 units.
#[cfg(windows)]
const WINDOWS_TOOLTIP_MAX_UTF16: usize = 127;

const BLUETOOTH_PERMISSION_HINT: &str = "Allow Bluetooth in System Settings › Privacy & Security";

const REFRESH_PROMPT_TITLE: &str = "Custom auto-refresh";
const REFRESH_PROMPT: &str = "How often to read the batteries, from 5s to 24h: 45s, 2m30s, 1h30m.";
const LOW_PROMPT_TITLE: &str = "Custom low-battery alert";
const LOW_PROMPT: &str = "Alert when a battery drops to this percent, from 1 to 99.";
const HIDE_OFFLINE_PROMPT_TITLE: &str = "Custom hide offline";
const HIDE_OFFLINE_PROMPT: &str =
    "Minutes before an offline device leaves the strip, from 1 to 1440.";
#[cfg(not(windows))]
const COLOR_PROMPT: &str = "Type a colour as #RRGGBB, for example #FF8800.";
#[cfg(not(windows))]
const COLOR_HINT: &str = "That is not a colour. Type #RRGGBB, for example #FF8800.";

#[cfg(windows)]
const BATTERY_CONFIRM_TITLE: &str = "Refresh on battery power";
#[cfg(windows)]
const BATTERY_CONFIRM: &str =
    "Auto-refresh while on battery power uses more of your laptop's battery. Turn it on?";
#[cfg(windows)]
const BATTERY_CONFIRM_OK: &str = "Turn on";

pub enum UserEvent {
    /// One read cycle's rows and the config file as the worker read it for that cycle.
    Devices(Vec<DeviceView>, Box<Config>),
    Menu(MenuEvent),
    #[cfg(target_os = "macos")]
    ThemeChanged,
    #[cfg(windows)]
    Taskbar(taskbar::Signal),
    /// A left or right click released on the tray icon: opens the menu thread's menu.
    #[cfg(windows)]
    TrayClick,
    /// The owner confirmed "Refresh on battery power" in its dialog, which runs off this thread.
    #[cfg(windows)]
    EnableRefreshOnBattery,
    /// A value prompt answered on its own thread; the owner's menu action runs on this one.
    #[cfg(windows)]
    Action(MenuAction),
    /// The update scheduler's tick: time for an automatic check.
    #[cfg(windows)]
    UpdateDue,
    /// An update check finished on its own thread; `manual` ones show their own dialog there.
    #[cfg(windows)]
    UpdateChecked {
        manual: bool,
        outcome: Result<UpdateCheck, String>,
    },
    /// The new exe is installed and running: this instance quits through the normal exit path.
    #[cfg(windows)]
    UpdateRestart,
}

impl UserEvent {
    /// The variant name, for the trace.
    fn name(&self) -> &'static str {
        match self {
            Self::Devices(..) => "Devices",
            Self::Menu(_) => "Menu",
            #[cfg(target_os = "macos")]
            Self::ThemeChanged => "ThemeChanged",
            #[cfg(windows)]
            Self::Taskbar(taskbar::Signal::ThemeChanged) => "Taskbar::ThemeChanged",
            #[cfg(windows)]
            Self::Taskbar(taskbar::Signal::PowerChanged) => "Taskbar::PowerChanged",
            #[cfg(windows)]
            Self::Taskbar(taskbar::Signal::Click) => "Taskbar::Click",
            #[cfg(windows)]
            Self::TrayClick => "TrayClick",
            #[cfg(windows)]
            Self::EnableRefreshOnBattery => "EnableRefreshOnBattery",
            #[cfg(windows)]
            Self::Action(_) => "Action",
            #[cfg(windows)]
            Self::UpdateDue => "UpdateDue",
            #[cfg(windows)]
            Self::UpdateChecked { .. } => "UpdateChecked",
            #[cfg(windows)]
            Self::UpdateRestart => "UpdateRestart",
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
    // The previous exe an update moved aside is deleted once nothing holds it.
    #[cfg(windows)]
    update::spawn_cleanup();

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
            // An offline device just crossed its "Hide offline after" deadline.
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(app) = app.as_mut() {
                    app.apply();
                }
            }
            _ => {}
        }
        // Sleeps until the next hide deadline instead of forever; each event recomputes it.
        if *control_flow == ControlFlow::Wait
            && let Some(deadline) = app.as_ref().and_then(App::next_hide_instant)
        {
            *control_flow = ControlFlow::WaitUntil(deadline);
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
    /// Windows builds the menu from fresh entries on each show, so only macOS compares them.
    #[cfg(target_os = "macos")]
    shown_entries: Vec<MenuEntry>,
    svgs: SvgCache,
    power_source: PowerSource,
    /// For the confirm dialog's thread, which answers with a `UserEvent`.
    #[cfg(windows)]
    proxy: EventLoopProxy<UserEvent>,
    /// When the last full refresh arrived; opening the menu skips its refresh while this is fresh.
    #[cfg(windows)]
    last_refresh: Option<Instant>,
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
    /// The newer release the last check found; the menu announces it on its first line.
    #[cfg(windows)]
    update: Option<UpdateInfo>,
    /// An automatic check was skipped on battery power and runs when AC returns.
    #[cfg(windows)]
    update_check_waits_for_ac: bool,
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
        // Power changes reach the app only through the strip's window, so without a strip a
        // battery start would never see AC return; the source stays unknown and nothing pauses.
        #[cfg(windows)]
        let source = if taskbar.is_some() {
            pause::current_power_source()
        } else {
            PowerSource::NoBattery
        };
        #[cfg(not(windows))]
        let source = pause::current_power_source();
        let power = PowerState {
            source,
            refresh_on_battery: config.refresh_on_battery,
        };
        log::trace(format_args!("power at start {power:?}"));
        let worker = refresh::spawn_worker(proxy.clone(), config.refresh_seconds, power)?;
        #[cfg(windows)]
        update::spawn_scheduler(proxy.clone());
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
            #[cfg(target_os = "macos")]
            shown_entries: Vec::new(),
            svgs: SvgCache::default(),
            power_source: power.source,
            #[cfg(windows)]
            proxy,
            #[cfg(windows)]
            last_refresh: None,
            #[cfg(target_os = "macos")]
            text: ui_font::menu_bar_text(MAC_STRIP_HEIGHT_PX),
            #[cfg(target_os = "macos")]
            _edit_menu: install_edit_menu(),
            #[cfg(windows)]
            taskbar,
            #[cfg(windows)]
            shown_taskbar: None,
            #[cfg(windows)]
            update: None,
            #[cfg(windows)]
            update_check_waits_for_ac: false,
        };
        app.apply();
        Ok(app)
    }

    /// Handles one event and says whether the loop keeps running.
    fn handle(&mut self, event: UserEvent) -> ControlFlow {
        // Every handled event proves the UI thread is pumping.
        #[cfg(windows)]
        crate::watchdog::beat(crate::watchdog::Watched::Ui);
        let _span = log::span_fmt(format_args!("handle {}", event.name()));
        match event {
            UserEvent::Devices(devices, file) => self.on_devices(devices, *file),
            UserEvent::Menu(event) => {
                // muda flips check items on click; a forced rebuild restores the config's ticks.
                #[cfg(target_os = "macos")]
                self.shown_entries.clear();
                if let Some(action) = menu::action_for(event.id.as_ref()) {
                    log::trace(format_args!("menu action {action:?}"));
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
            // The theme changed; `apply` below re-reads it and sends the strip again.
            #[cfg(windows)]
            UserEvent::Taskbar(taskbar::Signal::ThemeChanged) => {}
            #[cfg(windows)]
            UserEvent::Taskbar(taskbar::Signal::PowerChanged) => {
                self.on_power_changed();
                return ControlFlow::Wait;
            }
            #[cfg(windows)]
            UserEvent::Taskbar(taskbar::Signal::Click) | UserEvent::TrayClick => self.show_menu(),
            // Config writes stay on this thread, so the dialog's thread only reports the answer.
            #[cfg(windows)]
            UserEvent::EnableRefreshOnBattery => self.set_refresh_on_battery(true),
            #[cfg(windows)]
            UserEvent::Action(action) => self.on_action(action),
            // Nothing on screen changes, so no `apply`.
            #[cfg(windows)]
            UserEvent::UpdateDue => {
                self.on_update_due();
                return ControlFlow::Wait;
            }
            #[cfg(windows)]
            UserEvent::UpdateChecked { manual, outcome } => {
                self.on_update_checked(manual, outcome);
                return ControlFlow::Wait;
            }
            // Same exit as Quit, so the tray icon and strip are removed first.
            #[cfg(windows)]
            UserEvent::UpdateRestart => {
                self.hide_taskbar();
                return ControlFlow::Exit;
            }
        }
        self.apply();
        ControlFlow::Wait
    }

    fn on_devices(&mut self, mut devices: Vec<DeviceView>, file: Config) {
        log::trace(format_args!("devices received {}", devices.len()));
        // Same-kind devices sit together in the strip and menu, highest level first and offline last within a kind.
        sort_grouped(&mut devices);
        // Every device is checked so its last reading stays current even while hidden.
        for device in &devices {
            let crossed = self.alerts.check(device, self.config.low_battery_percent);
            if crossed && !self.config.is_hidden(&device.id, &device.name) {
                notify_low_battery(device);
            }
        }
        self.devices = devices;
        self.has_read = true;
        // Hand edits change the device keys in the file; the worker read it for this cycle, so
        // this thread follows them without a second read.
        self.config.take_device_lists(file);
        #[cfg(windows)]
        {
            self.last_refresh = Some(Instant::now());
        }
    }

    fn on_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Refresh => self.tell_worker(Command::RefreshNow),
            MenuAction::SetRefresh(seconds) => self.set_refresh(seconds),
            MenuAction::CustomRefresh => self.prompt_refresh(),
            MenuAction::PickSvg(id) => self.pick_svg(&id),
            MenuAction::CustomLow => self.prompt_low(),
            MenuAction::CustomHideOffline => self.prompt_hide_offline(),
            MenuAction::CustomColor(id, theme) => self.prompt_color(&id, theme),
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
            MenuAction::ToggleLabel(id) => {
                self.update_device(&id, |options| options.show_label = !options.show_label);
            }
            MenuAction::SetIcon(id, icon) => self.update_device(&id, |options| options.icon = icon),
            MenuAction::SetColors(id, light, dark) => self.update_device(&id, |options| {
                options.color_light = light;
                options.color_dark = dark;
            }),
            MenuAction::SetHideOffline(minutes) => {
                self.config.hide_offline_minutes = minutes;
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
            MenuAction::ToggleRefreshOnBattery => {
                if self.config.refresh_on_battery {
                    self.set_refresh_on_battery(false);
                } else {
                    self.confirm_refresh_on_battery();
                }
            }
            MenuAction::CheckForUpdates => self.check_for_updates(),
            MenuAction::ToggleCheckUpdates => {
                self.config.check_updates = !self.config.check_updates;
                self.save_config();
            }
            MenuAction::OpenUpdate => self.open_update(),
        }
    }

    /// Windows checks GitHub on its own thread, which then shows the answer.
    #[cfg(windows)]
    fn check_for_updates(&self) {
        update::spawn_check(self.proxy.clone(), true);
    }

    /// macOS sends nothing: the releases page is the check.
    #[cfg(not(windows))]
    fn check_for_updates(&self) {
        update::open_url(update::RELEASES_URL);
    }

    /// The menu line offers the update in a dialog on its own thread.
    #[cfg(windows)]
    fn open_update(&self) {
        if let Some(info) = &self.update {
            update::spawn_offer(self.proxy.clone(), info.clone());
        }
    }

    /// macOS never announces an update, so its menu has no such line.
    #[cfg(not(windows))]
    fn open_update(&self) {}

    /// Saves the battery option and tells the worker, which restarts or stops its timer.
    fn set_refresh_on_battery(&mut self, on: bool) {
        self.config.refresh_on_battery = on;
        self.save_config();
        self.send_power();
    }

    /// Asks before turning the option on; the dialog blocks, so it runs on its own thread and
    /// answers with `UserEvent::EnableRefreshOnBattery`. One dialog at a time.
    #[cfg(windows)]
    fn confirm_refresh_on_battery(&mut self) {
        use std::sync::atomic::{AtomicBool, Ordering};

        static OPEN: AtomicBool = AtomicBool::new(false);
        if OPEN.swap(true, Ordering::Relaxed) {
            return;
        }
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("confirm".into())
            .spawn(move || {
                let _span = log::span("dialog battery refresh confirm");
                if dialog::confirm(
                    BATTERY_CONFIRM_TITLE,
                    BATTERY_CONFIRM,
                    BATTERY_CONFIRM_OK,
                    "Cancel",
                ) {
                    // A closed event loop means the app is exiting.
                    let _ = proxy.send_event(UserEvent::EnableRefreshOnBattery);
                }
                OPEN.store(false, Ordering::Relaxed);
            });
        if let Err(error) = spawned {
            OPEN.store(false, Ordering::Relaxed);
            eprintln!("confirm dialog unavailable: {error}");
        }
    }

    /// Only Windows detects a battery, so the menu never offers the option elsewhere.
    #[cfg(not(windows))]
    fn confirm_refresh_on_battery(&mut self) {
        self.set_refresh_on_battery(true);
    }

    /// The inputs the worker's pause rules read.
    fn power_state(&self) -> PowerState {
        PowerState {
            source: self.power_source,
            refresh_on_battery: self.config.refresh_on_battery,
        }
    }

    fn send_power(&self) {
        self.tell_worker(Command::SetPower(self.power_state()));
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
        self.ask_then_act("dialog refresh prompt", move || {
            dialog::prompt_parsed(
                REFRESH_PROMPT_TITLE,
                REFRESH_PROMPT,
                &current,
                parse_interval,
            )
            .map(|seconds| MenuAction::SetRefresh(Some(seconds)))
        });
    }

    /// Runs a value prompt and then its action. Windows keeps the modal dialog off this thread
    /// (one at a time) and answers with `UserEvent::Action`; macOS needs NSAlert on the main one.
    #[cfg(windows)]
    fn ask_then_act(
        &mut self,
        label: &'static str,
        ask: impl FnOnce() -> Option<MenuAction> + Send + 'static,
    ) {
        use std::sync::atomic::{AtomicBool, Ordering};

        static OPEN: AtomicBool = AtomicBool::new(false);
        if OPEN.swap(true, Ordering::Relaxed) {
            return;
        }
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("prompt".into())
            .spawn(move || {
                let action = {
                    let _span = log::span(label);
                    ask()
                };
                if let Some(action) = action {
                    // A closed event loop means the app is exiting.
                    let _ = proxy.send_event(UserEvent::Action(action));
                }
                OPEN.store(false, Ordering::Relaxed);
            });
        if let Err(error) = spawned {
            OPEN.store(false, Ordering::Relaxed);
            eprintln!("prompt unavailable: {error}");
        }
    }

    #[cfg(not(windows))]
    fn ask_then_act(
        &mut self,
        label: &'static str,
        ask: impl FnOnce() -> Option<MenuAction> + Send + 'static,
    ) {
        let action = {
            let _span = log::span(label);
            ask()
        };
        if let Some(action) = action {
            self.on_action(action);
        }
    }

    /// Asks for a custom low-battery percent, pre-filled with the current one; it drives both the
    /// alert and the strip's red gauge like the quick choices.
    fn prompt_low(&mut self) {
        let current = self
            .config
            .low_battery_percent
            .map(|percent| percent.to_string())
            .unwrap_or_default();
        self.ask_then_act("dialog low prompt", move || {
            dialog::prompt_parsed(LOW_PROMPT_TITLE, LOW_PROMPT, &current, parse_low_percent)
                .map(|percent| MenuAction::SetLow(Some(percent)))
        });
    }

    /// Asks for custom minutes before an offline device leaves the strip.
    fn prompt_hide_offline(&mut self) {
        let current = self
            .config
            .hide_offline_minutes
            .map(|minutes| minutes.to_string())
            .unwrap_or_default();
        self.ask_then_act("dialog hide offline prompt", move || {
            dialog::prompt_parsed(
                HIDE_OFFLINE_PROMPT_TITLE,
                HIDE_OFFLINE_PROMPT,
                &current,
                parse_hide_offline_minutes,
            )
            .map(|minutes| MenuAction::SetHideOffline(Some(minutes)))
        });
    }

    /// Asks for a custom icon colour, starting from the current one; `theme` `None` applies it
    /// to both taskbar themes.
    fn prompt_color(&mut self, id: &str, theme: Option<Theme>) {
        let options = self.config.display_options(id);
        let current = match theme {
            Some(Theme::Light) => options.color_light,
            Some(Theme::Dark) => options.color_dark,
            None => [options.color_light, options.color_dark]
                .into_iter()
                .find(|color| matches!(color, IconColor::Rgb(..)))
                .unwrap_or(IconColor::Auto),
        };
        let _span = log::span("dialog color prompt");
        if let Some(color) = ask_color(current, theme) {
            self.update_device(id, |options| set_theme_color(options, theme, color));
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
        log::trace(format_args!("tell worker"));
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

        // Only macOS keeps a built menu; Windows builds it from fresh entries on each show, so
        // building them here would only be compared and dropped.
        #[cfg(target_os = "macos")]
        {
            let entries = self.entries(SystemTime::now());
            if entries != self.shown_entries {
                let _span = log::span("menu rebuild");
                self.current_menu = menu::build_menu(&entries);
                self.tray
                    .set_menu(Some(Box::new(self.current_menu.clone())));
                self.shown_entries = entries;
            }
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
        let segments = build_strip(&devices, self.config.low_battery_percent, &[], &|id| {
            options.get(id).cloned().unwrap_or_default()
        });
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
        let has_battery = self.power_source != PowerSource::NoBattery;
        let mut entries = menu::menu_entries(
            &self.devices,
            &self.config,
            login::login_state(),
            has_battery,
            now,
        );
        // The hint sits right under the "no devices" line.
        let empty_line = entries.iter().position(|e| e.text == EMPTY_MENU_TEXT);
        if let Some(at) = empty_line.filter(|_| self.has_read && instance::is_bundled()) {
            entries.insert(at + 1, MenuEntry::label(BLUETOOTH_PERMISSION_HINT));
        }
        // The update line leads the menu, above "Refresh now".
        #[cfg(windows)]
        if let Some(info) = &self.update {
            entries.splice(0..0, menu::update_available_entries(&info.version));
        }
        entries
    }

    /// "Hide offline after" as a duration; `None` = never.
    fn hide_offline_after(&self) -> Option<Duration> {
        self.config
            .hide_offline_minutes
            .map(|minutes| Duration::from_secs(u64::from(minutes) * 60))
    }

    /// The devices the strip and tooltip show: listed in the menu, not hidden by the user and
    /// not offline longer than "Hide offline after"; the menu keeps them, marked offline.
    fn visible_devices(&self) -> Vec<DeviceView> {
        let now = SystemTime::now();
        let hide_after = self.hide_offline_after();
        menu::listed_devices(&self.devices)
            .filter(|device| !self.config.is_hidden(&device.id, &device.name))
            .filter(|device| !device.hidden_when_offline(hide_after, now))
            .cloned()
            .collect()
    }

    /// When the earliest shown device crosses "Hide offline after", so the event loop wakes
    /// once for it instead of waiting for the next refresh; `None` when no device is pending.
    fn next_hide_instant(&self) -> Option<Instant> {
        let now = SystemTime::now();
        let hide_after = self.hide_offline_after();
        let wait = menu::listed_devices(&self.devices)
            .filter(|device| !self.config.is_hidden(&device.id, &device.name))
            .filter_map(|device| device.hide_deadline(hide_after))
            .filter_map(|deadline| deadline.duration_since(now).ok())
            .min()?;
        Some(Instant::now() + wait)
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
    /// The power status changed; the worker hears of it only when AC or battery actually flipped.
    fn on_power_changed(&mut self) {
        let source = pause::current_power_source();
        if source == self.power_source {
            return;
        }
        log::trace(format_args!(
            "power source {:?} -> {source:?}",
            self.power_source
        ));
        self.power_source = source;
        self.send_power();
        if self.update_check_waits_for_ac && pause::timer_may_read(self.power_state()) {
            self.on_update_due();
        }
    }

    /// An automatic check, unless the owner turned them off; on battery power it waits for AC
    /// (the same rule as the auto-refresh timer).
    fn on_update_due(&mut self) {
        self.update_check_waits_for_ac = false;
        if !self.config.check_updates {
            return;
        }
        if !pause::timer_may_read(self.power_state()) {
            self.update_check_waits_for_ac = true;
            return;
        }
        update::spawn_check(self.proxy.clone(), false);
    }

    /// Remembers the newest release for the menu line. The owner is told once per version: by a
    /// notification after an automatic check, silently after a manual one (its dialog already did).
    fn on_update_checked(&mut self, manual: bool, outcome: Result<UpdateCheck, String>) {
        match outcome {
            Ok(UpdateCheck::Available(info)) => {
                let is_new = update::check::should_notify(
                    self.config.notified_update.as_deref(),
                    &info.version,
                );
                if is_new {
                    self.config.notified_update = Some(info.version.clone());
                    self.save_config();
                    if !manual {
                        notify_update_available(&info.version);
                    }
                }
                self.update = Some(info);
            }
            Ok(UpdateCheck::UpToDate) => self.update = None,
            // The owner sees a manual check's error in its dialog; an automatic one retries tomorrow.
            Err(error) => log::line(&format!("update check failed: {error}")),
        }
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
        // Opening the menu also refreshes unless a refresh just finished; per-device floors still
        // limit what is queried.
        if pause::menu_refresh_due(self.last_refresh, Instant::now()) {
            self.tell_worker(Command::RefreshNow);
        }
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

/// Sets the colour of one taskbar theme, or of both when `theme` is `None`.
fn set_theme_color(options: &mut DisplayOptions, theme: Option<Theme>, color: IconColor) {
    if theme != Some(Theme::Dark) {
        options.color_light = color;
    }
    if theme != Some(Theme::Light) {
        options.color_dark = color;
    }
}

/// The Windows colour dialog, opened on `current`; Auto and Original have no colour of their
/// own, so it opens on the theme ink.
#[cfg(windows)]
fn ask_color(current: IconColor, theme: Option<Theme>) -> Option<IconColor> {
    let seed = match (current, theme) {
        (IconColor::Rgb(red, green, blue), _) => (red, green, blue),
        (_, Some(Theme::Dark)) => (255, 255, 255),
        (_, _) => (0, 0, 0),
    };
    dialog::pick_color(seed).map(|(red, green, blue)| IconColor::Rgb(red, green, blue))
}

/// macOS has no colour dialog here: a `#RRGGBB` prompt pre-filled with the current custom colour.
#[cfg(not(windows))]
fn ask_color(current: IconColor, theme: Option<Theme>) -> Option<IconColor> {
    let shown = match current {
        IconColor::Rgb(red, green, blue) => format!("#{}", crate::config::hex(red, green, blue)),
        IconColor::Auto | IconColor::Original => String::new(),
    };
    let title = match theme {
        None => "Custom icon colour",
        Some(Theme::Light) => "Custom icon colour (light menu bar)",
        Some(Theme::Dark) => "Custom icon colour (dark menu bar)",
    };
    dialog::prompt_parsed(title, COLOR_PROMPT, &shown, parse_hex_color)
}

/// Only a `#RRGGBB` colour (the `#` is optional) counts as a custom colour.
#[cfg(not(windows))]
fn parse_hex_color(text: &str) -> Result<IconColor, String> {
    match crate::config::parse_color(text.trim()) {
        Some(color @ IconColor::Rgb(..)) => Ok(color),
        _ => Err(COLOR_HINT.to_owned()),
    }
}
