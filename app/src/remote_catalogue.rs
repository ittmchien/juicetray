//! Looks up an id outside the built-in catalogue in the GitHub `main` catalogue files (Windows
//! popup): a usable row becomes the custom device offered; any failure means "no row".

use protocol::family_vid;
use toml::{Table, Value};

use crate::config::{CustomEntry, custom_device};
use crate::log;
use crate::plug::Id;
use crate::query::parse_vid_pid;

/// Where the catalogue files are served raw; a file name from `protocol::CATALOGUE_FILES` follows.
#[cfg(windows)]
const RAW_BASE: &str =
    "https://raw.githubusercontent.com/ittmchien/juicetray/main/crates/protocol/devices/";
/// A catalogue file is a few KB; anything past this is refused.
#[cfg(windows)]
const MAX_FILE_BYTES: usize = 256 * 1024;
/// Per-step WinHTTP wait: GitHub answers in well under a second, and a hung network (captive
/// portal, blocked host) should delay the popup by seconds, not the update check's 10 s steps.
#[cfg(windows)]
const TIMEOUT_MS: i32 = 3_000;
/// Longest row name shown in the popup and saved to the config.
const MAX_NAME_CHARS: usize = 64;

/// Why a file gave no rows.
#[derive(Debug)]
enum FetchError {
    /// No reply at all: every other file would wait out the same timeouts, so the lookup ends.
    Offline(String),
    /// A reply that is not a readable file (HTTP error, too large, not UTF-8): only it is skipped.
    File(String),
}

/// One catalogue row as a remote file holds it; fields the app does not use are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    vid: String,
    pid: String,
    name: String,
    family: String,
    kind: String,
    connection: String,
    needs_capture: bool,
}

/// The catalogue row GitHub `main` has for `id` as a custom entry, or `None` (not listed,
/// unusable, offline or any other failure, each logged). Blocks for the downloads.
#[cfg(windows)]
pub fn lookup(id: Id) -> Option<CustomEntry> {
    let _span = log::span("remote catalogue lookup");
    lookup_in(id, protocol::CATALOGUE_FILES, fetch)
}

/// `lookup` over `files`, each read with `fetch`, in order: the first usable row wins.
// ponytail: only the files this build knows are fetched, so a new brand on an existing family
// VID goes into an existing file to reach older apps; a new file reaches only newer builds.
fn lookup_in(
    id: Id,
    files: &[&str],
    mut fetch: impl FnMut(&str) -> Result<String, FetchError>,
) -> Option<CustomEntry> {
    if !family_vid(id.0, id.1) {
        log::trace(format_args!(
            "remote catalogue: {:04X}:{:04X} outside every family VID, no fetch",
            id.0, id.1
        ));
        return None;
    }
    for &file in files {
        let rows = match fetch(file) {
            Ok(text) => parse(&text),
            Err(FetchError::Offline(error)) => {
                log::line(&format!(
                    "remote catalogue: {file} unreachable, lookup ends: {error}"
                ));
                return None;
            }
            Err(FetchError::File(error)) => Err(error),
        };
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => {
                log::line(&format!("remote catalogue: {file} skipped: {error}"));
                continue;
            }
        };
        if let Some(entry) = pick(&rows, id, file) {
            log::line(&format!(
                "remote catalogue: {} found in {file} as {} {}",
                entry.id, entry.family, entry.kind
            ));
            return Some(entry);
        }
    }
    log::trace(format_args!(
        "remote catalogue: {:04X}:{:04X} not listed",
        id.0, id.1
    ));
    None
}

/// GETs one catalogue file from GitHub; only a 200 reply within the cap counts.
#[cfg(windows)]
fn fetch(file: &str) -> Result<String, FetchError> {
    let url = format!("{RAW_BASE}{file}");
    let reply = crate::update::get_within(&url, None, MAX_FILE_BYTES, TIMEOUT_MS)
        .map_err(FetchError::Offline)?;
    if reply.status != 200 {
        return Err(FetchError::File(format!("HTTP {}", reply.status)));
    }
    if reply.truncated {
        return Err(FetchError::File(format!(
            "larger than {} KiB",
            MAX_FILE_BYTES / 1024
        )));
    }
    String::from_utf8(reply.body).map_err(|_| FetchError::File("not UTF-8".to_owned()))
}

/// The rows of one catalogue file. A row with a missing or mistyped field is skipped (and
/// logged), so one odd row from a newer format never hides the others.
fn parse(text: &str) -> Result<Vec<Row>, String> {
    let table = text.parse::<Table>().map_err(|error| error.to_string())?;
    let items = table
        .get("devices")
        .and_then(Value::as_array)
        .ok_or("no devices array")?;
    let rows = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let parsed = row(item);
            if parsed.is_none() {
                log::line(&format!("remote catalogue: row {} unreadable", index + 1));
            }
            parsed
        })
        .collect();
    Ok(rows)
}

/// One row's fields; `needs_capture` defaults to false but must be a boolean when present.
fn row(item: &Value) -> Option<Row> {
    let item = item.as_table()?;
    let text = |key: &str| item.get(key)?.as_str().map(str::to_owned);
    let needs_capture = match item.get("needs_capture") {
        None => false,
        Some(value) => value.as_bool()?,
    };
    Some(Row {
        vid: text("vid")?,
        pid: text("pid")?,
        name: text("name")?,
        family: text("family")?,
        kind: text("kind")?,
        connection: text("connection")?,
        needs_capture,
    })
}

/// The first usable row naming `id`; refused rows for `id` are logged with why.
fn pick(rows: &[Row], id: Id, file: &str) -> Option<CustomEntry> {
    rows.iter()
        .filter(|row| row_id(row) == Some(id))
        .find_map(|row| {
            usable(row, id)
                .map_err(|why| log::line(&format!("remote catalogue: {file} row refused: {why}")))
                .ok()
        })
}

/// A row's `VID:PID` by the probe command's rule (4 hex digits each, any case).
fn row_id(row: &Row) -> Option<Id> {
    parse_vid_pid(&format!("{}:{}", row.vid, row.pid)).ok()
}

/// Bidi, zero-width and BOM characters that can disguise a name in the popup.
fn is_invisible_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// The custom entry a row for `(vid, pid)` saves, or why it is refused (capture needed,
/// odd name, id off every family VID, or a family custom devices cannot use).
fn usable(row: &Row, (vid, pid): Id) -> Result<CustomEntry, String> {
    if row.needs_capture {
        return Err("needs a capture".to_owned());
    }
    let name = row.name.trim();
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_control() || is_invisible_format(c))
    {
        return Err("empty name or control/format characters".to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("name longer than {MAX_NAME_CHARS} characters"));
    }
    if !family_vid(vid, pid) {
        return Err("id outside every family VID".to_owned());
    }
    let entry = CustomEntry {
        id: format!("{vid:04X}:{pid:04X}"),
        name: name.to_owned(),
        family: row.family.clone(),
        kind: row.kind.clone(),
        connection: row.connection.clone(),
    };
    custom_device(&entry)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Id = (0x3554, 0x7777);

    fn row(name: &str, family: &str) -> Row {
        Row {
            vid: "3554".into(),
            pid: "7777".into(),
            name: name.into(),
            family: family.into(),
            kind: "keyboard".into(),
            connection: "receiver".into(),
            needs_capture: false,
        }
    }

    #[test]
    fn parse_ignores_unknown_fields_and_skips_bad_rows() {
        let text = r#"
            # A newer file format may add top-level keys and row fields.
            schema = 2
            devices = [
              { vid = "3554", pid = "7777", name = "Board", family = "Report13", kind = "keyboard", connection = "receiver", verified = false, colour = "red" },
              { vid = "3554", pid = "7778", name = "No family", kind = "keyboard", connection = "receiver" },
              { vid = "3554", pid = "7779", name = "Odd flag", family = "Report13", kind = "keyboard", connection = "receiver", needs_capture = "yes" },
              "not a table",
              { vid = "3554", pid = "777A", name = "Quiet", family = "Report13", kind = "keyboard", connection = "cable", needs_capture = true },
            ]
        "#;
        let rows = parse(text).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], row("Board", "Report13"));
        assert!(rows[1].needs_capture && rows[1].connection == "cable");

        assert!(parse("devices = 3").is_err());
        assert!(parse("not toml [").is_err());
    }

    #[test]
    fn pick_saves_a_clean_entry_for_its_id_only() {
        let mut lower = row("  Board 8K  ", "Report13");
        lower.pid = "777a".into();
        let other = row("Other", "Report13");
        let entry = pick(&[other, lower], (0x3554, 0x777A), "t.toml");
        assert_eq!(
            entry,
            Some(CustomEntry {
                id: "3554:777A".into(),
                name: "Board 8K".into(),
                family: "Report13".into(),
                kind: "keyboard".into(),
                connection: "receiver".into(),
            })
        );
    }

    #[test]
    fn usable_refuses_unsafe_rows() {
        let needs_capture = Row {
            needs_capture: true,
            ..row("Board", "Report13")
        };
        assert!(usable(&needs_capture, ID).is_err());
        let longest = "x".repeat(MAX_NAME_CHARS);
        let too_long = format!("{longest}x");
        for name in [
            "",
            "   ",
            "Bad\nname",
            "Bell\u{7}",
            "Mouse\u{202E}evil",
            too_long.as_str(),
        ] {
            assert!(usable(&row(name, "Report13"), ID).is_err(), "{name:?}");
        }
        assert!(usable(&row(&longest, "Report13"), ID).is_ok());
        // Families only the built-in catalogue can use, and unknown ones.
        for family in [
            "AtkSerial",
            "RongYuanF7",
            "Driveall",
            "Xctech",
            "Generic",
            "Future",
        ] {
            assert!(usable(&row("Board", family), ID).is_err(), "{family}");
        }
        // An id off every family VID, and a recovery id on one.
        let mut logitech = row("Board", "Compx");
        logitech.vid = "046D".into();
        assert!(usable(&logitech, (0x046D, 0x7777)).is_err());
        let mut recovery = row("Board", "Compx");
        recovery.pid = "F2FF".into();
        assert!(usable(&recovery, (0x3554, 0xF2FF)).is_err());
    }

    #[test]
    fn lookup_skips_bad_files_stops_offline_and_takes_the_first_usable_row() {
        let good = r#"devices = [{ vid = "3554", pid = "7777", name = "Board", family = "Report13", kind = "keyboard", connection = "receiver" }]"#;
        let quiet = r#"devices = [{ vid = "3554", pid = "7777", name = "Quiet", family = "Report13", kind = "keyboard", connection = "receiver", needs_capture = true }]"#;
        let mut asked = Vec::new();
        let found = lookup_in(ID, &["a.toml", "b.toml", "c.toml", "d.toml"], |file| {
            asked.push(file.to_owned());
            match file {
                "a.toml" => Err(FetchError::File("HTTP 404".into())),
                "b.toml" => Ok(quiet.to_owned()),
                "c.toml" => Ok(good.to_owned()),
                _ => panic!("fetched past the match"),
            }
        });
        assert_eq!(found.map(|entry| entry.name), Some("Board".to_owned()));
        assert_eq!(asked, ["a.toml", "b.toml", "c.toml"]);

        // Offline: the first failure ends the lookup.
        let mut tries = 0;
        let offline = lookup_in(ID, &["a.toml", "b.toml"], |_| {
            tries += 1;
            Err(FetchError::Offline("no network".into()))
        });
        assert_eq!((offline, tries), (None, 1));

        // An id off every family VID never fetches.
        let mut fetched = 0;
        let outside = lookup_in((0x046D, 0x7777), &["c.toml"], |_| {
            fetched += 1;
            Ok(good.to_owned())
        });
        assert_eq!((outside, fetched), (None, 0));

        // Another id's row is no match.
        assert_eq!(
            lookup_in((0x3554, 0x1111), &["c.toml"], |_| Ok(good.to_owned())),
            None
        );
    }
}
