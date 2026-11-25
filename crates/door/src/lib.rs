//! Device I/O for HID and serial: read-only enumeration (`present` and `present_all` open
//! nothing), the shape cache, the transports, and the one write door.
//! Only `write.rs` sends bytes to a device, through targets opened from an `Inventory`;
//! `listen.rs` only reads.

mod enumerate;
mod listen;
mod shapes;
mod transport;
mod write;

pub use enumerate::{
    Access, HidCollection, Inventory, Listing, Presence, SerialEntry, inventory, list, present,
    present_all,
};
pub use shapes::{STABLE_GAP, ShapeCache};
pub use transport::Target;
pub use write::{
    Connection, Door, Exchange, Fallback, MAX_KEPT_UNRELATED, Source, first_battery_query,
};

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
    fn open_hid_errors_never_show_the_path() {
        let path = r"\\?\hid#vid_0001&pid_0002&mi_01&col01#7&2a5b1c3d&0&0000#{4d1e55b2}";
        let opened = Access::Opened {
            descriptor: Vec::new(),
            parse_error: None,
        };
        let inventory = Inventory {
            hid: vec![
                collection(path, Access::Skipped("system mouse")),
                HidCollection {
                    shape: HidShape {
                        interface: None,
                        usage_page: 0xFF02,
                        usage: 0x0002,
                        reports: Vec::new(),
                    },
                    ..collection(path, opened)
                },
            ],
            ..empty_inventory()
        };
        let foreign = collection(&format!("{path}x"), Access::Skipped("system mouse"));
        for candidate in [&inventory.hid[0], &inventory.hid[1], &foreign] {
            let text = refusal(inventory.open_hid(candidate));
            assert!(!text.contains("7&2a5b1c3d"), "{text}");
            assert!(!text.contains("hid#"), "{text}");
        }
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
