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

/// What one device node reports besides its container id and battery.
struct NodeProps {
    class_of_device: Option<u32>,
    name: Option<String>,
    connected: bool,
}

impl Container {
    /// Folds one node into the container.
    fn merge(&mut self, node: NodeProps) {
        let is_main_node = node.class_of_device.is_some();
        self.class_of_device = self.class_of_device.or(node.class_of_device);

        // A main-node name replaces one taken from a service node; otherwise the first name stays.
        if node.name.is_some()
            && (self.name.is_none() || (is_main_node && !self.name_from_main_node))
        {
            self.name = node.name;
            self.name_from_main_node = is_main_node;
        }

        // A headset's battery node can report disconnected while audio flows, so any node counts.
        self.connected |= node.connected;
    }
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

    // First pass reads two cheap keys per node: the container id (one physical device shows up as
    // several nodes) and the battery. Only containers with a stored battery are ever listed.
    let mut nodes: Vec<(u128, SP_DEVINFO_DATA)> = Vec::new();
    let mut batteries: BTreeMap<u128, u8> = BTreeMap::new();
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
        // Nodes that sit in no container are ignored.
        let Some(id) =
            fixed_property::<16>(*set, &node, &DEVPKEY_Device_ContainerId, DEVPROP_TYPE_GUID)
                .map(u128::from_le_bytes)
                .filter(|&id| id != 0)
        else {
            continue;
        };
        if let Some(percent) =
            fixed_property::<1>(*set, &node, &DEVPKEY_BLUETOOTH_BATTERY, DEVPROP_TYPE_BYTE)
                .and_then(|[raw]| normalize(i64::from(raw)))
        {
            batteries.entry(id).or_insert(percent);
        }
        nodes.push((id, node));
    }

    // Second pass: the remaining keys, read only for nodes of listed containers.
    let mut containers: BTreeMap<u128, Container> = batteries
        .into_iter()
        .map(|(id, percent)| {
            let container = Container {
                battery: Some(percent),
                ..Default::default()
            };
            (id, container)
        })
        .collect();
    for (id, node) in &nodes {
        if let Some(container) = containers.get_mut(id) {
            container.merge(read_node_props(*set, node));
        }
    }

    let now = SystemTime::now();
    Ok(containers
        .into_iter()
        .filter_map(|(id, container)| view(id, container, now))
        .collect())
}

/// The class of device, name and connection flag of one node.
fn read_node_props(set: HDEVINFO, node: &SP_DEVINFO_DATA) -> NodeProps {
    NodeProps {
        class_of_device: fixed_property::<4>(
            set,
            node,
            &DEVPKEY_Bluetooth_ClassOfDevice,
            DEVPROP_TYPE_UINT32,
        )
        .map(u32::from_le_bytes),
        name: string_property(set, node, &DEVPKEY_Device_FriendlyName)
            .or_else(|| string_property(set, node, &DEVPKEY_Device_DeviceDesc)),
        connected: fixed_property::<1>(
            set,
            node,
            &DEVPKEY_BLUETOOTH_IS_CONNECTED,
            DEVPROP_TYPE_BOOLEAN,
        )
        .is_some_and(|[flag]| flag != 0),
    }
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

/// Reads a device property into `buffer`; the bytes written, only when it has the expected type.
fn read_property<'a>(
    set: HDEVINFO,
    node: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
    expected: DEVPROPTYPE,
    buffer: &'a mut [u8],
) -> Option<&'a [u8]> {
    let mut property_type = DEVPROPTYPE::default();
    let mut size = 0u32;
    // SAFETY: `node` belongs to `set`; the buffer and out-pointers outlive the call.
    unsafe {
        SetupDiGetDevicePropertyW(
            set,
            node,
            key,
            &mut property_type,
            Some(&mut *buffer),
            Some(&mut size),
            0,
        )
    }
    .ok()?;
    if property_type != expected {
        return None;
    }
    buffer.get(..size as usize)
}

/// A property of exactly `N` bytes, read without an allocation.
fn fixed_property<const N: usize>(
    set: HDEVINFO,
    node: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
    expected: DEVPROPTYPE,
) -> Option<[u8; N]> {
    let mut buffer = [0u8; N];
    read_property(set, node, key, expected, &mut buffer)?
        .try_into()
        .ok()
}

/// A UTF-16 string property, cut at the first NUL.
fn string_property(set: HDEVINFO, node: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<String> {
    let mut buffer = [0u8; PROPERTY_BUFFER_BYTES];
    let bytes = read_property(set, node, key, DEVPROP_TYPE_STRING, &mut buffer)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units)).filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(class_of_device: Option<u32>, name: Option<&str>, connected: bool) -> NodeProps {
        NodeProps {
            class_of_device,
            name: name.map(str::to_owned),
            connected,
        }
    }

    #[test]
    fn main_node_name_replaces_service_node_name() {
        let mut container = Container::default();
        container.merge(props(None, Some("Service"), false));
        container.merge(props(Some(0x2580), Some("Main"), false));
        container.merge(props(None, Some("Later service"), false));
        assert_eq!(container.name.as_deref(), Some("Main"));
        assert_eq!(container.class_of_device, Some(0x2580));
    }

    #[test]
    fn first_name_stays_without_a_main_node() {
        let mut container = Container::default();
        container.merge(props(None, Some("First"), false));
        container.merge(props(None, Some("Second"), false));
        assert_eq!(container.name.as_deref(), Some("First"));
    }

    #[test]
    fn any_connected_node_connects_the_container() {
        let mut container = Container::default();
        container.merge(props(None, None, false));
        container.merge(props(None, None, true));
        container.merge(props(None, None, false));
        assert!(container.connected);
    }
}
