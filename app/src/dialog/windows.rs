use std::sync::{Mutex, PoisonError};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW, CommDlgExtendedError,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DLGTEMPLATE, DS_CENTER, DS_MODALFRAME, DS_SETFONT,
    DS_SETFOREGROUND, DialogBoxIndirectParamW, ES_AUTOHSCROLL, EndDialog, GWLP_USERDATA,
    GetDlgItem, GetDlgItemTextW, GetWindowLongPtrW, GetWindowTextLengthW, IDCANCEL, IDOK,
    MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW, SetDlgItemTextW, SetWindowLongPtrW,
    WINDOW_STYLE, WM_COMMAND, WM_INITDIALOG, WS_BORDER, WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};
use windows::core::HSTRING;

/// Predefined control classes, by the atoms dialog templates use for them.
const BUTTON_CLASS: u16 = 0x0080;
const EDIT_CLASS: u16 = 0x0081;
const STATIC_CLASS: u16 = 0x0082;
/// Marks the next template word as a class atom instead of a class name.
const ATOM_MARKER: u16 = 0xFFFF;

const MESSAGE_ID: u16 = 100;
const INPUT_ID: u16 = 101;

const FONT_POINTS: u16 = 9;
const FONT_FACE: &str = "Segoe UI";

/// Layout in dialog units: `[x, y, width, height]`.
const DIALOG_RECT: [i16; 4] = [0, 0, 220, 84];
const MESSAGE_RECT: [i16; 4] = [7, 7, 206, 26];
const INPUT_RECT: [i16; 4] = [7, 38, 206, 14];
const OK_RECT: [i16; 4] = [109, 62, 50, 14];
const CANCEL_RECT: [i16; 4] = [163, 62, 50, 14];
/// The confirm dialog has no input, so its buttons sit right under the message.
const CONFIRM_RECT: [i16; 4] = [0, 0, 220, 58];
const CONFIRM_OK_RECT: [i16; 4] = [109, 38, 50, 14];
const CONFIRM_CANCEL_RECT: [i16; 4] = [163, 38, 50, 14];
/// The middle button's control id in the three-button dialog.
const ALT_ID: u16 = 102;
const THREE_WAY_RECT: [i16; 4] = [0, 0, 250, 58];
const THREE_WAY_MESSAGE_RECT: [i16; 4] = [7, 7, 236, 26];
const THREE_WAY_OK_RECT: [i16; 4] = [37, 38, 64, 14];
const THREE_WAY_ALT_RECT: [i16; 4] = [105, 38, 88, 14];
const THREE_WAY_CANCEL_RECT: [i16; 4] = [197, 38, 46, 14];

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
    let template = confirm_template(title, message, ok_label, cancel_label);
    run_choice(&template) == Some(IDOK.0 as isize)
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
    let template = three_way_template(title, message, ok_label, alt_label, cancel_label);
    match run_choice(&template) {
        Some(id) if id == IDOK.0 as isize => Answer::Ok,
        Some(id) if id == ALT_ID as isize => Answer::Alt,
        _ => Answer::Cancel,
    }
}

/// Runs a button-only dialog; the id of the pressed button, `None` when the dialog failed.
fn run_choice(template: &[u32]) -> Option<isize> {
    // SAFETY: a null module name asks for this executable's own handle, which never fails here.
    let instance = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => HINSTANCE::from(module),
        Err(error) => {
            eprintln!("confirm dialog failed: {error}");
            return None;
        }
    };
    // SAFETY: the template is DWORD-aligned and well formed; the dialog procedure reads no
    // creation data.
    let result = unsafe {
        DialogBoxIndirectParamW(
            Some(instance),
            template.as_ptr().cast::<DLGTEMPLATE>(),
            None,
            Some(confirm_proc),
            LPARAM(0),
        )
    };
    if result == -1 {
        eprintln!(
            "confirm dialog failed: {}",
            windows::core::Error::from_thread()
        );
        return None;
    }
    Some(result)
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

/// Closes the confirm dialog with the id of the button pressed (OK, the middle one or Cancel).
unsafe extern "system" fn confirm_proc(
    dialog: HWND,
    message: u32,
    wparam: WPARAM,
    _lparam: LPARAM,
) -> isize {
    match message {
        // TRUE lets the dialog manager focus the first tab stop, the default button.
        WM_INITDIALOG => 1,
        WM_COMMAND => {
            let id = i32::from(wparam.0 as u16);
            if id != IDOK.0 && id != IDCANCEL.0 && id != i32::from(ALT_ID) {
                return 0;
            }
            // SAFETY: `dialog` is the live modal dialog.
            if let Err(error) = unsafe { EndDialog(dialog, id as isize) } {
                eprintln!("closing the confirm dialog failed: {error}");
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
    let style = WS_POPUP
        | WS_CAPTION
        | WS_SYSMENU
        | dialog_style(DS_MODALFRAME | DS_SETFONT | DS_CENTER | DS_SETFOREGROUND);
    words.dword(style.0);
    words.dword(0);
    let item_count = 4;
    words.word(item_count);
    words.rect(DIALOG_RECT);
    // No menu, default dialog class.
    words.word(0);
    words.word(0);
    words.text(title);
    words.word(FONT_POINTS);
    words.text(FONT_FACE);

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

/// An in-memory `DLGTEMPLATE` for the confirm dialog: caption, font, message, `ok_label` and
/// `cancel_label`.
fn confirm_template(title: &str, message: &str, ok_label: &str, cancel_label: &str) -> Vec<u32> {
    button_template(
        title,
        message,
        [CONFIRM_RECT, MESSAGE_RECT],
        &[
            (IDOK.0 as u16, CONFIRM_OK_RECT, ok_label),
            (IDCANCEL.0 as u16, CONFIRM_CANCEL_RECT, cancel_label),
        ],
    )
}

/// The confirm dialog with a middle button, wider so the longer labels fit.
fn three_way_template(
    title: &str,
    message: &str,
    ok_label: &str,
    alt_label: &str,
    cancel_label: &str,
) -> Vec<u32> {
    button_template(
        title,
        message,
        [THREE_WAY_RECT, THREE_WAY_MESSAGE_RECT],
        &[
            (IDOK.0 as u16, THREE_WAY_OK_RECT, ok_label),
            (ALT_ID, THREE_WAY_ALT_RECT, alt_label),
            (IDCANCEL.0 as u16, THREE_WAY_CANCEL_RECT, cancel_label),
        ],
    )
}

/// A caption, the font, a message and push buttons `(id, rect, label)`; the first is the default.
fn button_template(
    title: &str,
    message: &str,
    [dialog_rect, message_rect]: [[i16; 4]; 2],
    buttons: &[(u16, [i16; 4], &str)],
) -> Vec<u32> {
    let mut words = TemplateWords::default();
    let style = WS_POPUP
        | WS_CAPTION
        | WS_SYSMENU
        | dialog_style(DS_MODALFRAME | DS_SETFONT | DS_CENTER | DS_SETFOREGROUND);
    words.dword(style.0);
    words.dword(0);
    words.word(1 + buttons.len() as u16);
    words.rect(dialog_rect);
    // No menu, default dialog class.
    words.word(0);
    words.word(0);
    words.text(title);
    words.word(FONT_POINTS);
    words.text(FONT_FACE);

    let control = WS_CHILD | WS_VISIBLE;
    words.item(control, message_rect, MESSAGE_ID, STATIC_CLASS, message);
    for (index, (id, rect, label)) in buttons.iter().enumerate() {
        let kind = if index == 0 {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        words.item(
            control | WS_TABSTOP | dialog_style(kind),
            *rect,
            *id,
            BUTTON_CLASS,
            label,
        );
    }
    words.into_dwords()
}

/// Dialog, edit and button style constants are `i32` bit sets in the bindings.
fn dialog_style(bits: i32) -> WINDOW_STYLE {
    WINDOW_STYLE(bits as u32)
}

/// Little-endian 16-bit words of a dialog template.
#[derive(Default)]
struct TemplateWords(Vec<u16>);

impl TemplateWords {
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
    fn item(&mut self, style: WINDOW_STYLE, rect: [i16; 4], id: u16, class: u16, text: &str) {
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

    fn into_dwords(self) -> Vec<u32> {
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
    fn confirm_template_has_three_aligned_items() {
        let template = confirm_template("Odd", "x", "Turn on", "Cancel");
        let words: Vec<u16> = template
            .iter()
            .flat_map(|dword| [*dword as u16, (dword >> 16) as u16])
            .collect();
        assert_eq!(words[4], 3);
        // Same header as the text prompt: 11 fixed words, "Odd\0", point size, "Segoe UI\0".
        let header_words = 11 + 4 + 1 + 9;
        let first_item = header_words + header_words % 2;
        let style = u32::from(words[first_item]) | (u32::from(words[first_item + 1]) << 16);
        assert_eq!(style, (WS_CHILD | WS_VISIBLE).0);
    }

    #[test]
    fn three_way_template_has_four_items() {
        let template = three_way_template("Odd", "x", "Update now", "Open download page", "Later");
        let words: Vec<u16> = template
            .iter()
            .flat_map(|dword| [*dword as u16, (dword >> 16) as u16])
            .collect();
        assert_eq!(words[4], 4);
    }
}
