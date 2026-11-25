//! macOS Bluetooth reads through a short-lived child process of this executable.

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use ui_model::{DeviceView, Level, Path};

/// The hidden subcommand the child runs; the parent passes it to its own executable.
pub const SUBCOMMAND: &str = "bluetooth-tsv";

/// Longest the parent waits for the child before killing it.
const CHILD_TIMEOUT: Duration = Duration::from_secs(5);

/// Child side: prints the paired devices, one tab-separated line each, then exits.
pub fn run_child() -> ExitCode {
    match bluetooth::read_devices() {
        Ok(devices) => {
            print!("{}", encode(&devices));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("bluetooth read failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Parent side: runs the child and parses its output.
// IOBluetooth battery values freeze in a long-running process after a reconnect; a fresh
// process reads them correctly.
pub fn read_devices() -> Result<Vec<DeviceView>> {
    let exe = std::env::current_exe().context("find own executable")?;
    let mut child = Command::new(exe)
        .arg(SUBCOMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .context("start the Bluetooth child")?;
    let mut stdout = child.stdout.take().context("child stdout")?;
    // A reader thread lets the wait time out; it ends when the child exits or is killed.
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut text = String::new();
        let _ = sender.send(stdout.read_to_string(&mut text).map(|_| text));
    });
    let Ok(read) = receiver.recv_timeout(CHILD_TIMEOUT) else {
        let _ = child.kill();
        let _ = child.wait();
        bail!("Bluetooth child timed out after {CHILD_TIMEOUT:?}");
    };
    let text = read.context("read the Bluetooth child output")?;
    let status = child.wait().context("wait for the Bluetooth child")?;
    if !status.success() {
        bail!("Bluetooth child failed: {status}");
    }
    decode(&text, SystemTime::now())
}

/// One line per device: id, name, kind, level, charging, connected (tab-separated).
fn encode(devices: &[DeviceView]) -> String {
    devices
        .iter()
        .map(|device| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                clean(&device.id),
                clean(&device.name),
                device.kind.as_str(),
                encode_level(&device.level),
                device.charging,
                device.connected,
            )
        })
        .collect()
}

/// Tabs and line breaks would break the line format.
fn clean(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

fn encode_level(level: &Level) -> String {
    match level {
        Level::Percent(percent) => format!("p:{percent}"),
        Level::Earbuds { left, right, case } => {
            let part = |value: &Option<u8>| value.map_or_else(|| "-".to_owned(), |v| v.to_string());
            format!("e:{},{},{}", part(left), part(right), part(case))
        }
        Level::NoData(text) => format!("n:{}", clean(text)),
    }
}

/// Parses `encode` output; a malformed line fails the whole read.
fn decode(text: &str, now: SystemTime) -> Result<Vec<DeviceView>> {
    text.lines().map(|line| decode_line(line, now)).collect()
}

fn decode_line(line: &str, now: SystemTime) -> Result<DeviceView> {
    let mut fields = line.split('\t');
    let mut next = |name: &str| {
        fields
            .next()
            .with_context(|| format!("missing {name} in {line:?}"))
    };
    let id = next("id")?.to_owned();
    let name = next("name")?.to_owned();
    let kind = next("kind")?
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown kind in {line:?}"))?;
    let level = decode_level(next("level")?)?;
    let charging = next("charging")?.parse()?;
    let connected = next("connected")?.parse()?;
    Ok(DeviceView {
        id,
        name,
        kind,
        path: Path::Bluetooth,
        level,
        charging,
        connected,
        updated_at: now,
        stale: false,
        note: None,
    })
}

fn decode_level(text: &str) -> Result<Level> {
    let (tag, value) = text.split_once(':').context("level without a tag")?;
    match tag {
        "p" => Ok(Level::Percent(value.parse()?)),
        "n" => Ok(Level::NoData(value.to_owned())),
        "e" => {
            let part = |value: &str| match value {
                "-" => Ok(None),
                number => number.parse().map(Some),
            };
            let mut parts = value.split(',');
            let mut next = || parts.next().context("earbuds level needs three parts");
            let (left, right, case) = (part(next()?)?, part(next()?)?, part(next()?)?);
            Ok(Level::Earbuds { left, right, case })
        }
        _ => bail!("unknown level tag {tag:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui_model::DeviceKind;

    fn view(name: &str, kind: DeviceKind, level: Level, connected: bool) -> DeviceView {
        DeviceView {
            id: "bt:AA-BB".into(),
            name: name.into(),
            kind,
            path: Path::Bluetooth,
            level,
            charging: false,
            connected,
            updated_at: SystemTime::UNIX_EPOCH,
            stale: false,
            note: None,
        }
    }

    #[test]
    fn devices_round_trip_through_the_line_format() {
        let devices = [
            view("FIIL Atom", DeviceKind::Headset, Level::Percent(100), true),
            view(
                "Buds",
                DeviceKind::Earbuds,
                Level::Earbuds {
                    left: Some(80),
                    right: None,
                    case: Some(50),
                },
                true,
            ),
            view(
                "Mouse",
                DeviceKind::Mouse,
                Level::NoData("no info".into()),
                false,
            ),
        ];
        let now = SystemTime::now();
        let back = decode(&encode(&devices), now).unwrap();
        let expected: Vec<DeviceView> = devices
            .into_iter()
            .map(|device| DeviceView {
                updated_at: now,
                ..device
            })
            .collect();
        assert_eq!(back, expected);
    }

    #[test]
    fn tabs_in_names_are_flattened_and_bad_lines_fail() {
        let devices = [view("A\tB", DeviceKind::Other, Level::Percent(1), true)];
        assert_eq!(
            decode(&encode(&devices), SystemTime::now()).unwrap()[0].name,
            "A B"
        );
        assert!(decode("only\ttwo\n", SystemTime::now()).is_err());
    }

    // Malformed levels fail the read instead of showing a wrong value; empty output means no devices.
    #[test]
    fn malformed_levels_fail_and_empty_output_is_no_devices() {
        assert!(decode_level("x:1").is_err());
        assert!(decode_level("e:1,2").is_err());
        assert!(decode_level("p:abc").is_err());
        assert!(decode("", SystemTime::now()).unwrap().is_empty());
    }
}
