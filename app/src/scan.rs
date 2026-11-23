use std::collections::BTreeMap;
use std::time::SystemTime;

use door::{Access, HidCollection, SerialEntry};
use protocol::{ReportKind, ReportSize, known};
use ui_model::{DeviceView, EMPTY_MENU_TEXT, menu_line};

/// Plain-text device list for `juicetray scan`; device ids stay out of the output.
pub fn scan_report(devices: &[DeviceView], now: SystemTime) -> String {
    let mut lines = vec!["Bluetooth (OS-stored battery)".to_owned()];
    if devices.is_empty() {
        lines.push(EMPTY_MENU_TEXT.to_owned());
    } else {
        lines.extend(devices.iter().map(|device| menu_line(device, now)));
    }
    lines.join("\n")
}

/// The HID collections and serial ports sharing one VID:PID.
type Group<'a> = (Vec<&'a HidCollection>, Vec<&'a SerialEntry>);

/// Plain-text USB section: HID collections and serial ports grouped by VID:PID. Serial numbers
/// and device instance ids never appear.
pub fn usb_report(hid: &[HidCollection], serial: &[SerialEntry], errors: &[String]) -> String {
    let mut lines = vec!["USB devices (HID collections and serial ports)".to_owned()];
    if hid.is_empty() && serial.is_empty() {
        lines.push("No USB HID devices or serial ports found".to_owned());
    } else {
        lines.push("Reports: in/out/feat <report id>:<bytes incl. id byte>".to_owned());
        let mut groups: BTreeMap<(u16, u16), Group> = BTreeMap::new();
        for collection in hid {
            groups
                .entry((collection.vid, collection.pid))
                .or_default()
                .0
                .push(collection);
        }
        for entry in serial {
            groups
                .entry((entry.vid, entry.pid))
                .or_default()
                .1
                .push(entry);
        }
        for (&(vid, pid), (collections, ports)) in &groups {
            lines.extend(group_lines(vid, pid, collections, ports));
        }
    }
    lines.extend(errors.iter().map(|text| format!("error: {text}")));
    lines.join("\n")
}

/// Header, identity line and one entry per collection / port of one VID:PID.
fn group_lines(
    vid: u16,
    pid: u16,
    collections: &[&HidCollection],
    ports: &[&SerialEntry],
) -> Vec<String> {
    let label = known(vid, pid).map_or("unknown device".to_owned(), |device| {
        format!("{} · {}", device.name, device.family.name())
    });
    let mut lines = vec![format!("{vid:04X}:{pid:04X}  {label}")];

    let identity = identity_parts(
        collections.iter().find_map(|c| c.manufacturer.as_deref()),
        collections.iter().find_map(|c| c.product.as_deref()),
        collections.iter().any(|c| c.has_serial),
    );
    if !identity.is_empty() {
        lines.push(format!("  {}", identity.join(" · ")));
    }
    for collection in collections {
        lines.extend(hid_lines(collection));
    }
    lines.extend(ports.iter().map(|entry| serial_line(entry)));
    lines
}

/// Manufacturer, product and the serial-number notice; absent parts are left out.
fn identity_parts(
    manufacturer: Option<&str>,
    product: Option<&str>,
    has_serial: bool,
) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(text) = manufacturer {
        parts.push(format!("manufacturer \"{text}\""));
    }
    if let Some(text) = product {
        parts.push(format!("product \"{text}\""));
    }
    if has_serial {
        parts.push("serial number redacted".to_owned());
    }
    parts
}

fn serial_line(entry: &SerialEntry) -> String {
    let serial = entry.serial_number.as_deref();
    let mut parts = vec![format!("serial {}", mask_serial(&entry.port_name, serial))];
    parts.extend(identity_parts(
        entry.manufacturer.as_deref(),
        entry.product.as_deref(),
        serial.is_some_and(|text| !text.is_empty()),
    ));
    format!("  {}", parts.join(" · "))
}

/// The port name with a non-empty USB serial string inside it replaced by `<serial>`.
pub(crate) fn mask_serial(port_name: &str, serial: Option<&str>) -> String {
    match serial {
        Some(text) if !text.is_empty() => port_name.replace(text, "<serial>"),
        _ => port_name.to_owned(),
    }
}

/// One collection line, its descriptor (only when it was read) and its path.
fn hid_lines(collection: &HidCollection) -> Vec<String> {
    let shape = &collection.shape;
    let interface = shape.interface.map_or("?".to_owned(), |n| n.to_string());
    let usage = usage_name(shape.usage_page, shape.usage);
    let (status, descriptor) = match &collection.access {
        Access::Opened {
            descriptor,
            parse_error,
        } => (
            reports_text(&shape.reports, parse_error.as_deref()),
            Some(descriptor),
        ),
        Access::Skipped(reason) => (format!("not opened ({reason})"), None),
        Access::Failed(text) => (format!("open failed: {text}"), None),
    };

    let mut head = format!(
        "  HID if {interface} {:04X}:{:04X}",
        shape.usage_page, shape.usage
    );
    if !usage.is_empty() {
        head.push(' ');
        head.push_str(usage);
    }
    let mut lines = vec![format!("{head} · {} · {status}", collection.bus)];
    if let Some(bytes) = descriptor {
        lines.push(format!("    {}", bytes_line("descriptor", bytes)));
    }
    lines.push(format!("    path {}", redact_path(&collection.path)));
    lines
}

/// Report sizes of an opened collection, or the reason there are none.
fn reports_text(reports: &[ReportSize], parse_error: Option<&str>) -> String {
    if let Some(error) = parse_error {
        return format!("report sizes unknown ({error})");
    }
    if reports.is_empty() {
        return "no reports in descriptor".to_owned();
    }
    let sizes: Vec<String> = reports
        .iter()
        .map(|report| {
            let kind = match report.kind {
                ReportKind::Input => "in",
                ReportKind::Output => "out",
                ReportKind::Feature => "feat",
            };
            format!("{kind} {:02x}:{}", report.id, report.bytes)
        })
        .collect();
    sizes.join(" ")
}

/// `<label> (<n> B): <bytes>`: every byte, lower-case hex, never truncated.
pub(crate) fn bytes_line(label: &str, bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|byte| format!(" {byte:02x}")).collect();
    format!("{label} ({} B):{hex}", bytes.len())
}

/// Windows HID paths are `#`-separated; part 2 is the machine-local device instance id, and a
/// Bluetooth LE hardware id in part 1 carries the address as a `_<12 hex>` token.
pub(crate) fn redact_path(path: &str) -> String {
    let mut parts: Vec<String> = path.split('#').map(str::to_owned).collect();
    if parts.len() < 4 {
        return path.to_owned();
    }
    parts[1] = parts[1]
        .split('&')
        .map(mask_address)
        .collect::<Vec<_>>()
        .join("&");
    parts[2] = "…".to_owned();
    parts.join("#")
}

/// `…_<exactly 12 hex digits>` (one `&`-separated piece) becomes `…_…`.
fn mask_address(piece: &str) -> String {
    match piece.rsplit_once('_') {
        Some((head, tail)) if tail.len() == 12 && tail.bytes().all(|b| b.is_ascii_hexdigit()) => {
            format!("{head}_…")
        }
        _ => piece.to_owned(),
    }
}

/// Short name of a well-known usage; empty when there is nothing useful to say.
fn usage_name(page: u16, usage: u16) -> &'static str {
    match (page, usage) {
        (0x0001, 0x0002) => "mouse",
        (0x0001, 0x0006) => "keyboard",
        (0x0001, 0x0080) => "system control",
        (0x000C, 0x0001) => "consumer control",
        (0xFF00.., _) => "vendor",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use protocol::HidShape;
    use ui_model::{DeviceKind, Level, Path};

    use super::*;

    const WIN_PATH: &str = "\\\\?\\hid#vid_3554&pid_fa09&mi_01&col01#7&2a8b1c3d&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
    const WIN_PATH_MASKED: &str =
        "\\\\?\\hid#vid_3554&pid_fa09&mi_01&col01#…#{4d1e55b2-f16f-11cf-88cb-001111000030}";
    const COMPX_DESCRIPTOR: [u8; 29] = [
        0x06, 0x02, 0xff, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x08, 0x09, 0x01, 0x15, 0x00, 0x26, 0xff,
        0x00, 0x75, 0x08, 0x95, 0x10, 0x81, 0x02, 0x09, 0x01, 0x95, 0x10, 0x91, 0x02, 0xc0,
    ];

    fn hid(vid: u16, pid: u16, page: u16, usage: u16, access: Access) -> HidCollection {
        HidCollection {
            vid,
            pid,
            bus: "Usb".into(),
            manufacturer: Some("Acme".into()),
            product: Some("Gadget".into()),
            has_serial: false,
            path: WIN_PATH.into(),
            shape: HidShape {
                interface: Some(1),
                usage_page: page,
                usage,
                reports: Vec::new(),
            },
            access,
        }
    }

    fn compx_collection() -> HidCollection {
        let report = |kind| ReportSize {
            kind,
            id: 8,
            bytes: 17,
        };
        let mut collection = hid(
            0x3554,
            0xFA09,
            0xFF02,
            0x0002,
            Access::Opened {
                descriptor: COMPX_DESCRIPTOR.to_vec(),
                parse_error: None,
            },
        );
        collection.shape.reports = vec![report(ReportKind::Input), report(ReportKind::Output)];
        collection
    }

    fn port(name: &str, vid: u16, pid: u16, serial: Option<&str>) -> SerialEntry {
        SerialEntry {
            port_name: name.into(),
            vid,
            pid,
            manufacturer: Some("Acme".into()),
            product: Some("Gadget".into()),
            serial_number: serial.map(str::to_owned),
        }
    }

    fn device(id: &str, name: &str, level: Level, now: SystemTime) -> DeviceView {
        DeviceView {
            id: id.to_owned(),
            name: name.to_owned(),
            kind: DeviceKind::Headset,
            path: Path::Bluetooth,
            level,
            charging: false,
            connected: true,
            updated_at: now - Duration::from_secs(120),
            stale: false,
            note: None,
        }
    }

    #[test]
    fn scan_report_lists_devices_without_ids() {
        let now = SystemTime::now();
        let devices = [
            device("bt:AA-BB-CC-DD-EE-01", "FIIL Atom", Level::Percent(84), now),
            device(
                "bt:AA-BB-CC-DD-EE-02",
                "X",
                Level::NoData("no battery info".into()),
                now,
            ),
        ];

        let report = scan_report(&devices, now);

        assert!(report.starts_with("Bluetooth (OS-stored battery)"));
        for device in &devices {
            assert!(report.contains(&menu_line(device, now)));
            assert!(!report.contains(&device.id));
        }
        assert!(
            scan_report(&[], now)
                .lines()
                .any(|line| line == EMPTY_MENU_TEXT)
        );
    }

    #[test]
    fn usb_report_groups_per_device() {
        let mouse = hid(
            0x373B,
            0x1204,
            0x0001,
            0x0002,
            Access::Skipped("system mouse"),
        );
        let vendor = hid(
            0x373B,
            0x1204,
            0xFF00,
            0x0001,
            Access::Failed("denied".into()),
        );
        let hids = [mouse, vendor, compx_collection()];
        let ports = [port("COM3", 0x373B, 0x1204, None)];

        let report = usb_report(&hids, &ports, &[]);
        let lines: Vec<&str> = report.lines().collect();

        let header = lines
            .iter()
            .position(|line| *line == "373B:1204  ATK N9 Ultra (dongle) · ATK serial")
            .expect("one header for the N9");
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("373B:1204"))
                .count(),
            1
        );
        // Identity line, both HID collections, then the serial line, all under that header.
        let serial = lines
            .iter()
            .position(|line| line.starts_with("  serial COM3"))
            .expect("serial line");
        assert!(serial > header);
        assert!(lines[header + 1].starts_with("  manufacturer"));
        let between = &lines[header + 1..serial];
        assert!(
            between
                .iter()
                .any(|line| line.starts_with("  HID if 1 0001:0002 mouse"))
        );
        assert!(
            between
                .iter()
                .any(|line| line.starts_with("  HID if 1 FF00:0001 vendor"))
        );
        // The VGN device keeps its own header; the instance id is the only elision.
        assert!(lines.contains(&"3554:FA09  AULA F75 (2.4G receiver) · VGN report 0x13"));
        assert_eq!(report.matches('…').count(), 3);
    }

    #[test]
    fn usb_report_labels_known_devices() {
        let hids = [
            hid(0x3837, 0x6011, 0xFF07, 0x0222, Access::Skipped("x")),
            hid(0x046D, 0xC52B, 0xFF00, 0x0001, Access::Skipped("x")),
        ];

        let report = usb_report(&hids, &[], &[]);

        assert!(report.contains("3837:6011  MCHOSE V9 Pro 2 (dongle) · MCHOSE"));
        assert!(report.contains("046D:C52B  unknown device"));
    }

    #[test]
    fn usb_report_never_prints_serial_numbers() {
        let mut collection = hid(0x373B, 0x1204, 0xFF00, 0x0001, Access::Skipped("x"));
        collection.has_serial = true;
        let ports = [port(
            "/dev/cu.usbmodemABC123",
            0x373B,
            0x1204,
            Some("ABC123"),
        )];

        let report = usb_report(&[collection], &ports, &[]);

        assert!(!report.contains("ABC123"), "{report}");
        assert!(report.contains("/dev/cu.usbmodem<serial>"));
        assert!(report.contains("serial number redacted"));
    }

    #[test]
    fn redact_path_cases() {
        assert_eq!(redact_path(WIN_PATH), WIN_PATH_MASKED);
        assert_eq!(redact_path("DevSrvsID:4294969073"), "DevSrvsID:4294969073");
        assert_eq!(redact_path("COM3"), "COM3");
        assert_eq!(redact_path("a#b#c"), "a#b#c");
    }

    #[test]
    fn redact_path_masks_ble_address() {
        let ble = "\\\\?\\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&02046d_pid&b025_rev&0005_d1e5f6a7b8c9&col01#9&1f2e3d4c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let masked = redact_path(ble);

        assert_eq!(
            masked,
            "\\\\?\\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&02046d_pid&b025_rev&0005_…&col01#…#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        );
        assert!(!masked.contains("d1e5f6a7b8c9"), "{masked}");
        assert!(!masked.contains("1f2e3d4c"), "{masked}");
        assert!(masked.contains("vid&02046d_pid&b025"), "{masked}");
    }

    #[test]
    fn usb_report_shows_reports_and_descriptor() {
        let report = usb_report(&[compx_collection()], &[], &[]);

        assert!(report.contains("HID if 1 FF02:0002 vendor · Usb · in 08:17 out 08:17"));
        assert!(report.lines().any(|line| line.trim()
            == "descriptor (29 B): 06 02 ff 09 02 a1 01 85 08 09 01 15 00 26 ff 00 75 08 95 10 81 02 09 01 95 10 91 02 c0"));
        assert!(report.contains(&format!("path {WIN_PATH_MASKED}")));
        assert!(!report.contains("7&2a8b1c3d"));
    }

    #[test]
    fn usb_report_explains_unopened() {
        let mut mouse = hid(
            0x3554,
            0xFA09,
            0x0001,
            0x0002,
            Access::Skipped("system mouse"),
        );
        mouse.shape.interface = None;
        let failed = hid(
            0x3554,
            0xFA09,
            0xFF00,
            0x0001,
            Access::Failed("denied".into()),
        );
        let bad = hid(
            0x3554,
            0xFA09,
            0xFF01,
            0x0001,
            Access::Opened {
                descriptor: vec![0xfe],
                parse_error: Some("truncated long item".into()),
            },
        );
        let empty = hid(
            0x3554,
            0xFA09,
            0xFF03,
            0x0001,
            Access::Opened {
                descriptor: Vec::new(),
                parse_error: None,
            },
        );

        let report = usb_report(&[mouse, failed, bad, empty], &[], &[]);

        assert!(report.contains("HID if ? 0001:0002 mouse · Usb · not opened (system mouse)"));
        assert!(report.contains("HID if 1 FF00:0001 vendor · Usb · open failed: denied"));
        assert!(report.contains("report sizes unknown (truncated long item)"));
        assert!(report.contains("descriptor (1 B): fe"));
        assert!(report.contains("no reports in descriptor"));
        assert!(report.contains("descriptor (0 B):\n"));
    }

    #[test]
    fn usb_report_empty() {
        let report = usb_report(&[], &[], &["serial ports could not be listed: boom".into()]);

        assert!(report.contains("No USB HID devices or serial ports found"));
        assert!(
            report
                .lines()
                .any(|line| line == "error: serial ports could not be listed: boom")
        );
    }

    #[test]
    fn usage_name_cases() {
        assert_eq!(usage_name(0x01, 0x02), "mouse");
        assert_eq!(usage_name(0x01, 0x06), "keyboard");
        assert_eq!(usage_name(0x01, 0x80), "system control");
        assert_eq!(usage_name(0x0C, 0x01), "consumer control");
        assert_eq!(usage_name(0xFF00, 0x01), "vendor");
        assert_eq!(usage_name(0xFF07, 0x0222), "vendor");
        assert_eq!(usage_name(0x01, 0x04), "");
    }
}
