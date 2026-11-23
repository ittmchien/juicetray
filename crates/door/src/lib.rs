//! Device I/O for HID and serial: read-only enumeration, the transports, and the one write door.
//! Only `write.rs` sends bytes to a device, through targets opened from an `Inventory`.

mod enumerate;
mod transport;
mod write;

pub use enumerate::{Access, HidCollection, Inventory, SerialEntry, inventory};
pub use transport::Target;
pub use write::{Door, Exchange, HID_REPLY_TIMEOUT, MAX_KEPT_UNRELATED, SERIAL_REPLY_TIMEOUT};

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::HidShape;

    fn collection(path: &str, access: Access) -> HidCollection {
        HidCollection {
            vid: 0x0001,
            pid: 0x0002,
            bus: "Usb".into(),
            manufacturer: None,
            product: None,
            has_serial: false,
            path: path.into(),
            shape: HidShape {
                interface: None,
                usage_page: 0x0001,
                usage: 0x0002,
                reports: Vec::new(),
            },
            access,
        }
    }

    fn empty_inventory() -> Inventory {
        Inventory {
            api: None,
            hid: Vec::new(),
            serial: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn refusal(result: anyhow::Result<Target>) -> String {
        match result {
            Ok(_) => panic!("the inventory must refuse"),
            Err(error) => format!("{error:#}"),
        }
    }

    #[test]
    fn open_hid_refuses_skipped() {
        let skipped = collection("P", Access::Skipped("system mouse"));
        let inventory = Inventory {
            hid: vec![skipped.clone()],
            ..empty_inventory()
        };

        let text = refusal(inventory.open_hid(&inventory.hid[0]));
        assert!(text.contains("not opened during enumeration"), "{text}");

        // The inventory's own entry decides, not the caller's copy.
        let edited = HidCollection {
            access: Access::Opened {
                descriptor: Vec::new(),
                parse_error: None,
            },
            ..skipped
        };
        let text = refusal(inventory.open_hid(&edited));
        assert!(text.contains("not opened during enumeration"), "{text}");

        let foreign = collection("Q", Access::Skipped("system mouse"));
        let text = refusal(inventory.open_hid(&foreign));
        assert!(text.contains("not in this inventory"), "{text}");
    }

    #[test]
    fn open_serial_refuses_foreign_entry() {
        let entry = SerialEntry {
            port_name: "COM9".into(),
            vid: 0x373B,
            pid: 0x1204,
            manufacturer: None,
            product: None,
            serial_number: None,
        };
        let text = refusal(empty_inventory().open_serial(&entry));
        assert!(text.contains("not in this inventory"), "{text}");
    }
}
