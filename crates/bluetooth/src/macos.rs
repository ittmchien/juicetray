use std::time::SystemTime;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSNumber, NSString};
use objc2_io_bluetooth::IOBluetoothDevice;
use ui_model::{DeviceView, Level, Path};

use crate::{kind_from_class, normalize};

const NO_BATTERY_TEXT: &str = "no level";

pub fn read_devices() -> anyhow::Result<Vec<DeviceView>> {
    // SAFETY: class method without arguments; nil means nothing is paired.
    let Some(paired) = (unsafe { IOBluetoothDevice::pairedDevices() }) else {
        return Ok(Vec::new());
    };
    let now = SystemTime::now();
    Ok(paired
        .iter()
        .filter_map(|object| object.downcast::<IOBluetoothDevice>().ok())
        .filter_map(|device| device_view(&device, now))
        .collect())
}

/// One paired device as a view; `None` when it has no address to identify it by.
fn device_view(device: &IOBluetoothDevice, now: SystemTime) -> Option<DeviceView> {
    // SAFETY: plain getters on a live IOBluetoothDevice; nullable results are handled.
    // `name()` is avoided because it panics on nil; `nameOrAddress` is nullable.
    let (address, name, connected, major, minor) = unsafe {
        (
            device.addressString()?.to_string(),
            device.nameOrAddress(),
            device.isConnected(),
            device.deviceClassMajor(),
            device.deviceClassMinor(),
        )
    };
    let name = name.map_or_else(|| address.clone(), |name| name.to_string());

    // `updated_at` is the read time; `recentAccessDate` is not used because querying the
    // device refreshes it, so it never tells when a disconnected device was last seen.
    let updated_at = now;

    Some(DeviceView {
        id: format!("bt:{address}"),
        kind: kind_from_class(major, minor, &name),
        level: level(device),
        name,
        path: Path::Bluetooth,
        charging: false,
        connected,
        updated_at,
        stale: false,
        note: None,
    })
}

/// Single-cell batteries read as a percent; earbuds as left/right/case; otherwise no data.
fn level(device: &IOBluetoothDevice) -> Level {
    if let Some(percent) = battery(device, sel!(batteryPercentSingle)) {
        return Level::Percent(percent);
    }
    let left = battery(device, sel!(batteryPercentLeft));
    let right = battery(device, sel!(batteryPercentRight));
    let case = battery(device, sel!(batteryPercentCase));
    if left.is_none() && right.is_none() && case.is_none() {
        return Level::NoData(NO_BATTERY_TEXT.to_owned());
    }
    Level::Earbuds { left, right, case }
}

/// Reads one undocumented battery key; `respondsToSelector:` keeps an unknown key from throwing.
fn battery(device: &IOBluetoothDevice, key: Sel) -> Option<u8> {
    if !device.respondsToSelector(key) {
        return None;
    }
    let key = NSString::from_str(key.name().to_str().ok()?);
    // SAFETY: `valueForKey:` takes an NSString and may return nil; the key is answered above.
    let value: Option<Retained<AnyObject>> = unsafe { msg_send![device, valueForKey: &*key] };
    let number = value?.downcast::<NSNumber>().ok()?;
    normalize(number.as_i64())
}
