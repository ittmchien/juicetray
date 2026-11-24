//! The HID shapes read for each VID:PID, kept until the device leaves, so a device's
//! collections are opened for their descriptors once per plug-in, not once per refresh.

use std::collections::HashMap;
use std::time::Duration;

use protocol::HidShape;

use crate::enumerate::Presence;

/// Gap between the two `present_all` reads that must agree before shapes are cached.
pub const STABLE_GAP: Duration = Duration::from_millis(250);

#[derive(Debug, Default)]
pub struct ShapeCache {
    shapes: HashMap<(u16, u16), Vec<HidShape>>,
}

impl ShapeCache {
    pub fn get(&self, vid: u16, pid: u16) -> Option<&[HidShape]> {
        self.shapes.get(&(vid, pid)).map(Vec::as_slice)
    }

    /// Keeps `shapes` only when `before` and `after` (two `present_all` reads `STABLE_GAP`
    /// apart) list `vid:pid` with the same counts: a device still enumerating is read again
    /// later. `shapes` must be read after the `after` count, so they describe the stable
    /// device. Returns whether the shapes were kept.
    pub fn insert(
        &mut self,
        vid: u16,
        pid: u16,
        shapes: Vec<HidShape>,
        before: &[Presence],
        after: &[Presence],
    ) -> bool {
        let find = |all: &[Presence]| all.iter().find(|p| (p.vid, p.pid) == (vid, pid)).copied();
        let stable = find(before).is_some_and(|counts| find(after) == Some(counts));
        if stable {
            self.shapes.insert((vid, pid), shapes);
        }
        stable
    }

    /// Forgets every id `present` no longer lists, so a replugged device is read again.
    pub fn retain_present(&mut self, present: &[Presence]) {
        self.shapes
            .retain(|&(vid, pid), _| present.iter().any(|p| (p.vid, p.pid) == (vid, pid)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(vid: u16, pid: u16, hid: usize) -> Presence {
        Presence {
            vid,
            pid,
            hid,
            openable: hid,
            serial: 0,
        }
    }

    fn shapes() -> Vec<HidShape> {
        vec![HidShape {
            interface: Some(1),
            usage_page: 0xFF02,
            usage: 0x0002,
            reports: Vec::new(),
        }]
    }

    #[test]
    fn caches_only_when_two_reads_agree() {
        let mut cache = ShapeCache::default();
        let early = [presence(0x3554, 0xFA09, 1)];
        let settled = [presence(0x3554, 0xFA09, 3)];
        assert!(!cache.insert(0x3554, 0xFA09, shapes(), &early, &settled));
        assert_eq!(cache.get(0x3554, 0xFA09), None);
        assert!(cache.insert(0x3554, 0xFA09, shapes(), &settled, &settled));
        assert_eq!(cache.get(0x3554, 0xFA09), Some(shapes().as_slice()));
    }

    #[test]
    fn an_id_missing_from_either_read_is_not_cached() {
        let mut cache = ShapeCache::default();
        let here = [presence(0x3554, 0xFA09, 3)];
        assert!(!cache.insert(0x3554, 0xFA09, shapes(), &[], &here));
        assert!(!cache.insert(0x3554, 0xFA09, shapes(), &here, &[]));
        assert!(!cache.insert(0x373B, 0x1216, shapes(), &here, &here));
        assert_eq!(cache.get(0x373B, 0x1216), None);
    }

    #[test]
    fn removal_forgets_the_shapes() {
        let mut cache = ShapeCache::default();
        let both = [presence(0x3554, 0xFA09, 3), presence(0x373B, 0x1216, 4)];
        assert!(cache.insert(0x3554, 0xFA09, shapes(), &both, &both));
        assert!(cache.insert(0x373B, 0x1216, shapes(), &both, &both));
        cache.retain_present(&both[1..]);
        assert_eq!(cache.get(0x3554, 0xFA09), None);
        assert!(cache.get(0x373B, 0x1216).is_some());
    }
}
