//! The receiver and cable devices the app knows: catalogue models first, then the owner's
//! custom devices from the config (`lookup`). Used by the refresh worker, probe, listen and scan.

use protocol::{Family, Kind, KnownDevice, known};

/// A catalogue model or a custom device, in one shape for every reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub vid: u16,
    pub pid: u16,
    pub name: String,
    pub family: Family,
    pub kind: Kind,
    /// Read on real hardware (catalogue only); queried unasked.
    pub verified: bool,
    pub cable: bool,
    pub needs_capture: bool,
    /// Added by the owner outside the catalogue: queried with `Door::send_as`.
    pub custom: bool,
}

impl Device {
    /// The query gate: a verified catalogue model, an `allow` entry or a custom device.
    pub fn is_queried(&self, allow: &[(u16, u16)]) -> bool {
        self.verified || self.custom || allow.contains(&(self.vid, self.pid))
    }

    /// A custom BITMOUSE device on a cable, which is never sent to: the frame addresses the
    /// receiver, and the door checks the connection of catalogue rows only.
    pub fn is_custom_bitmouse_cable(&self) -> bool {
        self.custom && self.family == Family::AtkBitmouse && self.cable
    }
}

impl From<&KnownDevice> for Device {
    fn from(device: &KnownDevice) -> Self {
        Self {
            vid: device.vid,
            pid: device.pid,
            name: device.name.to_owned(),
            family: device.family,
            kind: device.kind,
            verified: device.verified,
            cable: device.cable,
            needs_capture: device.needs_capture,
            custom: false,
        }
    }
}

/// The catalogue model of `vid:pid`, else the first custom device with that id.
pub fn lookup(vid: u16, pid: u16, custom: &[Device]) -> Option<Device> {
    known(vid, pid).map(Device::from).or_else(|| {
        custom
            .iter()
            .find(|device| (device.vid, device.pid) == (vid, pid))
            .cloned()
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A custom receiver of `family` on `vid:pid`.
    pub(crate) fn custom(vid: u16, pid: u16, family: Family) -> Device {
        Device {
            vid,
            pid,
            name: "My mouse".into(),
            family,
            kind: Kind::Mouse,
            verified: false,
            cable: false,
            needs_capture: false,
            custom: true,
        }
    }

    #[test]
    fn lookup_takes_the_catalogue_first_then_custom_devices() {
        let mine = custom(0x1234, 0x5678, Family::Compx);
        // A custom entry naming a catalogue id never replaces the catalogue row.
        let shadow = custom(0x3837, 0x6011, Family::Compx);
        let list = [shadow, mine.clone()];

        let mchose = lookup(0x3837, 0x6011, &list).unwrap();
        assert_eq!(mchose.name, "MCHOSE V9 Pro 2");
        assert!(mchose.verified && !mchose.custom);
        assert_eq!(mchose.family, Family::Mchose);

        assert_eq!(lookup(0x1234, 0x5678, &list), Some(mine));
        assert_eq!(lookup(0x046D, 0xC52B, &list), None);
        assert_eq!(lookup(0x1234, 0x5678, &[]), None);
    }
}
