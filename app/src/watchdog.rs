//! Hang watchdog: the UI and taskbar threads each stamp a heartbeat, and a `watchdog` thread logs
//! every thread's stack to `juicetray.log` once one goes quiet, so a frozen taskbar shows where.
//! A context menu stuck open logs the other processes' hung windows its modal loop may wait on.

#[cfg(windows)]
pub use win::{Watched, beat, start, watch_current};

/// Silence longer than this counts as a hang; both heartbeat timers fire every 2 s and the
/// watchdog ticks every 2 s, so a healthy beat is up to ~4 s old when judged.
const STALL_MS: u64 = 6000;
/// A watchdog tick this late after the previous one means the machine slept, not the UI.
const GAP_MS: u64 = 5000;

/// What one watchdog tick reports.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Quiet,
    /// A new stall: dump the stacks now.
    Hang {
        silent_ms: u64,
    },
    /// The dumped stall ended after this long.
    Cleared {
        after_ms: u64,
    },
}

/// Stall bookkeeping between watchdog ticks; each stall is dumped at most once.
#[derive(Debug, Default)]
struct Stall {
    last_tick: u64,
    /// Silence counts from no earlier than this: the first tick or the end of a sleep gap.
    floor: u64,
    /// (heartbeat, silence start) of the stall already dumped.
    dumped: Option<(u64, u64)>,
}

impl Stall {
    /// Judges one tick at `now` given the latest `heartbeat`, both in ms since the epoch.
    fn check(&mut self, heartbeat: u64, now: u64) -> Verdict {
        if now.saturating_sub(self.last_tick) > GAP_MS {
            self.floor = now;
        }
        self.last_tick = now;
        if let Some((beat, start)) = self.dumped {
            if heartbeat == beat {
                return Verdict::Quiet;
            }
            self.dumped = None;
            return Verdict::Cleared {
                after_ms: heartbeat.saturating_sub(start),
            };
        }
        let start = heartbeat.max(self.floor);
        let silent_ms = now.saturating_sub(start);
        if silent_ms <= STALL_MS {
            return Verdict::Quiet;
        }
        self.dumped = Some((heartbeat, start));
        Verdict::Hang { silent_ms }
    }
}

#[cfg(windows)]
mod win {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Mutex, Once, PoisonError};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        ADDRESS64, AddrModeFlat, CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext, IMAGEHLP_MODULEW64,
        STACKFRAME64, SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_FAIL_CRITICAL_ERRORS,
        SYMOPT_NO_PROMPTS, SYMOPT_UNDNAME, StackWalk64, SymFromAddrW, SymFunctionTableAccess64,
        SymGetModuleBase64, SymGetModuleInfoW64, SymInitializeW, SymRefreshModuleList,
        SymSetOptions,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_AMD64;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, OpenThread,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
        THREAD_SUSPEND_RESUME,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, EnumWindows, GetWindowThreadProcessId, IsHungAppWindow,
        IsWindowVisible, RegisterClassExW, SetTimer, WINDOW_EX_STYLE, WM_TIMER, WNDCLASSEXW,
        WS_POPUP,
    };
    use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

    use super::{Stall, Verdict};
    use crate::log;

    const TICK: Duration = Duration::from_secs(2);
    const MAX_FRAMES: usize = 64;
    /// Longest symbol name kept, in UTF-16 units; longer generic names are cut.
    const MAX_NAME: usize = 512;
    const HEARTBEAT_CLASS: PCWSTR = w!("JuicetrayHeartbeat");
    const HEARTBEAT_TIMER_ID: usize = 1;
    const HEARTBEAT_INTERVAL_MS: u32 = 2000;

    /// A thread whose heartbeat the watchdog checks.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Watched {
        /// The tao event loop: dialogs run their modal loops here; menus run on their own thread.
        Ui,
        /// The thread owning the taskbar strip windows, input-attached to Explorer.
        Taskbar,
    }

    impl Watched {
        const ALL: [Self; 2] = [Self::Ui, Self::Taskbar];

        fn index(self) -> usize {
            self as usize
        }

        fn label(self) -> &'static str {
            match self {
                Self::Ui => "UI thread",
                Self::Taskbar => "taskbar thread",
            }
        }
    }

    /// Last beat per watched thread, in ms since the epoch.
    static HEARTBEATS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    /// Thread id per watched thread; 0 until it calls `watch_current`.
    static THREAD_IDS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
    /// dbghelp is single-threaded; every call into it holds this lock.
    static DBGHELP: Mutex<()> = Mutex::new(());

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    }

    /// Stamps `watched`'s heartbeat; called whenever that thread runs our code.
    pub fn beat(watched: Watched) {
        HEARTBEATS[watched.index()].store(now_ms(), Ordering::Relaxed);
    }

    /// Records the calling thread as `watched`, so the watchdog checks its heartbeat from now on.
    pub fn watch_current(watched: Watched) {
        beat(watched);
        // SAFETY: GetCurrentThreadId has no preconditions.
        THREAD_IDS[watched.index()].store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    }

    /// Gives the calling (UI) thread a heartbeat timer and starts the watchdog; call once from it.
    /// Returns the heartbeat window, which also hears plug-in and power broadcasts.
    pub fn start() -> Option<HWND> {
        let window = match start_heartbeat_timer() {
            Ok(window) => {
                watch_current(Watched::Ui);
                Some(window)
            }
            // Without its timer an idle UI thread would look hung, so it goes unwatched.
            Err(error) => {
                log::line(&format!("watchdog: UI heartbeat unavailable: {error}"));
                None
            }
        };
        let spawned = thread::Builder::new().name("watchdog".into()).spawn(watch);
        if let Err(error) = spawned {
            log::line(&format!("watchdog unavailable: {error}"));
        }
        window
    }

    /// A hidden top-level window on the calling thread whose 2 s timer beats the UI heartbeat.
    /// Modal loops (dialogs, the file picker) dispatch it too, so only a stuck loop or a
    /// blocked call goes silent; the window lives for the whole process.
    fn start_heartbeat_timer() -> windows::core::Result<HWND> {
        // SAFETY: a null module name asks for this executable's own handle.
        let instance = HINSTANCE::from(unsafe { GetModuleHandleW(None) }?);
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(heartbeat_proc),
            hInstance: instance,
            lpszClassName: HEARTBEAT_CLASS,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised; the name is a static literal.
        if unsafe { RegisterClassExW(&class) } == 0 {
            return Err(windows::core::Error::from_thread());
        }
        // SAFETY: the class is registered above; a top-level window gets no parent.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                HEARTBEAT_CLASS,
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
        // SAFETY: `window` was just created on this thread.
        if unsafe {
            SetTimer(
                Some(window),
                HEARTBEAT_TIMER_ID,
                HEARTBEAT_INTERVAL_MS,
                None,
            )
        } == 0
        {
            return Err(windows::core::Error::from_thread());
        }
        Ok(window)
    }

    unsafe extern "system" fn heartbeat_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_TIMER && wparam.0 == HEARTBEAT_TIMER_ID {
            beat(Watched::Ui);
            return LRESULT(0);
        }
        // Device arrivals and resume from sleep reach top-level windows; this one is always there.
        crate::plug::on_window_message(message, wparam, lparam);
        // SAFETY: forwards the unchanged message arguments of this window procedure.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Checks every watched heartbeat each `TICK`; a stall dumps every stack once and logs its end.
    fn watch() {
        crate::efficiency::enter_background_mode();
        let mut stalls: [Stall; 2] = Default::default();
        let mut reported_menu = None;
        loop {
            thread::sleep(TICK);
            check_menu(&mut reported_menu);
            let now = now_ms();
            for watched in Watched::ALL {
                // Not started (or never will be): nothing to judge yet.
                if THREAD_IDS[watched.index()].load(Ordering::Relaxed) == 0 {
                    continue;
                }
                let heartbeat = HEARTBEATS[watched.index()].load(Ordering::Relaxed);
                let label = watched.label();
                match stalls[watched.index()].check(heartbeat, now) {
                    Verdict::Quiet => {}
                    Verdict::Hang { silent_ms } => {
                        log::line(&format!("HANG: {label} silent for {silent_ms} ms"));
                        dump_all(watched);
                    }
                    Verdict::Cleared { after_ms } => {
                        log::line(&format!("HANG cleared: {label} after {after_ms} ms"));
                    }
                }
            }
        }
    }

    /// Logs a context menu open past `STUCK_AFTER` once, naming the hung windows its modal loop
    /// may wait on, and logs when that menu is gone (closed, or abandoned for a new one).
    fn check_menu(reported: &mut Option<u64>) {
        use crate::context_menu::{STUCK_AFTER, open_for};

        let open = open_for();
        match (open, *reported) {
            (Some((id, age)), _) if age >= STUCK_AFTER && *reported != Some(id) => {
                log::line(&format!(
                    "MENU stuck: open for {} ms; hung windows of other processes:",
                    age.as_millis()
                ));
                log_hung_windows();
                *reported = Some(id);
            }
            (open, Some(id)) if open.map(|(open_id, _)| open_id) != Some(id) => {
                log::line("MENU stuck cleared: that menu is gone");
                *reported = None;
            }
            _ => {}
        }
    }

    /// Logs each other process owning a hung top-level window as `exe (pid N, M windows, V
    /// visible)`; menu hit-testing only messages visible ones, so those name the likely culprit.
    fn log_hung_windows() {
        let mut windows: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes into `windows`, which outlives the call.
        let listed =
            unsafe { EnumWindows(Some(collect_window), LPARAM(&raw mut windows as isize)) };
        if let Err(error) = listed {
            log::line(&format!("  window list failed: {error}"));
            return;
        }
        // SAFETY: GetCurrentProcessId has no preconditions.
        let me = unsafe { GetCurrentProcessId() };
        // pid -> (hung windows, of which visible)
        let mut hung: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
        for window in windows {
            let mut pid = 0;
            // SAFETY: these only read window state without messaging it; a window closed since
            // the enumeration reads as not hung, not visible and pid 0.
            let (is_hung, is_visible) = unsafe {
                GetWindowThreadProcessId(window, Some(&mut pid));
                (
                    IsHungAppWindow(window).as_bool(),
                    IsWindowVisible(window).as_bool(),
                )
            };
            if is_hung && pid != 0 && pid != me {
                let (count, visible) = hung.entry(pid).or_default();
                *count += 1;
                *visible += u32::from(is_visible);
            }
        }
        if hung.is_empty() {
            log::line("  no hung windows");
        }
        for (pid, (count, visible)) in hung {
            log::line(&format!(
                "  hung: {} (pid {pid}, {count} windows, {visible} visible)",
                process_name(pid)
            ));
        }
    }

    unsafe extern "system" fn collect_window(window: HWND, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the `Vec<HWND>` handed to EnumWindows, alive for the whole call.
        unsafe { (*(list.0 as *mut Vec<HWND>)).push(window) };
        true.into()
    }

    /// The process's exe file name, or why it could not be read.
    fn process_name(pid: u32) -> String {
        let query = || -> windows::core::Result<String> {
            // SAFETY: plain access flags; the handle is closed by its guard.
            let process =
                Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }?);
            // MAX_PATH units: enough for an exe path outside the long-path opt-in.
            let mut buffer = [0u16; 260];
            let mut len = buffer.len() as u32;
            // SAFETY: `buffer` holds `len` units and both outlive the call.
            unsafe {
                QueryFullProcessImageNameW(
                    process.0,
                    PROCESS_NAME_WIN32,
                    PWSTR(buffer.as_mut_ptr()),
                    &mut len,
                )
            }?;
            let path = String::from_utf16_lossy(&buffer[..len as usize]);
            Ok(path.rsplit('\\').next().unwrap_or(&path).to_owned())
        };
        query().unwrap_or_else(|error| format!("? ({error})"))
    }

    /// Loads this process's module list; the exe folder is searched for `juicetray.pdb`.
    fn init_symbols() -> windows::core::Result<()> {
        let search = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(HSTRING::from));
        let search = search
            .as_ref()
            .map_or(PCWSTR::null(), |path| PCWSTR(path.as_ptr()));
        let _dbghelp = DBGHELP.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: only sets dbghelp's global option flags, under the dbghelp lock.
        unsafe {
            SymSetOptions(
                SYMOPT_UNDNAME
                    | SYMOPT_DEFERRED_LOADS
                    | SYMOPT_FAIL_CRITICAL_ERRORS
                    | SYMOPT_NO_PROMPTS,
            )
        };
        // SAFETY: the process pseudo handle is always valid; `search` outlives the call.
        unsafe { SymInitializeW(GetCurrentProcess(), search, true) }
    }

    /// Logs the `stalled` thread's stack, then the other watched one's, then every other
    /// thread's except the watchdog's own.
    fn dump_all(stalled: Watched) {
        // dbghelp loads its module list only for a real hang; an idle run never pays for it.
        static SYMBOLS: Once = Once::new();
        SYMBOLS.call_once(|| {
            if let Err(error) = init_symbols() {
                log::line(&format!("watchdog: SymInitialize failed: {error}"));
            }
        });
        let _dbghelp = DBGHELP.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: GetCurrentProcess returns a pseudo handle with no preconditions.
        let process = unsafe { GetCurrentProcess() };
        // Learns DLLs loaded since start (a USB driver stack, say) before any thread is paused.
        // SAFETY: `process` is the handle dbghelp was initialised with.
        let _ = unsafe { SymRefreshModuleList(process) };
        let watched: Vec<(u32, &str)> = std::iter::once(stalled)
            .chain(Watched::ALL.into_iter().filter(|&other| other != stalled))
            .map(|thread| {
                (
                    THREAD_IDS[thread.index()].load(Ordering::Relaxed),
                    thread.label(),
                )
            })
            .filter(|&(id, _)| id != 0)
            .collect();
        for &(id, label) in &watched {
            dump_thread(process, id, label);
        }
        // SAFETY: GetCurrentThreadId has no preconditions.
        let me = unsafe { GetCurrentThreadId() };
        match process_threads() {
            Ok(ids) => {
                let is_listed = |id: u32| id == me || watched.iter().any(|&(seen, _)| seen == id);
                for id in ids.into_iter().filter(|&id| !is_listed(id)) {
                    dump_thread(process, id, "thread");
                }
            }
            Err(error) => log::line(&format!("thread list failed: {error}")),
        }
        log::line("HANG dump end");
    }

    /// Logs one thread's frames as `  #n module!symbol+0xoff`.
    fn dump_thread(process: HANDLE, thread_id: u32, label: &str) {
        let mut frames = [0u64; MAX_FRAMES];
        match capture(process, thread_id, &mut frames) {
            Ok(count) => {
                log::line(&format!("{label} {thread_id}: {count} frames"));
                for (index, &address) in frames[..count].iter().enumerate() {
                    log::line(&format!("  #{index} {}", describe(process, address)));
                }
            }
            Err(error) => {
                log::line(&format!(
                    "{label} {thread_id}: stack capture failed: {error}"
                ));
            }
        }
    }

    /// Ids of this process's threads, from a Toolhelp snapshot.
    fn process_threads() -> windows::core::Result<Vec<u32>> {
        // SAFETY: plain flags; the snapshot handle is closed by its guard.
        let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }?);
        // SAFETY: GetCurrentProcessId has no preconditions.
        let pid = unsafe { GetCurrentProcessId() };
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut ids = Vec::new();
        // SAFETY: `entry` has its size set and outlives the call.
        let mut found = unsafe { Thread32First(snapshot.0, &mut entry) };
        while found.is_ok() {
            if entry.th32OwnerProcessID == pid {
                ids.push(entry.th32ThreadID);
            }
            // SAFETY: as above; the snapshot is still open.
            found = unsafe { Thread32Next(snapshot.0, &mut entry) };
        }
        Ok(ids)
    }

    /// x64 GetThreadContext needs a 16-byte aligned CONTEXT; the binding's struct is 8-aligned.
    #[repr(C, align(16))]
    struct AlignedContext(CONTEXT);

    /// Pauses `thread_id`, walks its stack into `frames` and resumes it. Nothing here logs or
    /// formats while the thread is paused: it may hold the log lock or the heap lock.
    fn capture(
        process: HANDLE,
        thread_id: u32,
        frames: &mut [u64; MAX_FRAMES],
    ) -> windows::core::Result<usize> {
        // SAFETY: plain access flags; the handle is closed by its guard.
        let thread = Handle(unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                false,
                thread_id,
            )
        }?);
        // SAFETY: the handle has THREAD_SUSPEND_RESUME; `_resume` undoes it on every path.
        if unsafe { SuspendThread(thread.0) } == u32::MAX {
            return Err(windows::core::Error::from_thread());
        }
        let _resume = Resume(&thread);
        let mut context = AlignedContext(CONTEXT {
            ContextFlags: CONTEXT_FULL_AMD64,
            ..Default::default()
        });
        // SAFETY: the thread is suspended and `context` is 16-byte aligned.
        unsafe { GetThreadContext(thread.0, &mut context.0) }?;
        let flat = |offset| ADDRESS64 {
            Offset: offset,
            Segment: 0,
            Mode: AddrModeFlat,
        };
        let mut frame = STACKFRAME64 {
            AddrPC: flat(context.0.Rip),
            AddrFrame: flat(context.0.Rbp),
            AddrStack: flat(context.0.Rsp),
            ..Default::default()
        };
        let mut count = 0;
        while count < MAX_FRAMES {
            // SAFETY: the thread stays suspended; `frame` and `context` are seeded from its
            // registers and outlive the call; the callbacks forward to dbghelp.
            let walked = unsafe {
                StackWalk64(
                    u32::from(IMAGE_FILE_MACHINE_AMD64.0),
                    process,
                    thread.0,
                    &mut frame,
                    (&raw mut context.0).cast(),
                    None,
                    Some(function_table),
                    Some(module_base),
                    None,
                )
            };
            if !walked.as_bool() || frame.AddrPC.Offset == 0 {
                break;
            }
            frames[count] = frame.AddrPC.Offset;
            count += 1;
        }
        Ok(count)
    }

    /// StackWalk64 takes `extern "system"` callbacks; the bindings are Rust functions.
    unsafe extern "system" fn function_table(process: HANDLE, base: u64) -> *mut c_void {
        // SAFETY: forwards StackWalk64's arguments unchanged.
        unsafe { SymFunctionTableAccess64(process, base) }
    }

    unsafe extern "system" fn module_base(process: HANDLE, address: u64) -> u64 {
        // SAFETY: forwards StackWalk64's arguments unchanged.
        unsafe { SymGetModuleBase64(process, address) }
    }

    /// `module!symbol+0xoff`, `module+0xoff` without a symbol, or the bare address.
    fn describe(process: HANDLE, address: u64) -> String {
        let module = module_name(process, address);
        match (symbol_name(process, address), module) {
            (Some((symbol, offset)), Some((module, _))) => format!("{module}!{symbol}+{offset:#x}"),
            (Some((symbol, offset)), None) => format!("?!{symbol}+{offset:#x}"),
            (None, Some((module, base))) => format!("{module}+{:#x}", address - base),
            (None, None) => format!("{address:#x}"),
        }
    }

    /// Module name and load address dbghelp knows for `address`.
    fn module_name(process: HANDLE, address: u64) -> Option<(String, u64)> {
        let mut module = IMAGEHLP_MODULEW64 {
            SizeOfStruct: size_of::<IMAGEHLP_MODULEW64>() as u32,
            ..Default::default()
        };
        // SAFETY: `module` has its size set and outlives the call.
        unsafe { SymGetModuleInfoW64(process, address, &mut module) }.ok()?;
        let name = &module.ModuleName;
        let len = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        Some((String::from_utf16_lossy(&name[..len]), module.BaseOfImage))
    }

    /// Symbol name and displacement for `address`, when the PDB or exports know it.
    fn symbol_name(process: HANDLE, address: u64) -> Option<(String, u64)> {
        // SYMBOL_INFOW ends in a one-unit name; the rest of the buffer holds the remainder.
        let mut buffer = [0u64; (size_of::<SYMBOL_INFOW>() + MAX_NAME * 2).div_ceil(8)];
        let info = buffer.as_mut_ptr().cast::<SYMBOL_INFOW>();
        let mut displacement = 0;
        // SAFETY: `buffer` is 8-byte aligned and holds the header plus MAX_NAME name units,
        // which is all dbghelp may write; the name is read back through the same pointer.
        unsafe {
            (*info).SizeOfStruct = size_of::<SYMBOL_INFOW>() as u32;
            (*info).MaxNameLen = MAX_NAME as u32;
            SymFromAddrW(process, address, Some(&mut displacement), info).ok()?;
            let len = ((*info).NameLen as usize).min(MAX_NAME);
            let name = std::slice::from_raw_parts((&raw const (*info).Name).cast::<u16>(), len);
            Some((String::from_utf16_lossy(name), displacement))
        }
    }

    /// Closes a kernel handle on drop.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from an open call and is closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// Resumes a suspended thread on drop, so every exit path undoes the suspend.
    struct Resume<'a>(&'a Handle);

    impl Drop for Resume<'_> {
        fn drop(&mut self) {
            // SAFETY: the handle has THREAD_SUSPEND_RESUME and was suspended once by `capture`.
            unsafe { ResumeThread(self.0.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_000_000;

    /// Ticks every 2 s (the watchdog tick) from `from` to `to` (inclusive) with a fixed heartbeat.
    fn tick_range(stall: &mut Stall, heartbeat: u64, from: u64, to: u64) -> Vec<Verdict> {
        (from..=to)
            .step_by(2000)
            .map(|now| stall.check(heartbeat, now))
            .collect()
    }

    #[test]
    fn dumps_once_per_stall_after_six_seconds() {
        let mut stall = Stall::default();
        let quiet = tick_range(&mut stall, T, T, T + 6000);
        assert!(quiet.iter().all(|verdict| *verdict == Verdict::Quiet));

        assert_eq!(stall.check(T, T + 8000), Verdict::Hang { silent_ms: 8000 });
        let later = tick_range(&mut stall, T, T + 10_000, T + 20_000);
        assert!(later.iter().all(|verdict| *verdict == Verdict::Quiet));
    }

    #[test]
    fn a_healthy_beat_never_reads_as_a_hang() {
        // Beats every 2 s, judged every 2 s with the worst phase (the beat just missed the
        // tick) and 0.5 s of timer lateness: still inside the stall limit.
        let mut stall = Stall::default();
        for tick in 0..100 {
            let now = T + tick * 2000;
            let heartbeat = now.saturating_sub(2000 + 500);
            assert_eq!(stall.check(heartbeat.max(T), now), Verdict::Quiet);
        }
    }

    #[test]
    fn a_resumed_heartbeat_clears_the_stall_and_rearms() {
        let mut stall = Stall::default();
        tick_range(&mut stall, T, T, T + 6000);
        assert_eq!(stall.check(T, T + 8000), Verdict::Hang { silent_ms: 8000 });
        assert_eq!(
            stall.check(T + 9000, T + 10_000),
            Verdict::Cleared { after_ms: 9000 }
        );

        let quiet = tick_range(&mut stall, T + 9000, T + 12_000, T + 14_000);
        assert!(quiet.iter().all(|verdict| *verdict == Verdict::Quiet));
        assert_eq!(
            stall.check(T + 9000, T + 16_000),
            Verdict::Hang { silent_ms: 7000 }
        );
    }

    #[test]
    fn a_sleep_gap_restarts_the_silence_clock() {
        let mut stall = Stall::default();
        assert_eq!(stall.check(T, T), Verdict::Quiet);
        // The machine slept a minute: the stale heartbeat is not a hang yet.
        let quiet = tick_range(&mut stall, T, T + 60_000, T + 66_000);
        assert!(quiet.iter().all(|verdict| *verdict == Verdict::Quiet));
        assert_eq!(
            stall.check(T, T + 68_000),
            Verdict::Hang { silent_ms: 8000 }
        );
    }
}
