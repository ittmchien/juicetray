//! Compx recovery and bootloader ids (the ATK HUB `CompxRecoveryFilter` group): never listed in
//! the catalogue, never queried, opened or offered. build.rs includes this file too.

/// Whether `vid:pid` is a Compx recovery or bootloader id: 3554:F2FF and 3554:F400-F410.
pub fn is_recovery(vid: u16, pid: u16) -> bool {
    vid == 0x3554 && (pid == 0xF2FF || (0xF400..=0xF410).contains(&pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_ids() {
        for pid in [0xF2FF, 0xF400, 0xF407, 0xF410] {
            assert!(is_recovery(0x3554, pid), "{pid:04X}");
        }
        assert!(!is_recovery(0x3554, 0xF3FF));
        assert!(!is_recovery(0x3554, 0xF411));
        assert!(!is_recovery(0x3554, 0xFA09));
        assert!(!is_recovery(0x373B, 0xF400));
    }
}
