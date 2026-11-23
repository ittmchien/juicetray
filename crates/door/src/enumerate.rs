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

/// Lists HID collections and USB serial ports, keeping only `filter`'s VID:PID when given.
/// Never fails as a whole; a source that cannot be listed is named in `errors`.
pub fn inventory(filter: Option<(u16, u16)>) -> Inventory {
    let mut errors = Vec::new();
    let (api, hid) = match HidApi::new() {
        Ok(api) => {
            let hid = hid_collections(&api, filter);
            (Some(api), hid)
        }
        Err(error) => {
            errors.push(format!("HID devices could not be listed: {error}"));
            (None, Vec::new())
        }
    };
    let serial = match serialport::available_ports() {
        Ok(ports) => serial_entries(ports, filter),
        Err(error) => {
            errors.push(format!("serial ports could not be listed: {error}"));
            Vec::new()
        }
    };
    Inventory {
        api,
        hid,
        serial,
        errors,
    }
}

fn wanted(filter: Option<(u16, u16)>, vid: u16, pid: u16) -> bool {
    filter.is_none_or(|ids| ids == (vid, pid))
}

/// Builds every collection's entry. Page and usage come from hidapi; the descriptor only
/// gives report sizes.
fn hid_collections(api: &HidApi, filter: Option<(u16, u16)>) -> Vec<HidCollection> {
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

/// Keeps USB ports only, then `filter`'s VID:PID.
fn serial_entries(ports: Vec<SerialPortInfo>, filter: Option<(u16, u16)>) -> Vec<SerialEntry> {
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
        .filter(|entry| wanted(filter, entry.vid, entry.pid))
        .collect();
    serial.sort_by(|a, b| (a.vid, a.pid, &a.port_name).cmp(&(b.vid, b.pid, &b.port_name)));
    serial
}
