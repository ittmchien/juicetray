//! Read-only enumeration of HID collections and USB serial ports. Nothing here sends anything:
//! a handle is opened only to fetch its report descriptor, and only when the open rule allows.

use std::collections::HashMap;
use std::ffi::CStr;

use hidapi::{DeviceInfo, HidApi};
use protocol::{
    HidShape, ReportSize, TopCollection, open_permission, parse_descriptor, reports_of,
};
use serialport::{SerialPortInfo, SerialPortType};

/// One HID collection as hidapi lists it. The serial string itself is never kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidCollection {
    pub vid: u16,
    pub pid: u16,
    /// hidapi `BusType` Debug text.
    pub bus: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub has_serial: bool,
    pub path: String,
    pub shape: HidShape,
    pub access: Access,
}

/// What enumeration did with a collection's handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// Opened for its report descriptor only.
    Opened {
        descriptor: Vec<u8>,
        parse_error: Option<String>,
    },
    /// The open rule forbids opening it.
    Skipped(&'static str),
    Failed(String),
}

/// One USB serial port. `serial_number` is kept only so scan can mask it in the port name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialEntry {
    pub port_name: String,
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

/// Everything one enumeration saw; the only source of targets (`open_hid`, `open_serial`).
pub struct Inventory {
    pub(crate) api: Option<HidApi>,
    pub hid: Vec<HidCollection>,
    pub serial: Vec<SerialEntry>,
    /// Per-source failures (hidapi init, serial port listing).
    pub errors: Vec<String>,
}

/// A descriptor read once per path, with its parse result.
type PathRead = Result<(Vec<u8>, Result<Vec<TopCollection>, String>), String>;

/// One enumeration of the HID and serial device lists; nothing is opened. `present` and
/// `into_inventory` both read it, so a refresh lists the devices once.
pub struct Listing {
    api: Option<HidApi>,
    /// VID:PID of every HID collection hidapi listed.
    hid_ids: Vec<(u16, u16)>,
    /// Every USB serial port, unfiltered.
    serial: Vec<SerialEntry>,
    errors: Vec<String>,
}

/// Lists the HID collections and USB serial ports once. Never fails as a whole; a source that
/// cannot be listed is named in `errors`.
pub fn list() -> Listing {
    let mut errors = Vec::new();
    let (api, hid_ids) = match HidApi::new() {
        Ok(api) => {
            let ids = api
                .device_list()
                .map(|info| (info.vendor_id(), info.product_id()))
                .collect();
            (Some(api), ids)
        }
        Err(error) => {
            errors.push(format!("HID devices could not be listed: {error}"));
            (None, Vec::new())
        }
    };
    let serial = match serialport::available_ports() {
        Ok(ports) => serial_entries(ports),
        Err(error) => {
            errors.push(format!("serial ports could not be listed: {error}"));
            Vec::new()
        }
    };
    Listing {
        api,
        hid_ids,
        serial,
        errors,
    }
}

impl Listing {
    /// Per-source failures (hidapi init, serial port listing).
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The `ids` that are plugged in, in `ids` order, from the lists alone: nothing is opened,
    /// so a device that must stay untouched can be checked. A source that cannot be listed
    /// counts as having none of them.
    pub fn present(&self, ids: &[(u16, u16)]) -> Vec<(u16, u16)> {
        let seen: Vec<(u16, u16)> = self
            .hid_ids
            .iter()
            .copied()
            .chain(self.serial.iter().map(|entry| (entry.vid, entry.pid)))
            .collect();
        listed_ids(ids, &seen)
    }

    /// Keeps only the VID:PIDs in `filter` (empty = every device) and opens a handle for each
    /// kept HID collection the open rule allows, to fetch its report descriptor.
    pub fn into_inventory(self, filter: &[(u16, u16)]) -> Inventory {
        let hid = self
            .api
            .as_ref()
            .map(|api| hid_collections(api, filter))
            .unwrap_or_default();
        let mut serial = self.serial;
        serial.retain(|entry| wanted(filter, entry.vid, entry.pid));
        Inventory {
            api: self.api,
            hid,
            serial,
            errors: self.errors,
        }
    }
}

/// Lists HID collections and USB serial ports, keeping only the VID:PIDs in `filter` (empty =
/// every device).
pub fn inventory(filter: &[(u16, u16)]) -> Inventory {
    list().into_inventory(filter)
}

/// The `ids` that are plugged in, from one fresh listing; `list` shares a listing with
/// `into_inventory`.
pub fn present(ids: &[(u16, u16)]) -> Vec<(u16, u16)> {
    list().present(ids)
}

/// The `ids` that appear in `seen`, once each, in `ids` order.
fn listed_ids(ids: &[(u16, u16)], seen: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut found: Vec<(u16, u16)> = Vec::new();
    for id in ids {
        if seen.contains(id) && !found.contains(id) {
            found.push(*id);
        }
    }
    found
}

/// An empty filter keeps every device; the open rule still ran over the full list.
fn wanted(filter: &[(u16, u16)], vid: u16, pid: u16) -> bool {
    filter.is_empty() || filter.contains(&(vid, pid))
}

/// Builds every collection's entry. Page and usage come from hidapi; the descriptor only
/// gives report sizes.
fn hid_collections(api: &HidApi, filter: &[(u16, u16)]) -> Vec<HidCollection> {
    let infos: Vec<&DeviceInfo> = api.device_list().collect();
    let paths: Vec<String> = infos
        .iter()
        .map(|info| info.path().to_string_lossy().into_owned())
        .collect();
    // The rule runs over the full list, so a path shared with a mouse/keyboard is seen.
    let keys: Vec<(&str, u16, u16)> = infos
        .iter()
        .zip(&paths)
        .map(|(info, path)| (path.as_str(), info.usage_page(), info.usage()))
        .collect();
    let verdicts = open_permission(&keys);

    let mut reads: HashMap<&str, PathRead> = HashMap::new();
    let mut hid = Vec::new();
    for ((info, path), verdict) in infos.iter().zip(&paths).zip(verdicts) {
        if !wanted(filter, info.vendor_id(), info.product_id()) {
            continue;
        }
        let (usage_page, usage) = (info.usage_page(), info.usage());
        let (reports, access) = match verdict {
            Err(reason) => (Vec::new(), Access::Skipped(reason)),
            Ok(()) => {
                let read = reads
                    .entry(path.as_str())
                    .or_insert_with(|| read_descriptor(api, info.path()));
                access_of(read, usage_page, usage)
            }
        };
        hid.push(HidCollection {
            vid: info.vendor_id(),
            pid: info.product_id(),
            bus: format!("{:?}", info.bus_type()),
            manufacturer: info.manufacturer_string().map(str::to_owned),
            product: info.product_string().map(str::to_owned),
            has_serial: has_serial(info),
            path: path.clone(),
            shape: HidShape {
                interface: u8::try_from(info.interface_number()).ok(),
                usage_page,
                usage,
                reports,
            },
            access,
        });
    }
    hid.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    hid
}

fn sort_key(c: &HidCollection) -> (u16, u16, Option<u8>, u16, u16, &str) {
    let shape = &c.shape;
    (
        c.vid,
        c.pid,
        shape.interface,
        shape.usage_page,
        shape.usage,
        &c.path,
    )
}

/// Opens `path`, fetches its report descriptor and drops the handle at once.
fn read_descriptor(api: &HidApi, path: &CStr) -> PathRead {
    let device = api.open_path(path).map_err(|error| error.to_string())?;
    let mut buf = vec![0; hidapi::MAX_REPORT_DESCRIPTOR_SIZE];
    let len = device
        .get_report_descriptor(&mut buf)
        .map_err(|error| error.to_string())?;
    buf.truncate(len);
    let tops = parse_descriptor(&buf);
    Ok((buf, tops))
}

/// Report sizes of one collection from its path's descriptor, and how the open went.
fn access_of(read: &PathRead, usage_page: u16, usage: u16) -> (Vec<ReportSize>, Access) {
    match read {
        Err(error) => (Vec::new(), Access::Failed(error.clone())),
        Ok((descriptor, Ok(tops))) => (
            reports_of(tops, usage_page, usage),
            Access::Opened {
                descriptor: descriptor.clone(),
                parse_error: None,
            },
        ),
        Ok((descriptor, Err(error))) => (
            Vec::new(),
            Access::Opened {
                descriptor: descriptor.clone(),
                parse_error: Some(error.clone()),
            },
        ),
    }
}

/// Whether the device reports a serial number; an empty one counts as none.
fn has_serial(info: &DeviceInfo) -> bool {
    info.serial_number().is_some_and(|text| !text.is_empty())
        || info.serial_number_raw().is_some_and(|raw| !raw.is_empty())
}

/// Keeps USB ports only, sorted.
fn serial_entries(ports: Vec<SerialPortInfo>) -> Vec<SerialEntry> {
    let mut serial: Vec<SerialEntry> = ports
        .into_iter()
        .filter_map(|port| match port.port_type {
            SerialPortType::UsbPort(usb) => Some(SerialEntry {
                port_name: port.port_name,
                vid: usb.vid,
                pid: usb.pid,
                manufacturer: usb.manufacturer,
                product: usb.product,
                serial_number: usb.serial_number,
            }),
            _ => None,
        })
        .collect();
    serial.sort_by(|a, b| (a.vid, a.pid, &a.port_name).cmp(&(b.vid, b.pid, &b.port_name)));
    serial
}

#[cfg(test)]
mod tests {
    use super::{Listing, SerialEntry, listed_ids, wanted};

    fn port(name: &str, vid: u16, pid: u16) -> SerialEntry {
        SerialEntry {
            port_name: name.into(),
            vid,
            pid,
            manufacturer: None,
            product: None,
            serial_number: None,
        }
    }

    /// A listing with no hidapi handle: HID ids and serial ports as if enumerated.
    fn listing(hid_ids: &[(u16, u16)], serial: Vec<SerialEntry>) -> Listing {
        Listing {
            api: None,
            hid_ids: hid_ids.to_vec(),
            serial,
            errors: vec!["HID devices could not be listed: x".into()],
        }
    }

    #[test]
    fn one_listing_feeds_presence_and_inventory() {
        let ids = [(0x3837, 0x6011), (0x3554, 0xFA09), (0x373B, 0x1204)];
        let listed = listing(
            &[(0x373B, 0x1204), (0x046D, 0xC52B)],
            vec![port("COM4", 0x3837, 0x6011), port("COM5", 0x1A86, 0x7523)],
        );
        // Presence spans HID and serial, in `ids` order.
        assert_eq!(listed.present(&ids), [(0x3837, 0x6011), (0x373B, 0x1204)]);
        // The same listing then narrows to the openable ids and keeps its errors.
        let inventory = listed.into_inventory(&[(0x3837, 0x6011)]);
        assert_eq!(inventory.serial, [port("COM4", 0x3837, 0x6011)]);
        assert!(inventory.hid.is_empty());
        assert_eq!(inventory.errors.len(), 1);
    }

    #[test]
    fn inventory_with_an_empty_filter_keeps_every_port() {
        let listed = listing(&[], vec![port("COM4", 0x3837, 0x6011), port("COM5", 1, 2)]);
        assert_eq!(listed.into_inventory(&[]).serial.len(), 2);
    }

    #[test]
    fn listed_ids_keeps_ids_order_and_drops_duplicates() {
        let ids = [(0x3837, 0x6011), (0x3554, 0xFA09), (0x373B, 0x1204)];
        let seen = [
            (0x373B, 0x1204),
            (0x046D, 0xC52B),
            (0x3837, 0x6011),
            (0x373B, 0x1204),
        ];
        assert_eq!(
            listed_ids(&ids, &seen),
            [(0x3837, 0x6011), (0x373B, 0x1204)]
        );
        assert!(listed_ids(&ids, &[]).is_empty());
        assert!(listed_ids(&[], &seen).is_empty());
    }

    #[test]
    fn wanted_keeps_only_listed_ids() {
        let filter = [(0x3837, 0x6011), (0x3554, 0xFA09)];
        assert!(wanted(&filter, 0x3837, 0x6011));
        assert!(wanted(&filter, 0x3554, 0xFA09));
        assert!(!wanted(&filter, 0x3837, 0xFA09));
        assert!(!wanted(&filter, 0x046D, 0xC52B));
    }

    #[test]
    fn wanted_empty_filter_keeps_everything() {
        assert!(wanted(&[], 0x046D, 0xC52B));
    }
}
