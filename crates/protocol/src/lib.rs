//! Pure battery protocol: device families, query frames, reply decoders, the device catalogue
//! (compiled from `devices/*.toml`), the recovery deny list, family detection, per-family query
//! floors, HID report-descriptor sizes, the collection open rule and target picking. No I/O;
//! everything is checked from byte frames.

mod atk_serial;
mod bitmouse;
pub mod compx;
mod devices;
mod driveall;
mod families;
mod generic;
// Shared with build.rs, which checks catalogue rows with it; compiled here for its tests.
#[cfg(test)]
mod generic_row;
mod hid;
mod mchose;
mod recovery;
mod report;
mod report13;
mod rongyuan;
mod vgn_kc2;

use std::time::{Duration, SystemTime};

pub use devices::{KNOWN, Kind, KnownDevice, known};
pub use families::{families_for, family_vid};
pub use generic::{GenericReport, GenericSpec};
pub use hid::{HidShape, PickError, open_permission, pick_target};
pub use recovery::is_recovery;
pub use report::{ReportKind, ReportSize, TopCollection, parse_descriptor, reports_of};
pub use report13::ON_CABLE_TEXT;

/// Size of one serial frame in both directions.
pub const SERIAL_FRAME_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Compx,
    Report13,
    Mchose,
    AtkSerial,
    AtkBitmouse,
    VgnKc2,
    RongYuanF7,
    Driveall,
    /// A catalogue row's own query and reply layout (catalogue ids only, never custom devices).
    Generic(&'static GenericSpec),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Hid,
    Serial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub percent: u8,
    pub charging: bool,
    pub asleep: bool,
}

/// What a Compx online-status reply says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Online {
    Yes,
    No,
    /// The device reports the command as unsupported.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// A report or frame that answers some other command.
    NotTheReply,
    Reading(Reading),
    /// The reply to the online-status query (never produced by `decode`).
    Online(Online),
    /// The reply to our query, but its content cannot be trusted.
    Invalid(&'static str),
}

impl Family {
    pub fn link(self) -> Link {
        match self {
            Family::Compx
            | Family::Report13
            | Family::Mchose
            | Family::AtkBitmouse
            | Family::VgnKc2
            | Family::RongYuanF7
            | Family::Driveall
            | Family::Generic(_) => Link::Hid,
            Family::AtkSerial => Link::Serial,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::Compx => "Compx report 8",
            Family::Report13 => "VGN report 0x13",
            Family::Mchose => "MCHOSE",
            Family::AtkSerial => "ATK serial",
            Family::AtkBitmouse => "ATK BITMOUSE",
            Family::VgnKc2 => "VGN kc2",
            Family::RongYuanF7 => "RongYuan F7",
            Family::Driveall => "Driveall",
            Family::Generic(_) => "Generic",
        }
    }

    /// Minimum time between two queries to one device, independent of the UI refresh value.
    pub fn query_floor(self) -> Duration {
        match self {
            // RongYuan dongles answer F7 from their own cache without waking the keyboard.
            // Driveall's 10 s is unverified: whether GET_DEVICE_INFO wakes the keyboard is unknown.
            // Generic rows take the same 10 s; their frames come from user reports.
            Family::Compx
            | Family::Report13
            | Family::AtkBitmouse
            | Family::VgnKc2
            | Family::RongYuanF7
            | Family::Driveall
            | Family::Generic(_) => Duration::from_secs(10),
            Family::Mchose | Family::AtkSerial => Duration::from_secs(300),
        }
    }

    /// Total wait for the reply to one query; per-family values taken from the vendor apps.
    /// For a feature-report family it is the pause between SetFeature and GetFeature.
    pub fn reply_timeout(self) -> Duration {
        match self {
            Family::Compx => Duration::from_millis(3000),
            Family::Report13 => Duration::from_millis(800),
            // Driveall: the web driver's per-try timeout (500 ms), sent once instead of 3 retries.
            Family::AtkBitmouse | Family::Mchose | Family::VgnKc2 | Family::Driveall => {
                Duration::from_millis(500)
            }
            Family::AtkSerial => Duration::from_millis(1000),
            // Vendor tools read ~20 ms after SetFeature and retry 100 ms apart on an unfilled
            // reply; one read after 100 ms stands in for those retries.
            Family::RongYuanF7 => Duration::from_millis(100),
            // Generic: the output wait of VGN kc2 / Driveall, the feature pause of RongYuan F7.
            Family::Generic(spec) => match spec.report {
                GenericReport::Output => Duration::from_millis(500),
                GenericReport::Feature => Duration::from_millis(100),
            },
        }
    }

    /// Whether the query is a SetFeature + GetFeature pair instead of an output report
    /// answered by input reports.
    pub fn uses_feature_report(self) -> bool {
        match self {
            Family::RongYuanF7 => true,
            Family::Generic(spec) => spec.report == GenericReport::Feature,
            _ => false,
        }
    }
}

/// The battery query frame of a family; HID frames start with the report id.
pub fn battery_query(family: Family) -> &'static [u8] {
    match family {
        Family::Compx => &compx::QUERY,
        Family::Report13 => &report13::QUERY,
        Family::Mchose => &mchose::QUERY,
        Family::AtkSerial => &atk_serial::QUERY,
        Family::AtkBitmouse => &bitmouse::QUERY,
        Family::VgnKc2 => &vgn_kc2::QUERY,
        Family::RongYuanF7 => &rongyuan::QUERY,
        Family::Driveall => &driveall::QUERY,
        Family::Generic(spec) => spec.request,
    }
}

/// The only non-battery frame the door may send: the online status (a read-only getter in the
/// Compx vendor apps, ATK HUB's getStatus for ATK serial). Every other family has none.
pub fn online_query(family: Family) -> Option<&'static [u8]> {
    match family {
        Family::Compx => Some(&compx::ONLINE_QUERY),
        Family::AtkSerial => Some(&atk_serial::ONLINE_QUERY),
        Family::Report13
        | Family::Mchose
        | Family::AtkBitmouse
        | Family::VgnKc2
        | Family::RongYuanF7
        | Family::Driveall
        | Family::Generic(_) => None,
    }
}

/// Decodes the reply to `online_query`; a family without that query has no such reply.
pub fn decode_online(family: Family, bytes: &[u8]) -> Decoded {
    match family {
        Family::Compx => compx::decode_online(bytes),
        Family::AtkSerial => atk_serial::decode_online(bytes),
        Family::Report13
        | Family::Mchose
        | Family::AtkBitmouse
        | Family::VgnKc2
        | Family::RongYuanF7
        | Family::Driveall
        | Family::Generic(_) => Decoded::NotTheReply,
    }
}

/// Decodes one HID input report as read (report id first; hidapi drops an unused id 0), one
/// GetFeature reply or one serial frame.
pub fn decode(family: Family, bytes: &[u8]) -> Decoded {
    match family {
        Family::Compx => compx::decode(bytes),
        Family::Report13 => report13::decode(bytes),
        Family::Mchose => mchose::decode(bytes),
        Family::AtkSerial => atk_serial::decode(bytes),
        Family::AtkBitmouse => bitmouse::decode(bytes),
        Family::VgnKc2 => vgn_kc2::decode(bytes),
        Family::RongYuanF7 => rongyuan::decode(bytes),
        Family::Driveall => driveall::decode(bytes),
        Family::Generic(spec) => generic::decode(spec, bytes),
    }
}

/// Time still to wait before `family` may be queried again, or `None` when a query is allowed.
/// A `last` in the future (clock moved back) waits the full floor.
pub fn floor_wait(family: Family, last: Option<SystemTime>, now: SystemTime) -> Option<Duration> {
    let last = last?;
    let elapsed = now.duration_since(last).unwrap_or(Duration::ZERO);
    family
        .query_floor()
        .checked_sub(elapsed)
        .filter(|wait| !wait.is_zero())
}

/// Zero-pads `head` into a fixed-size frame at compile time.
const fn padded<const N: usize>(head: &[u8]) -> [u8; N] {
    let mut frame = [0; N];
    let mut i = 0;
    while i < head.len() {
        frame[i] = head[i];
        i += 1;
    }
    frame
}

/// Builds a reading. A raw 0 means "no data" (a zeroed or echoed frame decodes to it) and a
/// percent above 100 is out of range; neither is ever shown as a level.
fn reading(percent: u8, charging: bool, asleep: bool) -> Decoded {
    if percent == 0 {
        return Decoded::Invalid("percent 0: no data");
    }
    if percent > 100 {
        return Decoded::Invalid("percent out of range");
    }
    Decoded::Reading(Reading {
        percent,
        charging,
        asleep,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn reply_timeout_values() {
        let ms = |family: Family| family.reply_timeout().as_millis();
        assert_eq!(ms(Family::Compx), 3000);
        assert_eq!(ms(Family::Report13), 800);
        assert_eq!(ms(Family::AtkBitmouse), 500);
        assert_eq!(ms(Family::Mchose), 500);
        assert_eq!(ms(Family::AtkSerial), 1000);
        assert_eq!(ms(Family::VgnKc2), 500);
        assert_eq!(ms(Family::RongYuanF7), 100);
        assert_eq!(ms(Family::Driveall), 500);
        assert_eq!(ms(Family::Generic(&generic::tests::OUTPUT)), 500);
    }

    #[test]
    fn query_floor_values() {
        assert_eq!(Family::Compx.query_floor(), Duration::from_secs(10));
        assert_eq!(Family::Report13.query_floor(), Duration::from_secs(10));
        assert_eq!(Family::Mchose.query_floor(), Duration::from_secs(300));
        assert_eq!(Family::AtkSerial.query_floor(), Duration::from_secs(300));
        assert_eq!(Family::AtkBitmouse.query_floor(), Duration::from_secs(10));
        assert_eq!(Family::VgnKc2.query_floor(), Duration::from_secs(10));
        assert_eq!(Family::RongYuanF7.query_floor(), Duration::from_secs(10));
        assert_eq!(Family::Driveall.query_floor(), Duration::from_secs(10));
        let generic = Family::Generic(&generic::tests::OUTPUT);
        assert_eq!(generic.query_floor(), Duration::from_secs(10));
    }

    #[test]
    fn floor_wait_cases() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ago = |secs| Some(now - Duration::from_secs(secs));
        let wait = |family, last| floor_wait(family, last, now);

        assert_eq!(wait(Family::Compx, None), None);
        assert_eq!(wait(Family::Compx, ago(4)), Some(Duration::from_secs(6)));
        assert_eq!(wait(Family::Compx, ago(10)), None);
        assert_eq!(
            wait(Family::Compx, Some(now + Duration::from_secs(1))),
            Some(Duration::from_secs(10))
        );
        assert_eq!(wait(Family::Mchose, ago(299)), Some(Duration::from_secs(1)));
    }

    #[test]
    fn family_link_and_name() {
        assert_eq!(Family::Compx.link(), Link::Hid);
        assert_eq!(Family::Report13.link(), Link::Hid);
        assert_eq!(Family::Mchose.link(), Link::Hid);
        assert_eq!(Family::AtkSerial.link(), Link::Serial);
        assert_eq!(Family::AtkBitmouse.link(), Link::Hid);
        assert_eq!(Family::Compx.name(), "Compx report 8");
        assert_eq!(Family::Report13.name(), "VGN report 0x13");
        assert_eq!(Family::Mchose.name(), "MCHOSE");
        assert_eq!(Family::AtkSerial.name(), "ATK serial");
        assert_eq!(Family::AtkBitmouse.name(), "ATK BITMOUSE");
        assert_eq!(Family::VgnKc2.link(), Link::Hid);
        assert_eq!(Family::RongYuanF7.link(), Link::Hid);
        assert_eq!(Family::VgnKc2.name(), "VGN kc2");
        assert_eq!(Family::RongYuanF7.name(), "RongYuan F7");
        assert_eq!(Family::Driveall.link(), Link::Hid);
        assert_eq!(Family::Driveall.name(), "Driveall");
        let generic = Family::Generic(&generic::tests::OUTPUT);
        assert_eq!((generic.link(), generic.name()), (Link::Hid, "Generic"));
    }

    /// Every variant; the match makes a new variant fail to compile here until it is listed.
    fn all_families() -> [Family; 9] {
        let all = [
            Family::Compx,
            Family::Report13,
            Family::Mchose,
            Family::AtkSerial,
            Family::AtkBitmouse,
            Family::VgnKc2,
            Family::RongYuanF7,
            Family::Driveall,
            Family::Generic(&generic::tests::OUTPUT),
        ];
        for family in all {
            match family {
                Family::Compx
                | Family::Report13
                | Family::Mchose
                | Family::AtkSerial
                | Family::AtkBitmouse
                | Family::VgnKc2
                | Family::RongYuanF7
                | Family::Driveall
                | Family::Generic(_) => {}
            }
        }
        all
    }

    #[test]
    fn build_script_families_match_the_enum() {
        let build = include_str!("../build.rs");
        let start = build.find("const FAMILIES").expect("FAMILIES in build.rs");
        let list = &build[start..];
        let list = &list[..list.find("];").expect("end of FAMILIES")];
        let all = all_families();
        for family in all {
            // A Generic family's Debug text carries its spec; the catalogue names the variant.
            let variant = match family {
                Family::Generic(_) => "Generic".to_owned(),
                other => format!("{other:?}"),
            };
            assert!(list.contains(&format!("\"{variant}\"")), "{variant}");
        }
        assert_eq!(list.matches('"').count(), 2 * all.len(), "{list}");
    }

    /// The Generic row listed here is an output row; a feature row is checked in `generic`.
    #[test]
    fn only_rongyuan_uses_feature_reports() {
        for family in all_families() {
            assert_eq!(
                family.uses_feature_report(),
                family == Family::RongYuanF7,
                "{family:?}"
            );
        }
    }
}
