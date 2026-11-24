//! Layered child window docked into Explorer's taskbar, kept alive by a 2 s timer.
//! It runs on its own "taskbar" thread: a child of Explorer's taskbar attaches its thread's input
//! queue to Explorer's, so that thread does no modal UI and the app's menus and dialogs stay on
//! the main thread, where a stuck menu loop cannot freeze the taskbar.
//! The thread's hidden top-level keeper window also hears the system's own events: theme changes,
//! power-status changes and fullscreen apps (as an appbar that never reserves space).

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, bail};
use strip_render::{Bitmap, SvgIcon, TextStyle, Theme};
use ui_model::Segment;
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, ClientToScreen,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP,
    HDC, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::Controls::{
    ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW, TTF_SUBCLASS,
    TTM_ADDTOOLW, TTM_DELTOOLW, TTM_SETMAXTIPWIDTH, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW,
    FindWindowW, GW_HWNDPREV, GetClientRect, GetMessageW, GetWindow, GetWindowRect, HWND_TOP,
    IsWindow, IsWindowVisible, MA_NOACTIVATE, MSG, PBT_APMPOWERSTATUSCHANGE, PostMessageW,
    RegisterClassExW, RegisterWindowMessageW, SW_HIDE, SWP_NOACTIVATE, SWP_NOSENDCHANGING,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SendMessageW, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_POWERBROADCAST, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER,
    WNDCLASSEXW, WNDPROC, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, PWSTR, w};

use crate::pause::PowerSource;
use crate::watchdog::{self, Watched};
use crate::{log, pause, theme, ui_font};

use super::layout::{
    Placement, Rect, natural_container, place_secondary_strip, place_strip, placement_stale,
    shrink_to_clear, strip_height,
};
use super::premultiplied_bgra;

const STRIP_CLASS: PCWSTR = w!("JuicetrayTaskbarStrip");
const KEEPER_CLASS: PCWSTR = w!("JuicetrayKeeper");
const KEEPER_TIMER_ID: usize = 1;
const KEEPER_INTERVAL_MS: u32 = 2000;
const TOOLTIP_TOOL_ID: usize = 1;
/// Wide enough that a tooltip's own line breaks decide the wrapping.
const TOOLTIP_MAX_WIDTH_PX: isize = 600;
/// Posted to the keeper window after a command is queued, so the taskbar thread wakes for it.
const WAKE_MESSAGE: u32 = WM_APP + 1;
/// The appbar notification the shell sends the keeper window (`ABN_*` code in `wParam`).
const APPBAR_CALLBACK: u32 = WM_APP + 2;
/// `WM_SETTINGCHANGE`'s `lParam` names the changed setting; this one is the light/dark theme.
const THEME_SETTING: &str = "ImmersiveColorSet";

/// A fullscreen app has the screen: the keeper tick and strip painting stand still.
static FULLSCREEN: AtomicBool = AtomicBool::new(false);
/// The theme last seen, so a settings broadcast that did not change it wakes nobody.
static DARK_THEME: AtomicBool = AtomicBool::new(false);
/// Whether the laptop was last seen on battery, for the same reason.
static ON_BATTERY: AtomicBool = AtomicBool::new(false);
/// Id of the "TaskbarCreated" broadcast Explorer sends after a restart; 0 until registered.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// What a window procedure tells the event loop; each one is sent only on a real change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The light/dark theme changed.
    ThemeChanged,
    /// A fullscreen app opened (`true`) or closed (`false`).
    Fullscreen(bool),
    /// The laptop switched between AC and battery power.
    PowerChanged,
    /// The strip was clicked with the left or right button.
    Click,
}

/// Window procedures are plain functions, so the callback lives in a process-wide slot.
static NOTIFY: OnceLock<Box<dyn Fn(Signal) + Send + Sync>> = OnceLock::new();

fn notify(signal: Signal) {
    // Written from the taskbar thread, so a missing line means that thread is not pumping.
    log::trace(format_args!("wndproc signal {signal:?}"));
    if let Some(callback) = NOTIFY.get() {
        callback(signal);
    }
}

/// The Explorer window class a dock's strip is a child of, for the trace.
fn taskbar_class(is_primary: bool) -> &'static str {
    if is_primary {
        "Shell_TrayWnd"
    } else {
        "Shell_SecondaryTrayWnd"
    }
}

/// The Explorer window class of the task-button container a dock resizes, for the trace.
fn container_class(is_primary: bool) -> &'static str {
    if is_primary {
        "ReBarWindow32"
    } else {
        "WorkerW"
    }
}

/// The strip child window and the tooltip it owns (destroyed together with it).
struct StripWindow {
    hwnd: HWND,
    tooltip: Option<HWND>,
}

/// What a dock's window currently shows: the (height, DPI) it was rendered for and its pixel size.
#[derive(Clone, Copy)]
struct Shown {
    size: (u32, u32),
    width: i32,
    height: i32,
}

/// One strip copy, the taskbar it lives in and the space it reserved there.
struct Dock {
    taskbar: HWND,
    /// `Shell_TrayWnd` (docks left of the notification area) rather than `Shell_SecondaryTrayWnd`.
    is_primary: bool,
    strip: StripWindow,
    /// The task-button container has been sized by this copy, so `release` must restore it.
    reserved: bool,
    /// `None` until the copy has been painted.
    shown: Option<Shown>,
    /// The placement last applied to the window; `None` until applied.
    placed: Option<Placement>,
}

/// Renders the strip for one taskbar's (height px, DPI).
type RenderStrip<'a> = dyn FnMut(u32, u32) -> Bitmap + 'a;

/// What the strip shows; the taskbar thread renders it per taskbar (height, DPI).
pub struct StripContent {
    pub segments: Vec<Segment>,
    pub theme: Theme,
    /// Parsed custom SVG icons by path, shared with the app's cache.
    pub icons: Arc<HashMap<String, SvgIcon>>,
    pub tooltip: String,
}

enum Command {
    Show(StripContent),
    Stop,
}

/// The main thread's side of the strip running on the "taskbar" thread. Dropping it stops the
/// thread once every copy is removed and Explorer's task-button containers are given back.
pub struct TaskbarHandle {
    commands: Sender<Command>,
    /// The taskbar thread's keeper window, which `WAKE_MESSAGE` is posted to.
    keeper: HWND,
    thread: Option<JoinHandle<()>>,
}

impl TaskbarHandle {
    /// Starts the taskbar thread and waits until its windows exist; `callback` receives every
    /// signal on that thread.
    pub fn new(callback: impl Fn(Signal) + Send + Sync + 'static) -> Result<Self> {
        let (commands, received) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("taskbar".into())
            .spawn(move || run_taskbar_thread(callback, &received, &ready))
            .context("spawning the taskbar thread")?;
        // HWND is not `Send`, so the keeper comes back as its address.
        let started = started
            .recv()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the taskbar thread stopped while starting")));
        match started {
            Ok(keeper) => Ok(Self {
                commands,
                keeper: HWND(keeper as *mut c_void),
                thread: Some(thread),
            }),
            Err(error) => {
                let _ = thread.join();
                Err(error)
            }
        }
    }

    /// Shows `content` on every taskbar.
    pub fn show(&self, content: StripContent) {
        self.send(Command::Show(content));
    }

    /// Queues `command` and wakes the taskbar thread.
    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            eprintln!("the taskbar thread has stopped");
            return;
        }
        // SAFETY: posting only queues a message; a destroyed keeper just fails the call.
        if let Err(error) =
            unsafe { PostMessageW(Some(self.keeper), WAKE_MESSAGE, WPARAM(0), LPARAM(0)) }
        {
            eprintln!("waking the taskbar thread failed: {error}");
        }
    }
}

impl Drop for TaskbarHandle {
    /// Waits for the thread, so the containers are restored before the process exits.
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The taskbar thread: owns every strip window and pumps their messages. The keeper's timer and
/// wake messages are handled here rather than in its window procedure, which has no state.
fn run_taskbar_thread(
    callback: impl Fn(Signal) + Send + Sync + 'static,
    commands: &Receiver<Command>,
    ready: &SyncSender<Result<isize>>,
) {
    let mut strip = match TaskbarStrip::new(callback) {
        Ok(strip) => strip,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let keeper = strip.keeper;
    if ready.send(Ok(keeper.0 as isize)).is_err() {
        return;
    }
    // The keeper timer beats every 2 s on this thread, so silence means a hang here.
    watchdog::watch_current(Watched::Taskbar);
    let mut content: Option<StripContent> = None;
    // Content that arrived but is not painted yet, because a fullscreen app had the screen.
    let mut unpainted = false;
    // The status-bar font per (taskbar DPI, strip height) it was read for.
    let mut texts: HashMap<(u32, u32), TextStyle> = HashMap::new();
    let mut message = MSG::default();
    // SAFETY: `message` is a local out-pointer; 0 means WM_QUIT and -1 an error, both end the loop.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        if message.hwnd == keeper {
            if message.message == WM_TIMER && message.wParam.0 == KEEPER_TIMER_ID {
                // The beat stays on during game mode, so the watchdog keeps seeing a live thread.
                watchdog::beat(Watched::Taskbar);
                // Re-docking and re-rendering are the whole tick; the theme arrives as an event.
                if !FULLSCREEN.load(Ordering::Relaxed)
                    && let Some(content) = &content
                {
                    strip.keep(&mut |height, dpi| render(content, &mut texts, height, dpi));
                }
                continue;
            }
            if message.message == WAKE_MESSAGE {
                for command in commands.try_iter() {
                    match command {
                        Command::Show(shown) => {
                            content = Some(shown);
                            unpainted = true;
                        }
                        // Dropping `strip` restores the containers; thread exit destroys the keeper.
                        Command::Stop => return,
                    }
                }
                // Also posted when a fullscreen app closes, which paints what arrived meanwhile.
                if unpainted
                    && !FULLSCREEN.load(Ordering::Relaxed)
                    && let Some(shown) = &content
                {
                    strip.show(
                        &mut |height, dpi| render(shown, &mut texts, height, dpi),
                        &shown.tooltip,
                    );
                    unpainted = false;
                }
                continue;
            }
        }
        // SAFETY: `message` was just filled by GetMessageW on this thread.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// Renders `content` for one taskbar; the OS font is read again only for an unseen (DPI, height).
fn render(
    content: &StripContent,
    texts: &mut HashMap<(u32, u32), TextStyle>,
    height: u32,
    dpi: u32,
) -> Bitmap {
    let _span = log::span_fmt(format_args!("render taskbar strip {height}px dpi {dpi}"));
    let text = texts
        .entry((dpi, height))
        .or_insert_with(|| ui_font::status_text(dpi, height));
    strip_render::render_strip(
        &content.segments,
        height,
        content.theme,
        &content.icons,
        text,
    )
}

struct TaskbarStrip {
    instance: HINSTANCE,
    /// Hidden window owning the keeper timer and receiving `WAKE_MESSAGE`.
    keeper: HWND,
    /// One copy per taskbar: the primary and every `Shell_SecondaryTrayWnd`.
    docks: Vec<Dock>,
    /// A strip is being shown (set by `show`, cleared by `hide`); the keeper only acts while it is.
    active: bool,
    tooltip: String,
    create_failure_logged: bool,
}

impl TaskbarStrip {
    /// Registers the window classes and starts the keeper timer; `callback` receives every signal.
    fn new(callback: impl Fn(Signal) + Send + Sync + 'static) -> Result<Self> {
        let _span = log::span("TaskbarStrip::new");
        if NOTIFY.set(Box::new(callback)).is_err() {
            bail!("the taskbar strip already exists");
        }
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        // SAFETY: `controls` is fully initialised and outlives the call.
        if !unsafe { InitCommonControlsEx(&controls) }.as_bool() {
            bail!("InitCommonControlsEx failed");
        }
        // SAFETY: a null module name asks for this executable's own handle.
        let instance = HINSTANCE::from(unsafe { GetModuleHandleW(None) }?);
        register_class(STRIP_CLASS, Some(strip_proc), instance)?;
        register_class(KEEPER_CLASS, Some(keeper_proc), instance)?;
        // The hidden top-level window (not message-only, not the child) lives as long as the
        // taskbar thread and owns the keeper timer.
        // SAFETY: the class is registered above; a top-level window gets no parent.
        let keeper = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                KEEPER_CLASS,
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
        }?;
        // SAFETY: `keeper` is the window just created on this thread.
        if unsafe { SetTimer(Some(keeper), KEEPER_TIMER_ID, KEEPER_INTERVAL_MS, None) } == 0 {
            // SAFETY: `keeper` is valid and owned by this thread; a failed destroy changes nothing.
            let _ = unsafe { DestroyWindow(keeper) };
            bail!("SetTimer failed: {}", windows::core::Error::from_thread());
        }
        // Start-up state of what the keeper window then keeps current by events.
        FULLSCREEN.store(pause::fullscreen_now(), Ordering::Relaxed);
        DARK_THEME.store(theme::system_theme() == Theme::Dark, Ordering::Relaxed);
        ON_BATTERY.store(
            pause::current_power_source() == PowerSource::Battery,
            Ordering::Relaxed,
        );
        // SAFETY: the name is a static literal; 0 (failure) leaves the restart hook off.
        TASKBAR_CREATED.store(
            unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            Ordering::Relaxed,
        );
        register_appbar(keeper);
        Ok(Self {
            instance,
            keeper,
            docks: Vec::new(),
            active: false,
            tooltip: String::new(),
            create_failure_logged: false,
        })
    }

    /// Shows the strip with `tooltip` on every taskbar, calling `render` once per distinct
    /// (height px, DPI) among them, and creates the windows that are missing.
    fn show(&mut self, render: &mut RenderStrip, tooltip: &str) {
        let _span = log::span("taskbar show");
        self.active = true;
        tooltip.clone_into(&mut self.tooltip);
        self.sync(render, true);
    }

    /// Keeper tick: recreates copies after an Explorer restart or a display change, re-renders
    /// copies whose taskbar height or DPI changed and re-docks them.
    fn keep(&mut self, render: &mut RenderStrip) {
        let _span = log::span("keeper tick");
        self.sync(render, false);
    }

    /// Removes every strip copy, gives back the reserved space and stops showing anything.
    fn hide(&mut self) {
        let _span = log::span("taskbar hide");
        self.active = false;
        self.docks.drain(..).for_each(Dock::destroy);
    }

    /// Gives every taskbar a live copy, paints copies when `repaint` (or when new, or when their
    /// taskbar's height or DPI changed) and docks them.
    fn sync(&mut self, render: &mut RenderStrip, repaint: bool) {
        if !self.active {
            return;
        }
        let _span = log::span_fmt(format_args!("sync repaint={repaint}"));
        let primary = find_taskbar();
        let taskbars: Vec<HWND> = primary
            .into_iter()
            .chain(dockable_secondary_taskbars())
            .collect();
        // Copies whose taskbar is gone (Explorer restart, display removed or setting off) go first.
        let (live, gone): (Vec<Dock>, Vec<Dock>) = std::mem::take(&mut self.docks)
            .into_iter()
            .partition(|dock| dock.strip.is_alive() && taskbars.contains(&dock.taskbar));
        if !gone.is_empty() {
            log::trace(format_args!("docks gone {}", gone.len()));
        }
        gone.into_iter().for_each(Dock::destroy);
        self.docks = live;
        // Taskbars with the same (height, DPI) share one render within this call.
        let mut rendered: HashMap<(u32, u32), Bitmap> = HashMap::new();
        for taskbar in taskbars {
            if self.docks.iter().any(|dock| dock.taskbar == taskbar) {
                continue;
            }
            // Explorer is mid-restart for this bar; a later tick creates its copy.
            let Some(size) = strip_size(taskbar) else {
                continue;
            };
            let bitmap = rendered
                .entry(size)
                .or_insert_with(|| render(size.0, size.1));
            let is_primary = Some(taskbar) == primary;
            let created = {
                let _span =
                    log::span_fmt(format_args!("dock add in {}", taskbar_class(is_primary)));
                StripWindow::create(taskbar, self.instance, bitmap)
            };
            match created {
                Ok(strip) => {
                    self.docks.push(Dock {
                        taskbar,
                        is_primary,
                        strip,
                        reserved: false,
                        shown: None,
                        placed: None,
                    });
                    self.create_failure_logged = false;
                }
                Err(error) => {
                    // The tray icon keeps working; log once, then retry quietly every tick.
                    if !self.create_failure_logged {
                        eprintln!("taskbar strip window failed (GetLastError): {error}");
                        self.create_failure_logged = true;
                    }
                }
            }
        }
        for dock in &mut self.docks {
            let Some(size) = strip_size(dock.taskbar) else {
                continue;
            };
            if repaint || dock.shown.is_none_or(|shown| shown.size != size) {
                let bitmap = rendered
                    .entry(size)
                    .or_insert_with(|| render(size.0, size.1));
                dock.strip.show_bitmap(bitmap, &self.tooltip);
                dock.shown = Some(Shown {
                    size,
                    width: px(bitmap.width),
                    height: px(bitmap.height),
                });
            }
            if let Some(shown) = dock.shown {
                dock.place(shown.width, shown.height);
            }
        }
    }
}

impl Drop for TaskbarStrip {
    /// Explorer keeps a shrunk task-button container until its next layout, so give it back.
    fn drop(&mut self) {
        self.hide();
        appbar_message(ABM_REMOVE, self.keeper);
    }
}

impl Dock {
    /// Docks the copy and reserves its space, or hides it and gives the space back when it does not fit.
    fn place(&mut self, width: i32, height: i32) {
        let _span = log::span_fmt(format_args!(
            "dock place in {}",
            taskbar_class(self.is_primary)
        ));
        let placement = client_rect_on_screen(self.taskbar).map_or(Placement::Fallback, |bar| {
            if self.is_primary {
                place_strip(bar, notification_area(self.taskbar), width, height)
            } else {
                let clock = find_child(self.taskbar, w!("ClockButton")).and_then(window_rect);
                place_secondary_strip(bar, clock, width, height)
            }
        });
        // The window is moved only when the placement changed or Explorer displaced it.
        if placement_stale(self.placed, placement, || self.strip.is_in_place(placement)) {
            self.placed = self.strip.place(placement).then_some(placement);
        }
        match placement {
            Placement::Docked(rect) => self.reserve(rect.x),
            Placement::Fallback => self.release(),
        }
    }

    /// Shrinks the task-button container to end at `strip_x` (taskbar client coordinates),
    /// moving it only when its rect differs from the wanted one.
    fn reserve(&mut self, strip_x: i32) {
        let _span = log::span_fmt(format_args!("reserve strip_x={strip_x}"));
        // No container (Windows 11 XAML taskbar) leaves the strip floating as before; `reserved`
        // stays as it is, since a sibling being recreated hides a container this copy shrank.
        let Some((container, natural, current)) = self.container_layout() else {
            return;
        };
        let wanted = shrink_to_clear(natural, strip_x).unwrap_or(natural);
        if wanted != current {
            set_child_rect(container, wanted, container_class(self.is_primary));
        }
        self.reserved = true;
    }

    /// Gives the container back Explorer's own rect once this copy has sized it.
    fn release(&mut self) {
        if !std::mem::take(&mut self.reserved) {
            return;
        }
        let _span = log::span("release");
        if let Some((container, natural, current)) = self.container_layout()
            && natural != current
        {
            set_child_rect(container, natural, container_class(self.is_primary));
        }
    }

    /// The task-button container with Explorer's own rect and its current one, in taskbar client
    /// coordinates. The own rect comes from the sibling it reaches, never from the current rect,
    /// which a crashed run may have left shrunk.
    fn container_layout(&self) -> Option<(HWND, Rect, Rect)> {
        let container = task_container(self.taskbar, self.is_primary)?;
        let bar = client_rect_on_screen(self.taskbar)?;
        let current = child_rect(container, self.taskbar)?;
        // Primary: the notification area's left; secondary: the clock's left, else the bar's end.
        let edge = if self.is_primary {
            window_rect(find_child(self.taskbar, w!("TrayNotifyWnd"))?)?.x - bar.x
        } else {
            find_child(self.taskbar, w!("ClockButton"))
                .and_then(window_rect)
                .map_or(bar.w, |clock| clock.x - bar.x)
        };
        let natural = natural_container(bar, current, edge)?;
        Some((container, natural, current))
    }

    fn destroy(mut self) {
        let _span = log::span_fmt(format_args!(
            "dock remove in {}",
            taskbar_class(self.is_primary)
        ));
        self.release();
        self.strip.destroy();
    }
}

impl StripWindow {
    fn create(taskbar: HWND, instance: HINSTANCE, bitmap: &Bitmap) -> windows::core::Result<Self> {
        // The parent lives in Explorer's thread, so the system notifies it while this call waits.
        let _span = log::span("CreateWindowExW JuicetrayTaskbarStrip (parent in Explorer)");
        // SAFETY: the class is registered; `taskbar` is a live window handle.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE,
                STRIP_CLASS,
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                px(bitmap.width),
                px(bitmap.height),
                Some(taskbar),
                None,
                Some(instance),
                None,
            )
        }?;
        // A missing tooltip only loses the hover text.
        let tooltip = create_tooltip(hwnd, instance)
            .inspect_err(|error| eprintln!("creating the strip tooltip failed: {error}"))
            .ok();
        Ok(Self { hwnd, tooltip })
    }

    fn is_alive(&self) -> bool {
        // SAFETY: IsWindow accepts any handle value.
        unsafe { IsWindow(Some(self.hwnd)) }.as_bool()
    }

    fn destroy(self) {
        if self.is_alive() {
            let _span = log::span("DestroyWindow JuicetrayTaskbarStrip (parent in Explorer)");
            // SAFETY: the window belongs to this thread; destroying it also destroys its tooltip.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Paints `bitmap` and points the tooltip at it.
    fn show_bitmap(&self, bitmap: &Bitmap, tooltip: &str) {
        if let Err(error) = self.paint(bitmap) {
            eprintln!("painting the taskbar strip failed: {error}");
        }
        self.set_tooltip(tooltip, bitmap);
    }

    /// Draws `bitmap` through a top-down 32-bit DIB and `UpdateLayeredWindow`.
    fn paint(&self, bitmap: &Bitmap) -> windows::core::Result<()> {
        let (width, height) = (px(bitmap.width), px(bitmap.height));
        // SAFETY: a null window asks for the whole-screen DC, released by the guard.
        let screen = ScreenDc(unsafe { GetDC(None) });
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A negative height makes the DIB top-down, matching the bitmap's row order.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits = ptr::null_mut();
        // SAFETY: `info` describes a 32-bit DIB; `bits` receives the pixel buffer owned by it.
        let dib = Dib(unsafe {
            CreateDIBSection(Some(screen.0), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        }?);
        if bits.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        // SAFETY: the DIB holds exactly `width * height` four-byte pixels and lives until `dib` drops.
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(
                bits.cast::<[u8; 4]>(),
                (bitmap.width as usize) * (bitmap.height as usize),
            )
        };
        for (target, source) in pixels.iter_mut().zip(bitmap.pixels()) {
            *target = premultiplied_bgra(*source);
        }

        // SAFETY: `screen` is a valid DC; the memory DC is deleted by its guard.
        let memory = MemoryDc(unsafe { CreateCompatibleDC(Some(screen.0)) });
        // SAFETY: both handles are valid; the DIB stays selected until the memory DC is deleted.
        unsafe { SelectObject(memory.0, dib.0.into()) };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let source_origin = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _span = log::span("UpdateLayeredWindow JuicetrayTaskbarStrip");
        // SAFETY: all pointers refer to locals that outlive the call; `pptdst` is None because
        // the position belongs to SetWindowPos in parent coordinates.
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                Some(screen.0),
                None,
                Some(&size),
                Some(memory.0),
                Some(&source_origin),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }

    /// Points the hover tooltip at the whole strip.
    fn set_tooltip(&self, text: &str, bitmap: &Bitmap) {
        let Some(tooltip) = self.tooltip else {
            return;
        };
        let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let info = TTTOOLINFOW {
            // The size up to `lpszText` is accepted by every comctl32 version.
            cbSize: (std::mem::offset_of!(TTTOOLINFOW, lpszText) + size_of::<PWSTR>()) as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: self.hwnd,
            uId: TOOLTIP_TOOL_ID,
            rect: RECT {
                left: 0,
                top: 0,
                right: px(bitmap.width),
                bottom: px(bitmap.height),
            },
            lpszText: PWSTR(wide.as_mut_ptr()),
            ..Default::default()
        };
        let lparam = LPARAM(ptr::from_ref(&info) as isize);
        let _span = log::span("SendMessageW tooltips_class32 TTM_DELTOOLW + TTM_ADDTOOLW");
        // SAFETY: `info` and `wide` outlive both calls; the control copies the text.
        unsafe {
            SendMessageW(tooltip, TTM_DELTOOLW, None, Some(lparam));
            SendMessageW(tooltip, TTM_ADDTOOLW, None, Some(lparam));
        }
    }

    /// Whether the window still sits where `placement` left it: docked = visible and topmost
    /// among its siblings, fallback = hidden.
    fn is_in_place(&self, placement: Placement) -> bool {
        // SAFETY: IsWindowVisible accepts any handle value.
        let visible = unsafe { IsWindowVisible(self.hwnd) }.as_bool();
        match placement {
            // SAFETY: GetWindow accepts any handle value; no previous sibling = already on top.
            Placement::Docked(_) => {
                visible && unsafe { GetWindow(self.hwnd, GW_HWNDPREV) }.is_err()
            }
            Placement::Fallback => !visible,
        }
    }

    /// Moves the window to its docked spot, or hides it when the layout does not fit.
    /// `false` when the move failed, so the caller does not remember it as applied.
    fn place(&self, placement: Placement) -> bool {
        match placement {
            Placement::Docked(rect) => {
                let _span = log::span("SetWindowPos JuicetrayTaskbarStrip");
                // SAFETY: the window is ours; the position is in the parent's client coordinates.
                let moved = unsafe {
                    SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOP),
                        rect.x,
                        rect.y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    )
                };
                if let Err(error) = &moved {
                    eprintln!("moving the taskbar strip failed: {error}");
                }
                moved.is_ok()
            }
            Placement::Fallback => {
                let _span = log::span("ShowWindow SW_HIDE JuicetrayTaskbarStrip");
                // SAFETY: the window is ours; the result is the previous visibility, not an error.
                let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
                true
            }
        }
    }
}

/// Releases the screen DC on every exit path.
struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: the DC came from `GetDC(None)` and is released once.
        unsafe { ReleaseDC(None, self.0) };
    }
}

/// Deletes a DIB section; declared before the memory DC so it drops after it (a selected bitmap cannot be deleted).
struct Dib(HBITMAP);

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: the bitmap came from `CreateDIBSection` and is deselected by now.
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

struct MemoryDc(HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: the DC came from `CreateCompatibleDC` and is deleted once.
        let _ = unsafe { DeleteDC(self.0) };
    }
}

fn register_class(name: PCWSTR, procedure: WNDPROC, instance: HINSTANCE) -> Result<()> {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: procedure,
        hInstance: instance,
        lpszClassName: name,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised; the name is a static literal.
    if unsafe { RegisterClassExW(&class) } == 0 {
        bail!(
            "registering a window class failed: {}",
            windows::core::Error::from_thread()
        );
    }
    Ok(())
}

fn create_tooltip(owner: HWND, instance: HINSTANCE) -> windows::core::Result<HWND> {
    let _span = log::span("CreateWindowExW tooltips_class32");
    // SAFETY: the control class is registered by `InitCommonControlsEx`; `owner` is a live window.
    let tooltip = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            PCWSTR::null(),
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(owner),
            None,
            Some(instance),
            None,
        )
    }?;
    // SAFETY: `tooltip` was just created; a max width lets `\n` break lines.
    unsafe {
        SendMessageW(
            tooltip,
            TTM_SETMAXTIPWIDTH,
            None,
            Some(LPARAM(TOOLTIP_MAX_WIDTH_PX)),
        )
    };
    Ok(tooltip)
}

/// Click on the strip: tell the event loop, which owns the current menu.
unsafe extern "system" fn strip_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_LBUTTONUP | WM_RBUTTONUP => {
            notify(Signal::Click);
            LRESULT(0)
        }
        // Clicking the strip must not pull focus away from the user's window.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: forwards the unchanged message arguments of this window procedure.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Keeper window: its timer and wake messages are handled in the taskbar thread's loop; the
/// system's broadcasts and the appbar callback arrive here, sent or posted, and each tells the
/// event loop only when something really changed.
unsafe extern "system" fn keeper_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        APPBAR_CALLBACK => {
            if wparam.0 == ABN_FULLSCREENAPP as usize {
                set_fullscreen(hwnd, lparam.0 != 0);
            }
            return LRESULT(0);
        }
        WM_SETTINGCHANGE if is_theme_setting(lparam) => notify_theme_if_changed(),
        WM_POWERBROADCAST if wparam.0 == PBT_APMPOWERSTATUSCHANGE as usize => {
            notify_power_if_changed();
        }
        // Explorer restarted and forgot the appbar registration.
        _ if message != 0 && message == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            register_appbar(hwnd);
        }
        _ => {}
    }
    // SAFETY: forwards the unchanged message arguments of this window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Records a fullscreen change; the change back to normal also wakes the thread's loop, which
/// paints what arrived during game mode.
fn set_fullscreen(keeper: HWND, on: bool) {
    if FULLSCREEN.swap(on, Ordering::Relaxed) == on {
        return;
    }
    notify(Signal::Fullscreen(on));
    if !on {
        // SAFETY: posting only queues a message to a window of this thread.
        if let Err(error) =
            unsafe { PostMessageW(Some(keeper), WAKE_MESSAGE, WPARAM(0), LPARAM(0)) }
        {
            eprintln!("waking the taskbar thread failed: {error}");
        }
    }
}

/// Whether a `WM_SETTINGCHANGE` names the theme setting.
fn is_theme_setting(lparam: LPARAM) -> bool {
    let name = lparam.0 as *const u16;
    if name.is_null() {
        return false;
    }
    // SAFETY: for this message `lParam` is null or a NUL-terminated string valid during the call.
    unsafe { PCWSTR(name).to_string() }.is_ok_and(|name| name == THEME_SETTING)
}

/// Tells the event loop about a theme that differs from the one last seen.
fn notify_theme_if_changed() {
    let dark = theme::system_theme() == Theme::Dark;
    if DARK_THEME.swap(dark, Ordering::Relaxed) != dark {
        notify(Signal::ThemeChanged);
    }
}

/// Tells the event loop when the laptop flips between AC and battery; the status event also
/// fires for every battery-level step, which must wake nobody.
fn notify_power_if_changed() {
    let on_battery = pause::current_power_source() == PowerSource::Battery;
    if ON_BATTERY.swap(on_battery, Ordering::Relaxed) != on_battery {
        notify(Signal::PowerChanged);
    }
}

/// Registers the keeper as an appbar so the shell reports fullscreen apps to it. No position is
/// ever set, so it reserves no screen space.
fn register_appbar(keeper: HWND) {
    if !appbar_message(ABM_NEW, keeper) {
        eprintln!("appbar registration failed; game mode only follows the state at start");
    }
}

/// Sends `ABM_NEW` or `ABM_REMOVE` for the keeper; the result is the shell's success flag.
fn appbar_message(message: u32, keeper: HWND) -> bool {
    let mut data = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: keeper,
        uCallbackMessage: APPBAR_CALLBACK,
        ..Default::default()
    };
    // SAFETY: `data` is initialised with its own size and outlives the call; the shell only
    // reads it for these two messages.
    unsafe { SHAppBarMessage(message, &mut data) != 0 }
}

/// (strip height px, DPI) for `taskbar`'s own monitor, or `None` while it cannot be measured.
fn strip_size(taskbar: HWND) -> Option<(u32, u32)> {
    let height = strip_height(client_rect_on_screen(taskbar)?)?;
    // SAFETY: only reads a property of a window handle; a stale handle gives 0.
    let dpi = unsafe { GetDpiForWindow(taskbar) };
    (dpi > 0).then_some((height, dpi))
}

fn find_taskbar() -> Option<HWND> {
    // SAFETY: the class name is a literal; no window title is given.
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok()
}

/// The taskbar's client area in screen coordinates.
fn client_rect_on_screen(window: HWND) -> Option<Rect> {
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: both out-pointers refer to locals.
    unsafe {
        GetClientRect(window, &mut client).ok()?;
        ClientToScreen(window, &mut origin).ok().ok()?;
    }
    Some(Rect {
        x: origin.x,
        y: origin.y,
        w: client.right,
        h: client.bottom,
    })
}

fn window_rect(window: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: the out-pointer refers to a local.
    unsafe { GetWindowRect(window, &mut rect) }.ok()?;
    Some(Rect {
        x: rect.left,
        y: rect.top,
        w: rect.right - rect.left,
        h: rect.bottom - rect.top,
    })
}

/// Every `Shell_SecondaryTrayWnd`: one per extra display while "Show taskbar on all displays" is on.
fn secondary_taskbars() -> Vec<HWND> {
    find_all(None, w!("Shell_SecondaryTrayWnd"))
}

/// Secondary taskbars that get a strip copy: a Windows 11 secondary draws its clock in XAML (no
/// `ClockButton` window to dock beside), so a copy there could cover it; unknown builds count as 11.
fn dockable_secondary_taskbars() -> Vec<HWND> {
    let before_windows_11 = os_version().is_some_and(|info| info.dwBuildNumber < 22000);
    secondary_taskbars()
        .into_iter()
        .filter(|taskbar| before_windows_11 || find_child(*taskbar, w!("ClockButton")).is_some())
        .collect()
}

/// Every window of class `class` among the children of `parent` (top-level windows for `None`).
fn find_all(parent: Option<HWND>, class: PCWSTR) -> Vec<HWND> {
    let mut found = Vec::new();
    let mut after = None;
    // SAFETY: the class name is a literal; `after` is the window found last under `parent`.
    while let Ok(next) = unsafe { FindWindowExW(parent, after, class, PCWSTR::null()) } {
        found.push(next);
        after = Some(next);
    }
    found
}

/// First child of `parent` with window class `class`.
fn find_child(parent: HWND, class: PCWSTR) -> Option<HWND> {
    // SAFETY: the class name is a literal; a stale parent handle only fails the call.
    unsafe { FindWindowExW(Some(parent), None, class, PCWSTR::null()) }.ok()
}

/// The visible window holding the task buttons on Windows 10: `ReBarWindow32` on the primary
/// taskbar, the `WorkerW` around `MSTaskListWClass` on a secondary one.
fn task_container(taskbar: HWND, is_primary: bool) -> Option<HWND> {
    let container = if is_primary {
        find_child(taskbar, w!("ReBarWindow32"))
    } else {
        // Any `WorkerW` child may come first; only the one holding the task list counts.
        find_all(Some(taskbar), w!("WorkerW"))
            .into_iter()
            .find(|worker| find_child(*worker, w!("MSTaskListWClass")).is_some())
    }?;
    // SAFETY: IsWindowVisible accepts any handle value.
    unsafe { IsWindowVisible(container) }
        .as_bool()
        .then_some(container)
}

/// A child's window rect in its parent's client coordinates.
fn child_rect(child: HWND, parent: HWND) -> Option<Rect> {
    let origin = client_rect_on_screen(parent)?;
    let rect = window_rect(child)?;
    Some(Rect {
        x: rect.x - origin.x,
        y: rect.y - origin.y,
        ..rect
    })
}

/// Sets a taskbar child's rect; `SWP_NOSENDCHANGING` because Explorer's `WM_WINDOWPOSCHANGING`
/// handler snaps any outside shrink back to its own layout.
fn set_child_rect(window: HWND, rect: Rect, class: &str) {
    // Synchronous into Explorer's thread (no SWP_ASYNCWINDOWPOS): it returns when Explorer has
    // handled the move, so a busy taskbar thread stalls this one.
    let _span = log::span_fmt(format_args!("SetWindowPos {class} (Explorer, synchronous)"));
    // SAFETY: only repositions a window by handle; a stale handle fails the call.
    let moved = unsafe {
        SetWindowPos(
            window,
            None,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
        )
    };
    if let Err(error) = moved {
        eprintln!("resizing the taskbar's task buttons failed: {error}");
    }
}

/// The notification area in screen coordinates; an empty rect when it cannot be found.
fn notification_area(taskbar: HWND) -> Rect {
    find_child(taskbar, w!("TrayNotifyWnd"))
        .and_then(window_rect)
        .unwrap_or(Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        })
}

fn px(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Taskbar, notification-area and task-button geometry plus the Windows build, for `juicetray scan`.
pub fn describe_environment() -> String {
    let mut lines = Vec::new();
    match find_taskbar() {
        Some(taskbar) => {
            lines.push(format!(
                "Taskbar rect: {}",
                describe_rect(window_rect(taskbar))
            ));
            lines.push(format!(
                "Taskbar client rect: {}",
                describe_rect(client_rect_on_screen(taskbar))
            ));
            lines.push(format!(
                "TrayNotifyWnd rect: {}",
                describe_rect(find_child(taskbar, w!("TrayNotifyWnd")).and_then(window_rect))
            ));
            lines.push(format!(
                "Task buttons (ReBarWindow32) rect: {}",
                describe_rect(task_container(taskbar, true).and_then(window_rect))
            ));
        }
        None => lines.push("Taskbar: not found".to_owned()),
    }
    // Secondary taskbars dock left of their clock and shrink their WorkerW task buttons.
    for (index, taskbar) in secondary_taskbars().into_iter().enumerate() {
        let number = index + 1;
        lines.push(format!(
            "Secondary taskbar {number} rect: {}",
            describe_rect(window_rect(taskbar))
        ));
        lines.push(format!(
            "Secondary taskbar {number} task buttons (WorkerW) rect: {}",
            describe_rect(task_container(taskbar, false).and_then(window_rect))
        ));
        lines.push(format!(
            "Secondary taskbar {number} ClockButton rect: {}",
            describe_rect(find_child(taskbar, w!("ClockButton")).and_then(window_rect))
        ));
    }
    lines.push(format!("Windows build: {}", windows_build()));
    lines.join("\n")
}

fn describe_rect(rect: Option<Rect>) -> String {
    rect.map_or_else(
        || "unavailable".to_owned(),
        |rect| format!("x={} y={} w={} h={}", rect.x, rect.y, rect.w, rect.h),
    )
}

fn windows_build() -> String {
    os_version().map_or_else(
        || "unknown".to_owned(),
        |info| {
            format!(
                "{}.{}.{}",
                info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
            )
        },
    )
}

/// The real Windows version (RtlGetVersion ignores the compatibility manifest), read once: it
/// cannot change while the process runs.
fn os_version() -> Option<OSVERSIONINFOW> {
    use windows::Wdk::System::SystemServices::RtlGetVersion;

    static VERSION: OnceLock<Option<OSVERSIONINFOW>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is initialised with its own size, as the call requires.
        let status = unsafe { RtlGetVersion(&mut info) };
        status.is_ok().then_some(info)
    })
}
