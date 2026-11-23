//! Layered child window docked into Explorer's taskbar, kept alive by a 2 s timer.
//! Every window procedure only forwards a signal to the event loop; nothing here blocks.

use std::ptr;
use std::sync::OnceLock;

use anyhow::{Result, bail};
use strip_render::Bitmap;
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
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW,
    GetClientRect, GetWindowRect, HWND_TOP, IsWindow, MA_NOACTIVATE, RegisterClassExW, SW_HIDE,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW, SendMessageW, SetTimer, SetWindowPos, ShowWindow,
    ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WNDPROC, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, PWSTR, w};

use super::layout::{Placement, Rect, place_strip};
use super::premultiplied_bgra;

const STRIP_CLASS: PCWSTR = w!("JuicetrayTaskbarStrip");
const KEEPER_CLASS: PCWSTR = w!("JuicetrayKeeper");
const KEEPER_TIMER_ID: usize = 1;
const KEEPER_INTERVAL_MS: u32 = 2000;
const TOOLTIP_TOOL_ID: usize = 1;
/// Wide enough that a tooltip's own line breaks decide the wrapping.
const TOOLTIP_MAX_WIDTH_PX: isize = 600;
/// Strip height as a share of the taskbar's client height, in percent.
const STRIP_HEIGHT_PERCENT: i32 = 55;

/// What a window procedure tells the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The 2 s keeper timer fired.
    KeeperTick,
    /// The strip was clicked with the left or right button.
    Click,
}

/// Window procedures are plain functions, so the callback lives in a process-wide slot.
static NOTIFY: OnceLock<Box<dyn Fn(Signal) + Send + Sync>> = OnceLock::new();

fn notify(signal: Signal) {
    if let Some(callback) = NOTIFY.get() {
        callback(signal);
    }
}

/// The strip child window and the tooltip it owns (destroyed together with it).
struct StripWindow {
    hwnd: HWND,
    tooltip: Option<HWND>,
}

pub struct TaskbarStrip {
    instance: HINSTANCE,
    /// The `Shell_TrayWnd` the strip window was created under.
    taskbar: HWND,
    strip: Option<StripWindow>,
    bitmap: Option<Bitmap>,
    tooltip: String,
    create_failure_logged: bool,
}

impl TaskbarStrip {
    /// Registers the window classes and starts the keeper timer; `callback` receives every signal.
    pub fn new(callback: impl Fn(Signal) + Send + Sync + 'static) -> Result<Self> {
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
        // The hidden top-level window (not message-only, not the child) lives for the whole
        // process and owns the keeper timer.
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
        Ok(Self {
            instance,
            taskbar: HWND::default(),
            strip: None,
            bitmap: None,
            tooltip: String::new(),
            create_failure_logged: false,
        })
    }

    /// Height of the strip bitmap for the primary taskbar, or `None` while it does not exist.
    pub fn strip_height() -> Option<u32> {
        let bar = client_rect_on_screen(find_taskbar()?)?;
        u32::try_from((bar.h * STRIP_HEIGHT_PERCENT + 50) / 100)
            .ok()
            .filter(|height| *height > 0)
    }

    /// DPI of the primary taskbar's monitor, or `None` while the taskbar does not exist.
    pub fn taskbar_dpi() -> Option<u32> {
        // SAFETY: only reads a property of a window handle; a stale handle gives 0.
        let dpi = unsafe { GetDpiForWindow(find_taskbar()?) };
        (dpi > 0).then_some(dpi)
    }

    /// Shows `bitmap` with `tooltip`, creating the window when it is missing.
    pub fn show(&mut self, bitmap: &Bitmap, tooltip: &str) {
        self.bitmap = Some(bitmap.clone());
        tooltip.clone_into(&mut self.tooltip);
        self.sync(true);
    }

    /// Keeper tick: recreates the window after an Explorer restart and re-docks it.
    pub fn keep(&mut self) {
        self.sync(false);
    }

    /// Removes the strip window and stops showing anything.
    pub fn hide(&mut self) {
        self.bitmap = None;
        if let Some(old) = self.strip.take() {
            old.destroy();
        }
    }

    /// Makes the window exist, paints it when `repaint` (or when new) and docks it.
    fn sync(&mut self, mut repaint: bool) {
        let Some(bitmap) = &self.bitmap else {
            return;
        };
        let Some(taskbar) = find_taskbar() else {
            // Explorer is gone; its windows, ours included, go with it. The next tick looks again.
            if let Some(old) = self.strip.take() {
                old.destroy();
            }
            return;
        };
        let alive = self.strip.as_ref().is_some_and(StripWindow::is_alive);
        if !alive || taskbar != self.taskbar {
            if let Some(old) = self.strip.take() {
                old.destroy();
            }
            self.taskbar = taskbar;
            match StripWindow::create(taskbar, self.instance, bitmap) {
                Ok(created) => {
                    self.strip = Some(created);
                    self.create_failure_logged = false;
                    repaint = true;
                }
                Err(error) => {
                    // The tray icon keeps working; log once, then retry quietly every tick.
                    if !self.create_failure_logged {
                        eprintln!("taskbar strip window failed (GetLastError): {error}");
                        self.create_failure_logged = true;
                    }
                    return;
                }
            }
        }
        let Some(strip) = &self.strip else {
            return;
        };
        if repaint {
            if let Err(error) = strip.paint(bitmap) {
                eprintln!("painting the taskbar strip failed: {error}");
            }
            strip.set_tooltip(&self.tooltip, bitmap);
        }
        strip.place(taskbar, bitmap);
    }
}

impl StripWindow {
    fn create(taskbar: HWND, instance: HINSTANCE, bitmap: &Bitmap) -> windows::core::Result<Self> {
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
            // SAFETY: the window belongs to this thread; destroying it also destroys its tooltip.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
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
        // SAFETY: `info` and `wide` outlive both calls; the control copies the text.
        unsafe {
            SendMessageW(tooltip, TTM_DELTOOLW, None, Some(lparam));
            SendMessageW(tooltip, TTM_ADDTOOLW, None, Some(lparam));
        }
    }

    /// Docks the window left of the notification area, or hides it when the layout does not fit.
    fn place(&self, taskbar: HWND, bitmap: &Bitmap) {
        let placement = client_rect_on_screen(taskbar).map_or(Placement::Fallback, |bar| {
            place_strip(
                bar,
                notification_area(taskbar),
                px(bitmap.width),
                px(bitmap.height),
            )
        });
        match placement {
            Placement::Docked(rect) => {
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
                if let Err(error) = moved {
                    eprintln!("moving the taskbar strip failed: {error}");
                }
            }
            Placement::Fallback => {
                // SAFETY: the window is ours; the result is the previous visibility, not an error.
                let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
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

/// Keeper timer: tell the event loop to run the keeper tick.
unsafe extern "system" fn keeper_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_TIMER && wparam.0 == KEEPER_TIMER_ID {
        notify(Signal::KeeperTick);
        return LRESULT(0);
    }
    // SAFETY: forwards the unchanged message arguments of this window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
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

fn notify_window(taskbar: HWND) -> Option<HWND> {
    // SAFETY: the class name is a literal; the taskbar handle may be stale, which only fails the call.
    unsafe { FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null()) }.ok()
}

/// The notification area in screen coordinates; an empty rect when it cannot be found.
fn notification_area(taskbar: HWND) -> Rect {
    notify_window(taskbar)
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

/// Taskbar and notification-area geometry plus the Windows build, for `juicetray scan`.
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
                describe_rect(notify_window(taskbar).and_then(window_rect))
            ));
        }
        None => lines.push("Taskbar: not found".to_owned()),
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
    use windows::Wdk::System::SystemServices::RtlGetVersion;

    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is initialised with its own size, as the call requires.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_ok() {
        format!(
            "{}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    } else {
        "unknown".to_owned()
    }
}
