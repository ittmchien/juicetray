//! Compiles `devices/*.toml` into `KNOWN` (`$OUT_DIR/known.rs`); any bad entry fails the
//! build, naming the file and entry.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs, process};

use serde::Deserialize;

#[path = "src/recovery.rs"]
mod recovery;

#[path = "src/generic_row.rs"]
mod generic_row;

/// The `Family` variants a catalogue line may name; keep in step with `protocol::Family`.
const FAMILIES: [&str; 10] = [
    "Compx",
    "Report13",
    "Mchose",
    "AtkSerial",
    "AtkBitmouse",
    "VgnKc2",
    "RongYuanF7",
    "Driveall",
    "Xctech",
    "Generic",
];

/// Catalogue `kind` values and their `protocol::Kind` variants.
const KINDS: [(&str, &str); 5] = [
    ("mouse", "Mouse"),
    ("headset", "Headset"),
    ("keyboard", "Keyboard"),
    ("gamepad", "Gamepad"),
    ("other", "Other"),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    devices: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    vid: String,
    pid: String,
    name: String,
    family: String,
    kind: String,
    connection: String,
    verified: bool,
    #[serde(default)]
    needs_capture: bool,
    // Generic rows only: the query and reply layout that replace family code.
    usage_page: Option<String>,
    usage: Option<String>,
    report: Option<String>,
    report_bytes: Option<u8>,
    request: Option<String>,
    reply_prefix: Option<String>,
    level_byte: Option<u8>,
    charging_byte: Option<u8>,
    charging_mask: Option<u8>,
    scale: Option<u8>,
}

impl Entry {
    fn generic_fields(&self) -> generic_row::Fields<'_> {
        generic_row::Fields {
            usage_page: self.usage_page.as_deref(),
            usage: self.usage.as_deref(),
            report: self.report.as_deref(),
            report_bytes: self.report_bytes,
            request: self.request.as_deref(),
            reply_prefix: self.reply_prefix.as_deref(),
            level_byte: self.level_byte,
            charging_byte: self.charging_byte,
            charging_mask: self.charging_mask,
            scale: self.scale,
        }
    }
}

fn main() {
    let dir = Path::new("devices");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/recovery.rs");
    println!("cargo:rerun-if-changed=src/generic_row.rs");
    println!("cargo:rerun-if-changed=devices");

    let mut errors = Vec::new();
    let mut rows = String::new();
    let mut seen: HashMap<(u16, u16), String> = HashMap::new();
    let files = catalogue_files(dir, &mut errors);
    // The app builds GitHub raw URLs from these names, so only plain lower-case names pass.
    for path in &files {
        if !is_plain_name(&file_name(path)) {
            errors.push(format!(
                "{}: file name must be lower-case letters, digits and dashes",
                path.display()
            ));
        }
    }
    for path in &files {
        let file = path.display().to_string();
        let parsed = fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|text| toml::from_str::<File>(&text).map_err(|error| error.to_string()));
        let entries = match parsed {
            Ok(parsed) => parsed.devices,
            Err(error) => {
                errors.push(format!("{file}: {error}"));
                continue;
            }
        };
        for (index, entry) in entries.iter().enumerate() {
            let at = format!("{file}: entry {} ({}:{})", index + 1, entry.vid, entry.pid);
            match row(entry) {
                Ok((id, text)) => {
                    // A repeat names the first occurrence, however many repeats follow.
                    match seen.get(&id) {
                        Some(first) => {
                            errors.push(format!("{at}: duplicate VID:PID, first in {first}"))
                        }
                        None => {
                            seen.insert(id, at.clone());
                        }
                    }
                    rows.push_str(&text);
                }
                Err(error) => errors.push(format!("{at}: {error}")),
            }
        }
    }

    if !errors.is_empty() {
        eprintln!("device catalogue (crates/protocol/devices) has errors:");
        for error in &errors {
            eprintln!("  {error}");
        }
        process::exit(1);
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let names: Vec<String> = files
        .iter()
        .map(|path| format!("{:?}", file_name(path)))
        .collect();
    let code = format!(
        "pub const KNOWN: &[KnownDevice] = &[\n{rows}];\n\
         /// The catalogue file names, sorted; the app fetches them from GitHub for unknown ids.\n\
         pub const CATALOGUE_FILES: &[&str] = &[{}];\n",
        names.join(", ")
    );
    fs::write(out.join("known.rs"), code).expect("write known.rs");
}

/// The `*.toml` files in `dir`, sorted by name so `KNOWN` keeps a stable order.
fn catalogue_files(dir: &Path, errors: &mut Vec<String>) -> Vec<PathBuf> {
    let listed = match fs::read_dir(dir) {
        Ok(listed) => listed,
        Err(error) => {
            errors.push(format!("{}: {error}", dir.display()));
            return Vec::new();
        }
    };
    let mut files: Vec<PathBuf> = listed
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    files
}

/// The file name of a catalogue path (lossy, so an odd name still reaches `is_plain_name`).
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `<lower-case letters, digits, dashes>.toml`.
fn is_plain_name(name: &str) -> bool {
    name.strip_suffix(".toml").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

/// One checked entry as its `VID:PID` and its `KnownDevice` literal.
fn row(entry: &Entry) -> Result<((u16, u16), String), String> {
    let vid = hex(&entry.vid, "vid")?;
    let pid = hex(&entry.pid, "pid")?;
    if recovery::is_recovery(vid, pid) {
        return Err("recovery/bootloader id; it must never be listed".to_owned());
    }
    if entry.name.trim().is_empty() {
        return Err("empty name".to_owned());
    }
    if !FAMILIES.contains(&entry.family.as_str()) {
        return Err(format!(
            "unknown family {:?}; one of {}",
            entry.family,
            FAMILIES.join(", ")
        ));
    }
    let kind = KINDS
        .iter()
        .find(|(name, _)| *name == entry.kind)
        .map(|(_, variant)| *variant)
        .ok_or_else(|| {
            let names: Vec<&str> = KINDS.iter().map(|(name, _)| *name).collect();
            format!("unknown kind {:?}; one of {}", entry.kind, names.join(", "))
        })?;
    let cable = match entry.connection.as_str() {
        "cable" => true,
        "receiver" => false,
        other => return Err(format!("unknown connection {other:?}; receiver or cable")),
    };
    // A Generic row's family carries its own spec; every other family is a plain variant.
    let family = match generic_row::check(entry.family == "Generic", &entry.generic_fields())? {
        Some(spec) => generic_row::literal(&spec),
        None => format!("Family::{}", entry.family),
    };
    let mut text = String::new();
    // `{:?}` of a str is a valid Rust string literal (quotes and backslashes escaped).
    let _ = writeln!(
        text,
        "    KnownDevice {{ vid: {vid:#06X}, pid: {pid:#06X}, name: {:?}, family: {family}, \
         kind: Kind::{kind}, verified: {}, cable: {cable}, needs_capture: {} }},",
        entry.name.trim(),
        entry.verified,
        entry.needs_capture
    );
    Ok(((vid, pid), text))
}

/// A `vid` / `pid` value: exactly 4 hex digits, any case, no prefix.
fn hex(value: &str, field: &str) -> Result<u16, String> {
    let is_hex = value.len() == 4 && value.chars().all(|c| c.is_ascii_hexdigit());
    is_hex
        .then(|| u16::from_str_radix(value, 16).ok())
        .flatten()
        .ok_or_else(|| format!("{field} {value:?} is not 4 hex digits"))
}
