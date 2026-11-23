//! Pure battery protocol: device families, query frames, reply decoders, the known-device table
//! and per-family query floors. No I/O; everything is checked from byte frames.

mod atk_serial;
pub mod compx;
mod devices;
mod mchose;

use std::time::{Duration, SystemTime};

pub use devices::{KNOWN, KnownDevice, known};

/// Size of one serial frame in both directions.
pub const SERIAL_FRAME_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Compx,
    Mchose,
    AtkSerial,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// A report or frame that answers some other command.
    NotTheReply,
    Reading(Reading),
    /// The reply to our query, but its content cannot be trusted.
    Invalid(&'static str),
}

impl Family {
    pub fn link(self) -> Link {
        match self {
            Family::Compx | Family::Mchose => Link::Hid,
            Family::AtkSerial => Link::Serial,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::Compx => "Compx report 8",
            Family::Mchose => "MCHOSE",
            Family::AtkSerial => "ATK serial",
        }
    }

    /// Minimum time between two queries to one device, independent of the UI refresh value.
    pub fn query_floor(self) -> Duration {
        match self {
            Family::Compx => Duration::from_secs(10),
            Family::Mchose | Family::AtkSerial => Duration::from_secs(300),
        }
    }
}

/// The battery query frame of a family; HID frames start with the report id.
pub fn battery_query(family: Family) -> &'static [u8] {
    match family {
        Family::Compx => &compx::QUERY,
        Family::Mchose => &mchose::QUERY,
        Family::AtkSerial => &atk_serial::QUERY,
    }
}

/// Decodes one HID input report as read (report id first) or one serial frame.
pub fn decode(family: Family, bytes: &[u8]) -> Decoded {
    match family {
        Family::Compx => compx::decode(bytes),
        Family::Mchose => mchose::decode(bytes),
        Family::AtkSerial => atk_serial::decode(bytes),
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
    fn query_floor_values() {
        assert_eq!(Family::Compx.query_floor(), Duration::from_secs(10));
        assert_eq!(Family::Mchose.query_floor(), Duration::from_secs(300));
        assert_eq!(Family::AtkSerial.query_floor(), Duration::from_secs(300));
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
        assert_eq!(Family::Mchose.link(), Link::Hid);
        assert_eq!(Family::AtkSerial.link(), Link::Serial);
        assert_eq!(Family::Compx.name(), "Compx report 8");
        assert_eq!(Family::Mchose.name(), "MCHOSE");
        assert_eq!(Family::AtkSerial.name(), "ATK serial");
    }
}
