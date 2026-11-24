use std::ptr::null_mut;
use std::sync::{Mutex, PoisonError};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, S_OK, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::Dialogs::{
    CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW, CommDlgExtendedError,
};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TDF_ALLOW_DIALOG_CANCELLATION,
    TDN_CREATED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DLGTEMPLATE, DS_CENTER, DS_MODALFRAME, DS_SETFONT,
    DS_SETFOREGROUND, DialogBoxIndirectParamW, ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA,
    GetDlgItem, GetDlgItemTextW, GetSystemMetrics, GetWindowLongPtrW, GetWindowTextLengthW,
    ICON_BIG, ICON_SMALL, IDCANCEL, IDOK, IMAGE_ICON, LR_SHARED, LoadImageW, MB_ICONINFORMATION,
    MB_OK, MB_SETFOREGROUND, MessageBoxW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON,
    SendMessageW, SetDlgItemTextW, SetForegroundWindow, SetWindowLongPtrW, WINDOW_STYLE,
    WM_COMMAND, WM_INITDIALOG, WM_SETICON, WS_BORDER, WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{BOOL, HRESULT, HSTRING, PCWSTR, s, w};

/// Predefined control classes, by the atoms dialog templates use for them.
pub(crate) const BUTTON_CLASS: u16 = 0x0080;
pub(crate) const EDIT_CLASS: u16 = 0x0081;
pub(crate) const STATIC_CLASS: u16 = 0x0082;
pub(crate) const COMBO_BOX_CLASS: u16 = 0x0085;
/// Marks the next template word as a class atom instead of a class name.
const ATOM_MARKER: u16 = 0xFFFF;

const MESSAGE_ID: u16 = 100;
const INPUT_ID: u16 = 101;

const FONT_POINTS: u16 = 9;
const FONT_FACE: &str = "Segoe UI";

/// Layout in dialog units `[x, y, width, height]`, per the Windows guidelines: 7 DLU margins,
/// 4 DLU between related controls, 7 DLU above the button row, 50x14 buttons right-aligned.
/// 260 wide like the Add device window; the message holds two lines.
const DIALOG_RECT: [i16; 4] = [0, 0, 260, 69];
const MESSAGE_RECT: [i16; 4] = [7, 7, 246, 16];
const INPUT_RECT: [i16; 4] = [7, 27, 246, 14];
const OK_RECT: [i16; 4] = [149, 48, 50, 14];
const CANCEL_RECT: [i16; 4] = [203, 48, 50, 14];
/// The alternative button's id in the three-button task dialogs.
const ALT_ID: i32 = 102;

/// The colour dialog's 16 custom slots, kept for the session; white marks an unused slot.
static CUSTOM_COLORS: Mutex<[COLORREF; 16]> = Mutex::new([COLORREF(0x00FF_FFFF); 16]);

/// What the dialog procedure reads at start and writes on OK.
struct Prompt {
    default: HSTRING,
    answer: Option<String>,
}

/// A modal dialog centred on the primary monitor; Enter is OK and Esc is Cancel.
pub fn prompt_text(title: &str, message: &str, default: &str) -> Option<String> {
    // SAFETY: a null module name asks for this executable's own handle, which never fails here.
    let instance = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => HINSTANCE::from(module),
        Err(error) => {
            eprintln!("text prompt failed: {error}");
            return None;
        }
    };
    let template = dialog_template(title, message);
    let mut prompt = Prompt {
        default: HSTRING::from(default),
        answer: None,
    };
    // SAFETY: the template is DWORD-aligned and well formed; `prompt` outlives the modal call,
    // which is the only time the dialog procedure reads it.
    let result = unsafe {
        DialogBoxIndirectParamW(
            Some(instance),
            template.as_ptr().cast::<DLGTEMPLATE>(),
            None,
            Some(dialog_proc),
            LPARAM(&raw mut prompt as isize),
        )
    };
    if result == -1 {
        eprintln!(
            "text prompt failed: {}",
            windows::core::Error::from_thread()
        );
    }
    prompt.answer
}

/// A modal yes/no dialog with `ok_label` as the default button; Enter confirms, Esc and the
/// `cancel_label` button answer no. Blocks the calling thread, so callers run it off the
/// event-loop thread.
pub fn confirm(title: &str, message: &str, ok_label: &str, cancel_label: &str) -> bool {
    let buttons = [(IDOK.0, ok_label), (IDCANCEL.0, cancel_label)];
    run_task_dialog(title, message, &buttons) == Some(IDOK.0)
}

/// The owner's pick in a three-button dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Ok,
    Alt,
    Cancel,
}

/// Like `confirm` with a second action: `ok_label` is the default button, `alt_label` the
/// middle one; Esc and the `cancel_label` button cancel. Blocks the calling thread.
pub fn confirm_or_alt(
    title: &str,
    message: &str,
    ok_label: &str,
    alt_label: &str,
    cancel_label: &str,
) -> Answer {
    let buttons = [
        (IDOK.0, ok_label),
        (ALT_ID, alt_label),
        (IDCANCEL.0, cancel_label),
    ];
    answer(run_task_dialog(title, message, &buttons))
}

/// Like `confirm_or_alt` with the cancel button in the middle and the alternative last, for a
/// final choice that Esc must never pick. Blocks the calling thread.
pub fn confirm_or_alt_last(
    title: &str,
    message: &str,
    ok_label: &str,
    cancel_label: &str,
    alt_label: &str,
) -> Answer {
    let buttons = [
        (IDOK.0, ok_label),
        (IDCANCEL.0, cancel_label),
        (ALT_ID, alt_label),
    ];
    answer(run_task_dialog(title, message, &buttons))
}

/// The pressed button as an `Answer`; Esc, the close box and a failed dialog cancel.
fn answer(pressed: Option<i32>) -> Answer {
    match pressed {
        Some(id) if id == IDOK.0 => Answer::Ok,
        Some(ALT_ID) => Answer::Alt,
        _ => Answer::Cancel,
    }
}

/// `TaskDialogIndirect`, which only Common Controls v6 exports.
type TaskDialogIndirectFn =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

/// Looks `TaskDialogIndirect` up at run time: a static import would stop any binary without the
/// v6 manifest (the test harness) from loading at all; `None` there.
fn task_dialog_indirect() -> Option<TaskDialogIndirectFn> {
    // SAFETY: comctl32 is a system library; the app manifest maps the name to v6.
    let module = unsafe { LoadLibraryW(w!("comctl32.dll")) }
        .inspect_err(|error| eprintln!("task dialog unavailable: {error}"))
        .ok()?;
    // SAFETY: `module` is loaded (and never freed); the name is NUL-terminated.
    let Some(proc) = (unsafe { GetProcAddress(module, s!("TaskDialogIndirect")) }) else {
        eprintln!("task dialog unavailable: Common Controls v6 is not active");
        return None;
    };
    // SAFETY: the export has exactly this signature (commctrl.h).
    Some(unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, TaskDialogIndirectFn>(proc)
    })
}

/// Splits a message into the task dialog's main instruction (up to the first blank line) and
/// its content (the rest, if any).
fn split_message(message: &str) -> (&str, Option<&str>) {
    match message.split_once("\n\n") {
        Some((instruction, content)) => (instruction, Some(content)),
        None => (message, None),
    }
}

/// A modal task dialog centred on the monitor: `title` on the caption, `message` as main
/// instruction and content (see `split_message`), and `(id, label)` push buttons in order with
/// the first as default. The pressed id (IDCANCEL for Esc or the close box); `None` on failure.
fn run_task_dialog(title: &str, message: &str, choices: &[(i32, &str)]) -> Option<i32> {
    let task_dialog = task_dialog_indirect()?;
    let (instruction, content) = split_message(message);
    let title = HSTRING::from(title);
    let instruction = HSTRING::from(instruction);
    let content = content.map(HSTRING::from);
    let labels: Vec<HSTRING> = choices.iter().map(|(_, label)| (*label).into()).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = choices
        .iter()
        .zip(&labels)
        .map(|((id, _), label)| TASKDIALOG_BUTTON {
            nButtonID: *id,
            pszButtonText: PCWSTR(label.as_ptr()),
        })
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        // Esc and the close box answer IDCANCEL, like the cancel button.
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: content
            .as_ref()
            .map_or(PCWSTR::null(), |content| PCWSTR(content.as_ptr())),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: choices.first().map_or(0, |(id, _)| *id),
        pfCallback: Some(task_dialog_callback),
        ..Default::default()
    };
    let mut pressed = 0;
    // SAFETY: `config` and every string and button it points at outlive the modal call; the
    // radio and verification outputs are not asked for.
    let result =
        unsafe { task_dialog(&raw const config, &raw mut pressed, null_mut(), null_mut()) };
    if let Err(error) = result.ok() {
        eprintln!("task dialog failed: {error}");
        return None;
    }
    Some(pressed)
}

/// Gives a new task dialog the app icon and the foreground, as DS_SETFOREGROUND does for the
/// template dialogs: it has no owner and opens from a tray app's background thread.
unsafe extern "system" fn task_dialog_callback(
    window: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    _wparam: WPARAM,
    _lparam: LPARAM,
    _data: isize,
) -> HRESULT {
    if notification == TDN_CREATED {
        set_app_icon(window);
        // SAFETY: `window` is the live task dialog; a refusal only leaves it behind.
        let _ = unsafe { SetForegroundWindow(window) };
    }
    S_OK
}

/// Puts the app icon (resource 1) on a dialog's title bar and taskbar button; dialogs get none
/// by default, so these ownerless ones showed a blank one.
pub(crate) fn set_app_icon(dialog: HWND) {
    // SAFETY: a null module name asks for this executable's own handle.
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let sizes = [
        (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
        (ICON_BIG, SM_CXICON, SM_CYICON),
    ];
    for (kind, width, height) in sizes {
        // SAFETY: resource 1 is the icon in juicetray.rc (an integer resource id); an LR_SHARED
        // icon belongs to the system, so nothing frees it.
        let icon = unsafe {
            LoadImageW(
                Some(module.into()),
                PCWSTR(1 as _),
                IMAGE_ICON,
                GetSystemMetrics(width),
                GetSystemMetrics(height),
                LR_SHARED,
            )
        };
        if let Ok(icon) = icon {
            // SAFETY: `dialog` is a live window; WM_SETICON only stores the handle.
            unsafe {
                SendMessageW(
                    dialog,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                )
            };
        }
    }
}

/// A modal message box with one OK button. Blocks the calling thread, so callers run it off the
/// event-loop thread.
pub fn notice(title: &str, message: &str) {
    // SAFETY: both strings are NUL-terminated and live until the call returns; no owner window.
    let _ = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            &HSTRING::from(title),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        )
    };
}

/// The system colour dialog, opened in full (custom colour) form on `initial`; `None` when the
/// owner cancels or the dialog fails.
pub fn pick_color((red, green, blue): (u8, u8, u8)) -> Option<(u8, u8, u8)> {
    // Held across the modal call: the dialog reads and updates the slots while it is open.
    let mut custom = CUSTOM_COLORS.lock().unwrap_or_else(PoisonError::into_inner);
    let mut choose = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        // COLORREF is 0x00BBGGRR.
        rgbResult: COLORREF(u32::from(red) | u32::from(green) << 8 | u32::from(blue) << 16),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    // SAFETY: `choose` is fully initialised with its true size; `lpCustColors` points at 16
    // COLORREFs that outlive the modal call; the owner is null, as for the text prompt.
    if !unsafe { ChooseColorW(&raw mut choose) }.as_bool() {
        // SAFETY: plain thread-local error query, valid right after the failed call.
        let error = unsafe { CommDlgExtendedError() };
        // Zero means the owner cancelled.
        if error.0 != 0 {
            eprintln!("colour picker failed: error {:#x}", error.0);
        }
        return None;
    }
    let rgb = choose.rgbResult.0;
    Some((rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8))
}

/// Fills the input at start, stores its text on OK and closes on OK or Cancel; the dialog
/// manager turns Enter into OK and Esc into Cancel.
unsafe extern "system" fn dialog_proc(
    dialog: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            // SAFETY: `lparam` is the `Prompt` pointer handed to DialogBoxIndirectParamW, valid
            // until it returns; the window keeps it for the WM_COMMAND below.
            let prompt = unsafe {
                SetWindowLongPtrW(dialog, GWLP_USERDATA, lparam.0);
                &*(lparam.0 as *const Prompt)
            };
            // SAFETY: `dialog` is the live dialog and INPUT_ID one of its controls.
            if let Err(error) = unsafe { SetDlgItemTextW(dialog, INPUT_ID.into(), &prompt.default) }
            {
                eprintln!("text prompt default not set: {error}");
            }
            set_app_icon(dialog);
            // TRUE lets the dialog manager focus the first tab stop, the input (text selected).
            1
        }
        WM_COMMAND => {
            let id = i32::from(wparam.0 as u16);
            if id != IDOK.0 && id != IDCANCEL.0 {
                return 0;
            }
            if id == IDOK.0 {
                // SAFETY: GWLP_USERDATA holds the `Prompt` pointer set at WM_INITDIALOG, still
                // valid inside the modal call; nothing else touches it meanwhile.
                let prompt =
                    unsafe { &mut *(GetWindowLongPtrW(dialog, GWLP_USERDATA) as *mut Prompt) };
                prompt.answer = Some(input_text(dialog));
            }
            // SAFETY: `dialog` is the live modal dialog.
            if let Err(error) = unsafe { EndDialog(dialog, id as isize) } {
                eprintln!("closing the text prompt failed: {error}");
            }
            1
        }
        _ => 0,
    }
}

/// The input control's current text.
fn input_text(dialog: HWND) -> String {
    // SAFETY: `dialog` is the live dialog; the buffer holds the text plus its terminator.
    unsafe {
        let length = GetDlgItem(Some(dialog), INPUT_ID.into())
            .map(|input| GetWindowTextLengthW(input))
            .unwrap_or_default();
        let mut buffer = vec![0u16; usize::try_from(length).unwrap_or_default() + 1];
        let copied = GetDlgItemTextW(dialog, INPUT_ID.into(), &mut buffer) as usize;
        String::from_utf16_lossy(&buffer[..copied.min(buffer.len())])
    }
}

/// An in-memory `DLGTEMPLATE`: caption, font, message, input, OK and Cancel. Returned as
/// `u32` words because the template must start on a DWORD boundary.
fn dialog_template(title: &str, message: &str) -> Vec<u32> {
    let mut words = TemplateWords::default();
    words.header(title, 4, DIALOG_RECT);

    let control = WS_CHILD | WS_VISIBLE;
    words.item(control, MESSAGE_RECT, MESSAGE_ID, STATIC_CLASS, message);
    words.item(
        control | WS_BORDER | WS_TABSTOP | dialog_style(ES_AUTOHSCROLL),
        INPUT_RECT,
        INPUT_ID,
        EDIT_CLASS,
        "",
    );
    words.item(
        control | WS_TABSTOP | dialog_style(BS_DEFPUSHBUTTON),
        OK_RECT,
        IDOK.0 as u16,
        BUTTON_CLASS,
        "OK",
    );
    words.item(
        control | WS_TABSTOP | dialog_style(BS_PUSHBUTTON),
        CANCEL_RECT,
        IDCANCEL.0 as u16,
        BUTTON_CLASS,
        "Cancel",
    );
    words.into_dwords()
}

/// Dialog, edit and button style constants are `i32` bit sets in the bindings.
pub(crate) fn dialog_style(bits: i32) -> WINDOW_STYLE {
    WINDOW_STYLE(bits as u32)
}

/// Little-endian 16-bit words of a dialog template.
#[derive(Default)]
pub(crate) struct TemplateWords(Vec<u16>);

impl TemplateWords {
    /// The `DLGTEMPLATE` header of a centred, captioned modal dialog in the app's font.
    pub(crate) fn header(&mut self, title: &str, item_count: u16, rect: [i16; 4]) {
        let style = WS_POPUP
            | WS_CAPTION
            | WS_SYSMENU
            | dialog_style(DS_MODALFRAME | DS_SETFONT | DS_CENTER | DS_SETFOREGROUND);
        self.dword(style.0);
        self.dword(0);
        self.word(item_count);
        self.rect(rect);
        // No menu, default dialog class.
        self.word(0);
        self.word(0);
        self.text(title);
        self.word(FONT_POINTS);
        self.text(FONT_FACE);
    }

    fn word(&mut self, value: u16) {
        self.0.push(value);
    }

    fn dword(&mut self, value: u32) {
        self.0.push(value as u16);
        self.0.push((value >> 16) as u16);
    }

    fn rect(&mut self, rect: [i16; 4]) {
        self.0.extend(rect.map(|value| value as u16));
    }

    /// A NUL-terminated UTF-16 string.
    fn text(&mut self, text: &str) {
        self.0.extend(text.encode_utf16());
        self.0.push(0);
    }

    /// One `DLGITEMTEMPLATE`, DWORD-aligned, with a predefined class and no creation data.
    pub(crate) fn item(
        &mut self,
        style: WINDOW_STYLE,
        rect: [i16; 4],
        id: u16,
        class: u16,
        text: &str,
    ) {
        if self.0.len() % 2 == 1 {
            self.0.push(0);
        }
        self.dword(style.0);
        self.dword(0);
        self.rect(rect);
        self.word(id);
        self.word(ATOM_MARKER);
        self.word(class);
        self.text(text);
        self.word(0);
    }

    pub(crate) fn into_dwords(self) -> Vec<u32> {
        self.0
            .chunks(2)
            .map(|pair| u32::from(pair[0]) | (u32::from(pair.get(1).copied().unwrap_or(0)) << 16))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_items_are_dword_aligned() {
        let template = dialog_template("Odd", "x");
        let words: Vec<u16> = template
            .iter()
            .flat_map(|dword| [*dword as u16, (dword >> 16) as u16])
            .collect();
        // Header: style, extended style, item count.
        assert_eq!(words[4], 4);
        // The first item follows the header (11 fixed words, "Odd\0", point size, "Segoe UI\0"),
        // padded to an even word index, and begins with its style.
        let header_words = 11 + 4 + 1 + 9;
        let first_item = header_words + header_words % 2;
        let style = u32::from(words[first_item]) | (u32::from(words[first_item + 1]) << 16);
        assert_eq!(style, (WS_CHILD | WS_VISIBLE).0);
    }

    #[test]
    fn message_splits_into_instruction_and_content() {
        assert_eq!(split_message("Add it?"), ("Add it?", None));
        assert_eq!(
            split_message("Add it?\n\nFound one.\n\nMore."),
            ("Add it?", Some("Found one.\n\nMore."))
        );
    }
}
