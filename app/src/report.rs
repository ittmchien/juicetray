//! Reports the owner sends to GitHub: the prefilled new-issue URL and the redaction every copied
//! or reported text gets first. The app never creates an issue itself (that needs a token).

use crate::scan::redact_path;

const NEW_ISSUE_URL: &str = "https://github.com/ittmchien/juicetray/issues/new";
/// Longest prefilled URL; GitHub and browsers refuse much longer ones.
pub const MAX_URL_LEN: usize = 7000;
/// Ends a body cut to fit the URL, so the owner knows the full text is on the clipboard path.
const TRUNCATED: &str = "…(truncated, use Copy)";

/// The new-issue page with `title` and `body` filled in, percent-encoded. A body too long is cut
/// a whole encoded character at a time and ends with `TRUNCATED`, so the URL stays within
/// `MAX_URL_LEN` and never ends inside a `%XX` escape or a multi-byte character.
pub fn issue_url(title: &str, body: &str) -> String {
    let mut url = format!("{NEW_ISSUE_URL}?title={}&body=", encode(title));
    let whole = encode(body);
    if url.len() + whole.len() <= MAX_URL_LEN {
        url.push_str(&whole);
        return url;
    }
    let note = encode(TRUNCATED);
    let limit = MAX_URL_LEN.saturating_sub(note.len());
    let mut buffer = [0u8; 4];
    for character in body.chars() {
        let piece = encode(character.encode_utf8(&mut buffer));
        if url.len() + piece.len() > limit {
            break;
        }
        url.push_str(&piece);
    }
    url.push_str(&note);
    url
}

/// The app version and Windows build, the last line of every report.
#[cfg(windows)]
pub fn environment() -> String {
    format!(
        "Juicetray v{}, Windows {}",
        env!("CARGO_PKG_VERSION"),
        crate::taskbar::windows_build()
    )
}

/// Percent-encodes everything but the URL-safe unreserved characters.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// What identifies this PC in a text: the user profile folder in each spelling and the computer
/// name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// The profile folder and the OneDrive folders, longest first.
    pub profiles: Vec<String>,
    pub computer: Option<String>,
}

impl Identity {
    /// This PC's profile, OneDrive and computer names from the environment.
    #[cfg(windows)]
    pub fn current() -> Self {
        let value = |name: &str| std::env::var(name).ok().filter(|text| text.len() > 1);
        let mut profiles: Vec<String> = [
            "OneDriveCommercial",
            "OneDriveConsumer",
            "OneDrive",
            "USERPROFILE",
        ]
        .into_iter()
        .filter_map(value)
        .collect();
        profiles.sort_by_key(|path| std::cmp::Reverse(path.len()));
        profiles.dedup();
        Self {
            profiles,
            computer: value("COMPUTERNAME"),
        }
    }
}

/// `text` without anything that identifies the owner or the PC: raw device paths keep only
/// their ids, profile folders (also 8.3 short and OneDrive forms) become `%USERPROFILE%`, the
/// computer name `<pc>` and Bluetooth addresses `<address>`. Serial numbers never enter a report.
pub fn redact(text: &str, identity: &Identity) -> String {
    let mut text = redact_device_paths(text);
    for profile in &identity.profiles {
        text = replace_ignoring_case(&text, profile, "%USERPROFILE%", false);
    }
    text = redact_users_folders(&text);
    // Whole words only: a short computer name also spells hex digits inside ids and bytes.
    if let Some(computer) = &identity.computer {
        text = replace_ignoring_case(&text, computer, "<pc>", true);
    }
    redact_addresses(&text)
}

/// Each `\\?\…` token keeps its ids only, as `scan` prints it.
fn redact_device_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(r"\\?\") {
        out.push_str(&rest[..start]);
        let token_end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |end| start + end);
        out.push_str(&redact_path(&rest[start..token_end]));
        rest = &rest[token_end..];
    }
    out.push_str(rest);
    out
}

/// `<drive>:\Users\<name>` in any spelling (an 8.3 short name too) becomes `%USERPROFILE%`. A
/// name may hold spaces, so it runs to the next separator, quote or line end.
fn redact_users_folders(text: &str) -> String {
    const USERS: &str = r":\users\";
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = lower[from..].find(USERS).map(|at| from + at) {
        from = found + USERS.len();
        let has_drive = found > 0 && text.as_bytes()[found - 1].is_ascii_alphabetic();
        if !has_drive || found - 1 < copied {
            continue;
        }
        let name_end = text[from..]
            .find(['\\', '/', '"', '\'', '\n', '\r'])
            .map_or(text.len(), |end| from + end);
        out.push_str(&text[copied..found - 1]);
        out.push_str("%USERPROFILE%");
        copied = name_end;
        from = name_end;
    }
    out.push_str(&text[copied..]);
    out
}

/// Six hex pairs joined by `:` or `-` become `<address>`.
fn redact_addresses(text: &str) -> String {
    const LEN: usize = 17;
    let bytes = text.as_bytes();
    let is_address = |at: usize| {
        bytes.get(at..at + LEN).is_some_and(|window| {
            let separator = window[2];
            matches!(separator, b':' | b'-')
                && window.iter().enumerate().all(|(index, &byte)| {
                    if index % 3 == 2 {
                        byte == separator
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
        })
    };
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if is_address(index) {
            out.push_str("<address>");
            index += LEN;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or_default();
        out.push(character);
        index += character.len_utf8().max(1);
    }
    out
}

/// Replaces every ASCII-case-insensitive match of `needle`; with `whole_word`, only matches no
/// ASCII letter or digit touches on either side.
fn replace_ignoring_case(text: &str, needle: &str, with: &str, whole_word: bool) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    // ASCII lowering keeps every byte offset, so matches map back onto `text`.
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let is_word_byte = |at: Option<usize>| {
        at.and_then(|at| lower.as_bytes().get(at))
            .is_some_and(u8::is_ascii_alphanumeric)
    };
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in lower.match_indices(&needle) {
        let inside_word = is_word_byte(at.checked_sub(1)) || is_word_byte(Some(at + needle.len()));
        if at < copied || (whole_word && inside_word) {
            continue;
        }
        out.push_str(&text[copied..at]);
        out.push_str(with);
        copied = at + needle.len();
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            profiles: vec![
                r"C:\Users\Jane Doe\OneDrive - Contoso".into(),
                r"C:\Users\Jane Doe".into(),
            ],
            computer: Some("DESKTOP-AB12".into()),
        }
    }

    #[test]
    fn redaction_covers_profiles_short_names_pc_paths_and_addresses() {
        let text = concat!(
            r"log at c:\users\jane doe\AppData\x.log; short C:\Users\JANEDO~1\AppData; ",
            r"docs C:\Users\Jane Doe\OneDrive - Contoso\a.txt; on desktop-ab12; ",
            r"BT 00:1A:7D:DA:71:13 or 00-1a-7d-da-71-13; ",
            r"path \\?\hid#vid_373b&pid_1216&mi_01#8&1a2b3c&0&0001#{4d1e55b2} end"
        );
        let redacted = redact(text, &identity());
        for leak in [
            "jane", "JANEDO", "Contoso", "ab12", "7D:DA", "7d-da", "1a2b3c",
        ] {
            assert!(
                !redacted
                    .to_ascii_lowercase()
                    .contains(&leak.to_ascii_lowercase()),
                "{leak} in {redacted}"
            );
        }
        assert!(
            redacted.contains(r"%USERPROFILE%\AppData\x.log"),
            "{redacted}"
        );
        assert!(redacted.contains(r"%USERPROFILE%\a.txt"), "{redacted}");
        assert!(redacted.contains("on <pc>;"), "{redacted}");
        assert!(
            redacted.contains("BT <address> or <address>;"),
            "{redacted}"
        );
        assert!(redacted.contains("vid_373b&pid_1216"), "{redacted}");
        assert!(redacted.ends_with(" end"), "{redacted}");
        // Nothing to hide: unchanged, non-ASCII included.
        let plain = "Chuột không dây 3554:FA09 · FF02:0002";
        assert_eq!(redact(plain, &identity()), plain);
        assert_eq!(redact(plain, &Identity::default()), plain);
    }

    #[test]
    fn issue_url_encodes_and_cuts_on_whole_characters() {
        let url = issue_url("Device 3554:FA09", "a b&c=d\nđ");
        assert_eq!(
            url,
            "https://github.com/ittmchien/juicetray/issues/new?title=Device%203554%3AFA09&body=a%20b%26c%3Dd%0A%C4%91"
        );

        let long = "đ".repeat(5000);
        let url = issue_url("t", &long);
        assert!(url.len() <= MAX_URL_LEN);
        // A cut body says so; each `đ` is two escapes and the cut never leaves half of one.
        let body = url.split("&body=").nth(1).unwrap();
        let body = body
            .strip_suffix(&encode(TRUNCATED))
            .expect("truncation note");
        assert_eq!(body.len() % 6, 0);
        assert!(body.len() > MAX_URL_LEN - 150);
        let short = issue_url("t", "x");
        assert!(short.ends_with("&body=x"));
        // A body that just fits gets no note.
        let fits = "x".repeat(MAX_URL_LEN - issue_url("t", "").len());
        assert_eq!(issue_url("t", &fits).len(), MAX_URL_LEN);
        assert!(issue_url("t", &fits).ends_with('x'));
    }

    #[test]
    fn profile_names_with_spaces_and_short_computer_names() {
        let other_user = redact(
            "log C:\\Users\\John Smith\\AppData\\x.log\nnext C:\\Users\\Ann\nInterfaces:",
            &Identity::default(),
        );
        assert_eq!(
            other_user,
            "log %USERPROFILE%\\AppData\\x.log\nnext %USERPROFILE%\nInterfaces:"
        );
        // A short computer name is replaced as a word, never inside hex.
        let short = Identity {
            profiles: Vec::new(),
            computer: Some("AB12".into()),
        };
        assert_eq!(
            redact("bytes 0xFFAB12 AB12CD on ab12.", &short),
            "bytes 0xFFAB12 AB12CD on <pc>."
        );
    }
}
