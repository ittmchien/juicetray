use std::collections::BTreeMap;
use std::mem::size_of;
use std::time::SystemTime;

use ::windows::Wdk::Devices::Bluetooth::DEVPKEY_Bluetooth_ClassOfDevice;
use ::windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA, SetupDiEnumDeviceInfo,
    SetupDiGetClassDevsW, SetupDiGetDevicePropertyW,
};
use ::windows::Win32::Devices::Properties::{
    DEVPKEY_Device_ContainerId, DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName,
    DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING,
    DEVPROP_TYPE_UINT32, DEVPROPTYPE,
};
use ::windows::Win32::Foundation::{DEVPROPKEY, ERROR_NO_MORE_ITEMS};
use ::windows::core::{GUID, Owned, PCWSTR};
use ui_model::{DeviceView, Level, Path};

use crate::{kind_from_class, normalize, split_cod};

/// `DEVPKEY_Bluetooth_Battery`: byte 0-100 that Windows reads over HFP or the GATT Battery Service.
const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
    pid: 2,
};

/// Boolean connection state of one device node.
const DEVPKEY_BLUETOOTH_IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x83DA6326_97A6_4088_9453_A1923F573B29),
    pid: 15,
};

/// Large enough for a friendly name; a longer property fails to read and is skipped.
const PROPERTY_BUFFER_BYTES: usize = 1024;

/// What the nodes of one container (one physical device) report, merged.
#[derive(Default)]
struct Container {
    name: Option<String>,
    // The main Bluetooth node carries the class of device and has the best name.
    name_from_main_node: bool,
    class_of_device: Option<u32>,
    battery: Option<u8>,
    connected: bool,
}

pub fn read_devices() -> anyhow::Result<Vec<DeviceView>> {
    // SAFETY: no class filter, enumerator or parent window; the list is freed by `Owned`.
    let set = unsafe {
        Owned::new(SetupDiGetClassDevsW(
            None,
            PCWSTR::null(),
            None,
            DIGCF_ALLCLASSES | DIGCF_PRESENT,
        )?)
    };

    // Keyed by container id: one physical device shows up as several device nodes.
    let mut containers: BTreeMap<u128, Container> = BTreeMap::new();
    let mut node = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    for index in 0.. {
        // SAFETY: `node` is a valid SP_DEVINFO_DATA with `cbSize` set.
        if let Err(error) = unsafe { SetupDiEnumDeviceInfo(*set, index, &mut node) } {
            if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() {
                break;
            }
            return Err(error.into());
        }
        merge_node(&mut containers, *set, &node);
    }

    let now = SystemTime::now();
    Ok(containers
        .into_iter()
        .filter_map(|(id, container)| view(id, container, now))
        .collect())
}

/// Folds one device node into its container, ignoring nodes that sit in no container.
fn merge_node(containers: &mut BTreeMap<u128, Container>, set: HDEVINFO, node: &SP_DEVINFO_DATA) {
    let Some(id) = property(set, node, &DEVPKEY_Device_ContainerId, DEVPROP_TYPE_GUID)
        .and_then(|bytes| Some(u128::from_le_bytes(bytes.get(..16)?.try_into().ok()?)))
        .filter(|&id| id != 0)
    else {
        return;
    };
    let container = containers.entry(id).or_default();

    let class_of_device = property(
        set,
        node,
        &DEVPKEY_Bluetooth_ClassOfDevice,
        DEVPROP_TYPE_UINT32,
    )
    .and_then(|bytes| Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?)));
    let is_main_node = class_of_device.is_some();
    container.class_of_device = container.class_of_device.or(class_of_device);

    // A main-node name replaces one taken from a service node; otherwise the first name stays.
    let name = string_property(set, node, &DEVPKEY_Device_FriendlyName)
        .or_else(|| string_property(set, node, &DEVPKEY_Device_DeviceDesc));
    if name.is_some()
        && (container.name.is_none() || (is_main_node && !container.name_from_main_node))
    {
        container.name = name;
        container.name_from_main_node = is_main_node;
    }

    let battery = property(set, node, &DEVPKEY_BLUETOOTH_BATTERY, DEVPROP_TYPE_BYTE)
        .and_then(|bytes| normalize(i64::from(*bytes.first()?)));
    container.battery = container.battery.or(battery);

    // A headset's battery node can report disconnected while audio flows, so any node counts.
    let connected = property(
        set,
        node,
        &DEVPKEY_BLUETOOTH_IS_CONNECTED,
        DEVPROP_TYPE_BOOLEAN,
    )
    .is_some_and(|bytes| bytes.first().is_some_and(|&flag| flag != 0));
    container.connected |= connected;
}

/// A device is listed only when the OS stores a battery value for its container.
fn view(id: u128, container: Container, now: SystemTime) -> Option<DeviceView> {
    let percent = container.battery?;
    let name = container
        .name
        .unwrap_or_else(|| "Bluetooth device".to_owned());
    let (major, minor) = container.class_of_device.map_or((0, 0), split_cod);
    Some(DeviceView {
        id: format!("bt:{id:032x}"),
        kind: kind_from_class(major, minor, &name),
        name,
        path: Path::Bluetooth,
        level: Level::Percent(percent),
        charging: false,
        connected: container.connected,
        updated_at: now,
        stale: false,
        note: None,
    })
}

/// Raw bytes of a device property, only when it exists with the expected type.
fn property(
    set: HDEVINFO,
    node: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
    expected: DEVPROPTYPE,
) -> Option<Vec<u8>> {
    let mut buffer = [0u8; PROPERTY_BUFFER_BYTES];
    let mut property_type = DEVPROPTYPE::default();
    let mut size = 0u32;
    // SAFETY: `node` belongs to `set`; the buffer and out-pointers outlive the call.
    unsafe {
        SetupDiGetDevicePropertyW(
            set,
            node,
            key,
            &mut property_type,
            Some(&mut buffer),
            Some(&mut size),
            0,
        )
    }
    .ok()?;
    if property_type != expected {
        return None;
    }
    buffer.get(..size as usize).map(<[u8]>::to_vec)
}

/// A UTF-16 string property, cut at the first NUL.
fn string_property(set: HDEVINFO, node: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<String> {
    let bytes = property(set, node, key, DEVPROP_TYPE_STRING)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units)).filter(|text| !text.is_empty())
}
