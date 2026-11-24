//! Compiles the device catalogue (`devices/*.toml`, one file per brand) into `KNOWN`
//! (`$OUT_DIR/known.rs`, included by `src/devices.rs`). Every problem in the files fails the
//! build with the file and entry named, so a bad line never reaches a release.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs, process};

use serde::Deserialize;

#[path = "src/recovery.rs"]
mod recovery;

/// The `Family` variants a catalogue line may name; keep in step with `protocol::Family`.
const FAMILIES: [&str; 5] = ["Compx", "Report13", "Mchose", "AtkSerial", "AtkBitmouse"];

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
}

fn main() {
    let dir = Path::new("devices");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/recovery.rs");
    println!("cargo:rerun-if-changed=devices");

    let mut errors = Vec::new();
    let mut rows = String::new();
    let mut seen: HashMap<(u16, u16), String> = HashMap::new();
    for path in catalogue_files(dir, &mut errors) {
        let file = path.display().to_string();
        let parsed = fs::read_to_string(&path)
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
    let code = format!("pub const KNOWN: &[KnownDevice] = &[\n{rows}];\n");
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
    let mut text = String::new();
    // `{:?}` of a str is a valid Rust string literal (quotes and backslashes escaped).
    let _ = writeln!(
        text,
        "    KnownDevice {{ vid: {vid:#06X}, pid: {pid:#06X}, name: {:?}, family: Family::{}, \
         kind: Kind::{kind}, verified: {}, cable: {cable}, needs_capture: {} }},",
        entry.name.trim(),
        entry.family,
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
