//! The Win32 side of the Add device window: a dialog built in memory, run on its own thread,
//! that shows the flow's view and feeds it buttons, its timer and its own plug-in events. Device
//! work goes to the refresh worker; the answer comes back on a channel the timer checks.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tao::event_loop::EventLoopProxy;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL,
    CBN_SELCHANGE, CBS_DROPDOWNLIST, DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE, DLGTEMPLATE,
    DialogBoxIndirectParamW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, EndDialog,
    GWLP_USERDATA, GetDlgItem, GetDlgItemTextW, GetWindowLongPtrW, GetWindowTextLengthW, IDCANCEL,
    IDOK, KillTimer, PostMessageW, SW_HIDE, SW_SHOW, SendDlgItemMessageW, SetDlgItemTextW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, ShowWindow, WINDOW_STYLE, WM_APP, WM_COMMAND,
    WM_COPY, WM_DESTROY, WM_DEVICECHANGE, WM_INITDIALOG, WM_TIMER, WS_BORDER, WS_CHILD, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
};
use windows::core::HSTRING;

use super::flow::{Effect, Event, Flow, Input, Inspection, KINDS, TryOutcome, View};
use crate::app::UserEvent;
use crate::config::Config;
use crate::dialog::{
    BUTTON_CLASS, COMBO_BOX_CLASS, EDIT_CLASS, STATIC_CLASS, TemplateWords, dialog_style,
};
use crate::plug::{self, Id, Registration};
use crate::refresh::Command;
use crate::report::{self, Identity};
use crate::{log, update};

const TITLE: &str = "Add device";

const MESSAGE_ID: u16 = 100;
const LIST_ID: u16 = 101;
const KIND_LABEL_ID: u16 = 102;
const KIND_ID: u16 = 103;
const CONNECTION_LABEL_ID: u16 = 104;
const CONNECTION_ID: u16 = 105;
const NAME_LABEL_ID: u16 = 106;
const NAME_ID: u16 = 107;
const DETAILS_ID: u16 = 108;
const ALT_ID: u16 = 109;
/// The custom-device fields, shown and hidden together.
const FORM_IDS: [u16; 6] = [
    KIND_LABEL_ID,
    KIND_ID,
    CONNECTION_LABEL_ID,
    CONNECTION_ID,
    NAME_LABEL_ID,
    NAME_ID,
];
/// `SS_NOPREFIX`: a device name's `&` shows as itself.
const NO_PREFIX: i32 = 0x0080;

/// Layout in dialog units: `[x, y, width, height]`; a combo box's height includes its list.
const DIALOG_RECT: [i16; 4] = [0, 0, 260, 153];
const MESSAGE_RECT: [i16; 4] = [7, 7, 246, 48];
const LIST_RECT: [i16; 4] = [7, 58, 246, 90];
const KIND_LABEL_RECT: [i16; 4] = [7, 78, 44, 10];
const KIND_RECT: [i16; 4] = [54, 76, 100, 90];
const CONNECTION_LABEL_RECT: [i16; 4] = [7, 96, 44, 10];
const CONNECTION_RECT: [i16; 4] = [54, 94, 199, 60];
const NAME_LABEL_RECT: [i16; 4] = [7, 114, 44, 10];
const NAME_RECT: [i16; 4] = [54, 112, 199, 14];
const DETAILS_RECT: [i16; 4] = [7, 58, 246, 68];
const PRIMARY_RECT: [i16; 4] = [55, 132, 62, 14];
const ALT_RECT: [i16; 4] = [121, 132, 80, 14];
const CLOSE_RECT: [i16; 4] = [205, 132, 48, 14];

/// Posted by `open` to the running window: focus it and take the handed-over id.
const WM_HANDOVER: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
/// Shortest timer, so a due tick never spins.
const MIN_TICK: Duration = Duration::from_millis(50);

/// Whether the window is on screen (or starting); popups wait meanwhile.
static OPEN: AtomicBool = AtomicBool::new(false);
/// The running window's handle, 0 while none.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// An id waiting for the window: the preselected one at start, or a popup's [OK] later.
static HANDOVER: Mutex<Option<Id>> = Mutex::new(None);

/// Whether new-device popups must wait: the window detects devices itself.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Opens the window on an `add device` thread (menu: `None`; popup [OK]: the id), which sends
/// `UserEvent::AddDeviceClosed` when it closes. While it is open, a call focuses it and hands
/// the id over.
pub fn open(proxy: EventLoopProxy<UserEvent>, worker: Sender<Command>, id: Option<Id>) {
    if let Some(id) = id {
        *HANDOVER.lock().unwrap_or_else(PoisonError::into_inner) = Some(id);
    }
    if OPEN.swap(true, Ordering::Relaxed) {
        let window = WINDOW.load(Ordering::Relaxed);
        // A window still starting takes the id at WM_INITDIALOG instead.
        if window != 0 {
            // SAFETY: posting to a window of another thread only queues the message; a window
            // that closed meanwhile makes the call fail, which is harmless.
            let posted = unsafe {
                PostMessageW(
                    Some(HWND(window as *mut _)),
                    WM_HANDOVER,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
            if let Err(error) = posted {
                log::trace(format_args!("add device handover not posted: {error}"));
            }
        }
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("add device".into())
        .spawn(move || {
            run(proxy.clone(), worker);
            HANDOVER
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            OPEN.store(false, Ordering::Relaxed);
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(UserEvent::AddDeviceClosed);
        });
    if let Err(error) = spawned {
        OPEN.store(false, Ordering::Relaxed);
        log::line(&format!("add device window unavailable: {error}"));
    }
}

/// A worker reply the window waits for.
enum Pending {
    Inspect(Receiver<Inspection>),
    Try(Receiver<TryOutcome>),
}

/// What the dialog procedure works on; it lives on the thread's stack for the modal call.
struct Session {
    flow: Flow,
    worker: Sender<Command>,
    proxy: EventLoopProxy<UserEvent>,
    pending: Option<Pending>,
    registration: Option<Registration>,
    shown: Option<View>,
}

/// Runs the modal window until it closes.
fn run(proxy: EventLoopProxy<UserEvent>, worker: Sender<Command>) {
    let _span = log::span("add device window");
    let config = Config::load();
    let mut session = Session {
        flow: Flow::new(
            config.allowed(),
            config.custom(),
            report::environment(),
            Identity::current(),
        ),
        worker,
        proxy,
        pending: None,
        registration: None,
        shown: None,
    };
    // SAFETY: a null module name asks for this executable's own handle, which never fails here.
    let instance = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => HINSTANCE::from(module),
        Err(error) => {
            log::line(&format!("add device window failed: {error}"));
            return;
        }
    };
    let template = template();
    // SAFETY: the template is DWORD-aligned and well formed; `session` outlives the modal call,
    // which is the only time the dialog procedure reads it.
    let result = unsafe {
        DialogBoxIndirectParamW(
            Some(instance),
            template.as_ptr().cast::<DLGTEMPLATE>(),
            None,
            Some(window_proc),
            LPARAM(&raw mut session as isize),
        )
    };
    if result == -1 {
        log::line(&format!(
            "add device window failed: {}",
            windows::core::Error::from_thread()
        ));
    }
}

impl Session {
    /// Feeds `event` to the flow, performs its effect and shows the result.
    fn handle(&mut self, dialog: HWND, event: Event) {
        let effect = self.flow.on(event, Instant::now());
        // A reply for a step the flow left is ignored; the worker's send then fails quietly.
        if !self.flow.awaits_worker() {
            self.pending = None;
        }
        if let Some(effect) = effect {
            self.perform(dialog, effect);
        }
        self.update(dialog);
    }

    fn perform(&mut self, dialog: HWND, effect: Effect) {
        match effect {
            Effect::Inspect(id) => {
                let (reply, answer) = mpsc::channel();
                if self.worker.send(Command::Inspect { id, reply }).is_ok() {
                    self.pending = Some(Pending::Inspect(answer));
                } else {
                    self.handle(dialog, Event::Inspected(Inspection::Failed(stopped())));
                }
            }
            Effect::TryAs(id, family) => {
                let (reply, answer) = mpsc::channel();
                if self
                    .worker
                    .send(Command::TryAs { id, family, reply })
                    .is_ok()
                {
                    self.pending = Some(Pending::Try(answer));
                } else {
                    self.handle(dialog, Event::Tried(TryOutcome::Failed(stopped())));
                }
            }
            // A closed event loop means the app is exiting.
            Effect::Add(request) => {
                let _ = self.proxy.send_event(UserEvent::AddDevice(request));
            }
            Effect::Copy => {
                // The details box holds the full text: select it all and copy it.
                send(dialog, DETAILS_ID, EM_SETSEL, 0, -1);
                send(dialog, DETAILS_ID, WM_COPY, 0, 0);
            }
            Effect::OpenUrl(url) => {
                if let Err(error) = update::shell_open(&url) {
                    log::line(&format!("add device: opening the report failed: {error}"));
                }
            }
        }
    }

    /// Takes the worker's reply when it is in; a dropped sender counts as a failure.
    fn poll(&mut self, dialog: HWND) {
        let event = match &self.pending {
            Some(Pending::Inspect(answer)) => match answer.try_recv() {
                Ok(inspection) => Event::Inspected(inspection),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Event::Inspected(Inspection::Failed(stopped())),
            },
            Some(Pending::Try(answer)) => match answer.try_recv() {
                Ok(outcome) => Event::Tried(outcome),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Event::Tried(TryOutcome::NoReply),
            },
            None => return,
        };
        self.pending = None;
        self.handle(dialog, event);
    }

    /// Shows the flow's view and sets the timer to its next tick.
    fn update(&mut self, dialog: HWND) {
        let now = Instant::now();
        let view = self.flow.view(now);
        render(dialog, &view, self.shown.as_ref());
        self.shown = Some(view);
        match self.flow.next_tick(now) {
            Some(wait) => {
                let millis = u32::try_from(wait.max(MIN_TICK).as_millis()).unwrap_or(u32::MAX);
                // SAFETY: `dialog` is this thread's live window; a repeated id resets the timer.
                unsafe { SetTimer(Some(dialog), TIMER_ID, millis, None) };
            }
            None => {
                // SAFETY: as above; killing a timer that is not set only fails.
                let _ = unsafe { KillTimer(Some(dialog), TIMER_ID) };
            }
        }
    }

    /// An interface arrival or removal: Bluetooth arrivals only hint, USB ids drive detection.
    fn device_change(&mut self, dialog: HWND, wparam: WPARAM, lparam: LPARAM) {
        let arrived = wparam.0 == DBT_DEVICEARRIVAL as usize;
        if !arrived && wparam.0 != DBT_DEVICEREMOVECOMPLETE as usize {
            return;
        }
        // SAFETY: for these two events `lParam` is null or a DEV_BROADCAST_HDR valid during the
        // call.
        let Some(name) = (unsafe { plug::interface_name(lparam) }) else {
            return;
        };
        let event = if plug::is_bluetooth_interface(&name) {
            if !arrived {
                return;
            }
            Event::BluetoothArrived
        } else {
            let Some(id) = plug::parse_interface_name(&name) else {
                return;
            };
            if arrived {
                Event::Arrived(id)
            } else {
                Event::Removed(id)
            }
        };
        self.handle(dialog, event);
    }

    /// Focuses the window and starts on a handed-over id, if one waits.
    fn take_handover(&mut self, dialog: HWND) {
        let id = HANDOVER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match id {
            Some(id) => self.handle(dialog, Event::Handover(id)),
            None => self.update(dialog),
        }
    }
}

fn stopped() -> String {
    "the reader has stopped".to_owned()
}

/// Routes the dialog's messages to its `Session`.
unsafe extern "system" fn window_proc(
    dialog: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if message == WM_INITDIALOG {
        // SAFETY: `lparam` is the `Session` pointer handed to DialogBoxIndirectParamW, valid
        // until it returns; the window keeps it for later messages.
        let session = unsafe {
            SetWindowLongPtrW(dialog, GWLP_USERDATA, lparam.0);
            &mut *(lparam.0 as *mut Session)
        };
        WINDOW.store(dialog.0 as isize, Ordering::Relaxed);
        session.registration = plug::register(dialog);
        fill_combo(dialog, KIND_ID, &KINDS.map(|(label, _)| label), 0);
        session.take_handover(dialog);
        // TRUE lets the dialog manager focus the first tab stop.
        return 1;
    }
    // SAFETY: GWLP_USERDATA is 0 before WM_INITDIALOG and the `Session` pointer after it, valid
    // inside the modal call; only this thread touches it.
    let session = unsafe { (GetWindowLongPtrW(dialog, GWLP_USERDATA) as *mut Session).as_mut() };
    let Some(session) = session else {
        return 0;
    };
    match message {
        WM_COMMAND => {
            let id = wparam.0 as u16;
            let code = (wparam.0 >> 16) as u16;
            match i32::from(id) {
                ok if ok == IDOK.0 => session.handle(dialog, Event::Primary(input(dialog))),
                cancel if cancel == IDCANCEL.0 => {
                    // SAFETY: `dialog` is the live modal dialog.
                    if let Err(error) = unsafe { EndDialog(dialog, 0) } {
                        log::line(&format!("closing the add device window failed: {error}"));
                    }
                }
                _ if id == ALT_ID => session.handle(dialog, Event::Alt),
                _ if id == LIST_ID && u32::from(code) == CBN_SELCHANGE => {
                    session.handle(dialog, Event::Select(selection(dialog, LIST_ID)));
                }
                _ => return 0,
            }
            1
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            session.poll(dialog);
            session.handle(dialog, Event::Tick);
            1
        }
        WM_DEVICECHANGE => {
            session.device_change(dialog, wparam, lparam);
            // The default answer, so a removal query is never refused.
            0
        }
        WM_HANDOVER => {
            // SAFETY: plain call on this thread's live window; a refusal only leaves it behind.
            let _ = unsafe { SetForegroundWindow(dialog) };
            session.take_handover(dialog);
            1
        }
        WM_DESTROY => {
            // SAFETY: as in `update`.
            let _ = unsafe { KillTimer(Some(dialog), TIMER_ID) };
            session.registration = None;
            WINDOW.store(0, Ordering::Relaxed);
            0
        }
        _ => 0,
    }
}

/// Shows `view`, touching only what changed since `shown`, so a picked row or typed name stays.
fn render(dialog: HWND, view: &View, shown: Option<&View>) {
    let changed = |part: &dyn Fn(&View) -> bool| shown.is_none_or(part);
    if changed(&|old| old.text != view.text) {
        set_text(dialog, MESSAGE_ID, &view.text);
    }
    if changed(&|old| old.list != view.list) {
        fill_combo(dialog, LIST_ID, &view.list, view.picked);
        show(dialog, LIST_ID, !view.list.is_empty());
    } else if changed(&|old| old.picked != view.picked) {
        send(dialog, LIST_ID, CB_SETCURSEL, view.picked, 0);
    }
    if changed(&|old| old.form != view.form) {
        if let Some(form) = &view.form {
            fill_combo(dialog, CONNECTION_ID, &form.connections, 0);
            send(dialog, KIND_ID, CB_SETCURSEL, 0, 0);
            set_text(dialog, NAME_ID, &form.name);
        }
        for id in FORM_IDS {
            show(dialog, id, view.form.is_some());
        }
    }
    if changed(&|old| old.details != view.details) {
        // An edit control breaks lines on CR LF only.
        let text = view.details.as_deref().unwrap_or_default();
        set_text(dialog, DETAILS_ID, &text.replace('\n', "\r\n"));
        show(dialog, DETAILS_ID, view.details.is_some());
    }
    if changed(&|old| old.primary != view.primary) {
        set_text(
            dialog,
            IDOK.0 as u16,
            view.primary.as_deref().unwrap_or_default(),
        );
        show(dialog, IDOK.0 as u16, view.primary.is_some());
    }
    if changed(&|old| old.alt != view.alt) {
        set_text(dialog, ALT_ID, view.alt.unwrap_or_default());
        show(dialog, ALT_ID, view.alt.is_some());
    }
    if changed(&|old| old.close != view.close) {
        set_text(dialog, IDCANCEL.0 as u16, view.close);
    }
}

/// The picked rows, the kind and connection and the typed name.
fn input(dialog: HWND) -> Input {
    Input {
        list: selection(dialog, LIST_ID),
        kind: selection(dialog, KIND_ID),
        connection: selection(dialog, CONNECTION_ID),
        name: text(dialog, NAME_ID),
    }
}

/// A combo box's picked row; none counts as the first.
fn selection(dialog: HWND, id: u16) -> usize {
    usize::try_from(send(dialog, id, CB_GETCURSEL, 0, 0)).unwrap_or(0)
}

fn fill_combo(dialog: HWND, id: u16, rows: &[impl AsRef<str>], picked: usize) {
    send(dialog, id, CB_RESETCONTENT, 0, 0);
    for row in rows {
        let row = HSTRING::from(row.as_ref());
        // The control copies the string during the call.
        send(dialog, id, CB_ADDSTRING, 0, row.as_ptr() as isize);
    }
    send(dialog, id, CB_SETCURSEL, picked, 0);
}

/// Sends a message to a control of `dialog`; the control's answer.
fn send(dialog: HWND, id: u16, message: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: `dialog` is this thread's live window; every pointer passed in `lparam` outlives
    // the synchronous call.
    unsafe {
        SendDlgItemMessageW(
            dialog,
            i32::from(id),
            message,
            WPARAM(wparam),
            LPARAM(lparam),
        )
    }
    .0
}

fn set_text(dialog: HWND, id: u16, text: &str) {
    // SAFETY: `dialog` is the live dialog and `id` one of its controls.
    if let Err(error) = unsafe { SetDlgItemTextW(dialog, i32::from(id), &HSTRING::from(text)) } {
        log::trace(format_args!(
            "add device: control {id} text not set: {error}"
        ));
    }
}

fn show(dialog: HWND, id: u16, visible: bool) {
    // SAFETY: `dialog` is the live dialog; a missing control is skipped.
    unsafe {
        if let Ok(control) = GetDlgItem(Some(dialog), i32::from(id)) {
            let _ = ShowWindow(control, if visible { SW_SHOW } else { SW_HIDE });
        }
    }
}

/// A control's current text.
fn text(dialog: HWND, id: u16) -> String {
    // SAFETY: `dialog` is the live dialog; the buffer holds the text plus its terminator.
    unsafe {
        let length = GetDlgItem(Some(dialog), i32::from(id))
            .map(|control| GetWindowTextLengthW(control))
            .unwrap_or_default();
        let mut buffer = vec![0u16; usize::try_from(length).unwrap_or_default() + 1];
        let copied = GetDlgItemTextW(dialog, i32::from(id), &mut buffer) as usize;
        String::from_utf16_lossy(&buffer[..copied.min(buffer.len())])
    }
}

/// The window's template: message, picker, custom-device fields, details and three buttons; only
/// the message and the close button start visible.
fn template() -> Vec<u32> {
    let hidden = WS_CHILD;
    let visible = WS_CHILD | WS_VISIBLE;
    let combo = hidden | WS_TABSTOP | WS_VSCROLL | dialog_style(CBS_DROPDOWNLIST);
    let label = hidden | dialog_style(NO_PREFIX);
    let button = |kind: i32| WS_TABSTOP | dialog_style(kind);
    let items: [(WINDOW_STYLE, [i16; 4], u16, u16, &str); 12] = [
        (
            visible | dialog_style(NO_PREFIX),
            MESSAGE_RECT,
            MESSAGE_ID,
            STATIC_CLASS,
            "",
        ),
        (combo, LIST_RECT, LIST_ID, COMBO_BOX_CLASS, ""),
        (label, KIND_LABEL_RECT, KIND_LABEL_ID, STATIC_CLASS, "Kind"),
        (combo, KIND_RECT, KIND_ID, COMBO_BOX_CLASS, ""),
        (
            label,
            CONNECTION_LABEL_RECT,
            CONNECTION_LABEL_ID,
            STATIC_CLASS,
            "Connection",
        ),
        (combo, CONNECTION_RECT, CONNECTION_ID, COMBO_BOX_CLASS, ""),
        (label, NAME_LABEL_RECT, NAME_LABEL_ID, STATIC_CLASS, "Name"),
        (
            hidden | WS_BORDER | WS_TABSTOP | dialog_style(ES_AUTOHSCROLL),
            NAME_RECT,
            NAME_ID,
            EDIT_CLASS,
            "",
        ),
        (
            hidden
                | WS_BORDER
                | WS_TABSTOP
                | WS_VSCROLL
                | dialog_style(ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL),
            DETAILS_RECT,
            DETAILS_ID,
            EDIT_CLASS,
            "",
        ),
        (
            hidden | button(BS_DEFPUSHBUTTON),
            PRIMARY_RECT,
            IDOK.0 as u16,
            BUTTON_CLASS,
            "",
        ),
        (
            hidden | button(BS_PUSHBUTTON),
            ALT_RECT,
            ALT_ID,
            BUTTON_CLASS,
            "",
        ),
        (
            visible | button(BS_PUSHBUTTON),
            CLOSE_RECT,
            IDCANCEL.0 as u16,
            BUTTON_CLASS,
            "Cancel",
        ),
    ];
    let mut words = TemplateWords::default();
    words.header(TITLE, items.len() as u16, DIALOG_RECT);
    for (style, rect, id, class, text) in items {
        words.item(style, rect, id, class, text);
    }
    words.into_dwords()
}
