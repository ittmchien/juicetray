//! New devices: plug-in notifications on the UI thread's heartbeat window, a trailing debounce
//! per VID:PID, and what a settled id earns: a refresh for a device the worker queries, or a
//! popup offer for an unverified catalogue model or an unknown id of a family VID. Only Windows
//! listens; the rules are pure. Interface paths are parsed for their ids and never logged.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use door::Presence;
use protocol::{Family, family_vid, is_recovery, known};

#[cfg(windows)]
use crate::config::Config;
use crate::device::{Device, lookup};

/// Quiet time after an id's last arrival before it is acted on; one plug-in announces every
/// interface and collection separately.
pub const SETTLE: Duration = Duration::from_secs(2);
/// An id refreshes at most once per this gap, however often it re-enumerates.
pub const REFRESH_GAP: Duration = Duration::from_secs(10);

/// A USB `(VID, PID)`.
pub type Id = (u16, u16);

/// The VID:PID in a device interface path such as `\\?\HID#VID_373B&PID_1216&MI_01#…`, in any
/// case; `None` for paths without both (Bluetooth paths spell them `VID&…`, so they never match).
pub fn parse_interface_name(name: &str) -> Option<Id> {
    let upper = name.to_ascii_uppercase();
    let hex_after = |tag: &str| {
        let start = upper.find(tag)? + tag.len();
        let digits = upper.get(start..start + 4)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
            .then(|| u16::from_str_radix(digits, 16).ok())
            .flatten()
    };
    Some((hex_after("VID_")?, hex_after("PID_")?))
}

/// Whether a device interface path belongs to a Bluetooth device (classic, LE or a HID over
/// either), which spells its ids `VID&…` or sits under a Bluetooth enumerator.
pub fn is_bluetooth_interface(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["BTHENUM", "BTHLE", "_VID&", "{00001124-", "{00001812-"]
        .iter()
        .any(|tag| upper.contains(tag))
}

/// Arrivals waiting out `SETTLE`, and when each id last earned a refresh.
#[derive(Debug, Default)]
pub struct Arrivals {
    last_arrival: HashMap<Id, Instant>,
    last_refresh: HashMap<Id, Instant>,
}

impl Arrivals {
    pub fn arrived(&mut self, id: Id, now: Instant) {
        self.last_arrival.insert(id, now);
    }

    /// When the earliest waiting id settles; `None` when nothing waits.
    pub fn next_due(&self) -> Option<Instant> {
        self.last_arrival.values().map(|at| *at + SETTLE).min()
    }

    /// When the latest waiting id settles, so every waiting id is due; `None` when nothing waits.
    pub fn quiet_at(&self) -> Option<Instant> {
        self.last_arrival.values().map(|at| *at + SETTLE).max()
    }

    /// Forgets a waiting id, for a device that left before it settled.
    pub fn forget(&mut self, id: Id) {
        self.last_arrival.remove(&id);
    }

    /// The ids quiet for `SETTLE` at `now`, sorted; they stop waiting.
    pub fn take_due(&mut self, now: Instant) -> Vec<Id> {
        let mut due: Vec<Id> = self
            .last_arrival
            .iter()
            .filter(|(_, at)| now >= **at + SETTLE)
            .map(|(id, _)| *id)
            .collect();
        due.sort_unstable();
        self.last_arrival.retain(|id, _| !due.contains(id));
        due
    }

    /// Whether `id` may refresh at `now`; a yes stamps it, so the next one waits `REFRESH_GAP`.
    pub fn may_refresh(&mut self, id: Id, now: Instant) -> bool {
        let recent = self
            .last_refresh
            .get(&id)
            .is_some_and(|at| now.saturating_duration_since(*at) < REFRESH_GAP);
        if !recent {
            self.last_refresh.insert(id, now);
        }
        !recent
    }
}

/// What a plugged-in id earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// The worker queries it (verified, `allow` or custom): read now.
    Refresh,
    /// A popup candidate: offered once the device lists show something to query or a receiver.
    Offer,
    Ignore,
}

/// The config values a plan reads.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub allow: Vec<Id>,
    pub custom: Vec<Device>,
    /// Ids already asked about in the popup (the `declined` config key): never offered again.
    pub declined: Vec<Id>,
    /// `detect_new_devices`: off, nothing is offered; queried devices still refresh.
    pub detect_new: bool,
}

impl Settings {
    // Only the Windows plug-in path builds settings from the config.
    #[cfg(windows)]
    pub fn from_config(config: &Config) -> Self {
        Self {
            allow: config.allowed(),
            custom: config.custom(),
            declined: config.declined_ids(),
            detect_new: config.detect_new_devices,
        }
    }
}

/// The plan for `id` under `settings`. Recovery ids are never touched; an id already asked about
/// is never offered again, and nothing is offered while detection is off.
pub fn plan(id: Id, settings: &Settings) -> Plan {
    let (vid, pid) = id;
    if is_recovery(vid, pid) {
        return Plan::Ignore;
    }
    if lookup(vid, pid, &settings.custom).is_some_and(|device| device.is_queried(&settings.allow)) {
        return Plan::Refresh;
    }
    if !settings.detect_new || settings.declined.contains(&id) {
        return Plan::Ignore;
    }
    // Any other id is a candidate; `offers` keeps those a family may read and wireless receivers.
    Plan::Offer
}

/// Whether a family may read `id`: a catalogue model or a model on a family's VID.
fn may_read((vid, pid): Id) -> bool {
    known(vid, pid).is_some() || family_vid(vid, pid)
}

/// Whether a USB product string names a wireless receiver; wired mice, keyboards, webcams and
/// other USB gear rarely use these words.
fn is_receiver(product: Option<&str>) -> bool {
    product.is_some_and(|text| {
        let text = text.to_ascii_lowercase();
        ["receiver", "dongle", "wireless", "2.4g"]
            .iter()
            .any(|word| text.contains(word))
    })
}

/// Whether the lists show something a family could query: a collection the open rule allows
/// (vendor pages, an Xctech model's 0001:0000), or the serial port of a catalogue ATK serial model.
pub fn has_query_channel(presence: &Presence) -> bool {
    let atk_serial =
        known(presence.vid, presence.pid).is_some_and(|device| device.family == Family::AtkSerial);
    presence.openable > 0 || (presence.serial > 0 && atk_serial)
}

/// One device the popup asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub id: Id,
    /// The catalogue name, else the USB product string, else `VID:PID`.
    pub name: String,
}

/// The offers among `present`: ids in `only` (the plugged-in ones; present devices are never
/// offered on their own) whose plan is `Offer` and that have a query channel. `product` gives an
/// id's USB product string.
pub fn offers<'a>(
    present: &[Presence],
    only: &[Id],
    settings: &Settings,
    product: impl Fn(Id) -> Option<&'a str>,
) -> Vec<Offer> {
    present
        .iter()
        .filter(|presence| {
            let id = (presence.vid, presence.pid);
            // A readable model needs a channel; any wireless receiver is offered so the owner
            // can try it or report it from the Add device window.
            only.contains(&id)
                && plan(id, settings) == Plan::Offer
                && ((may_read(id) && has_query_channel(presence)) || is_receiver(product(id)))
        })
        .map(|presence| {
            let id = (presence.vid, presence.pid);
            Offer {
                id,
                name: offer_name(id, product(id)),
            }
        })
        .collect()
}

/// The catalogue name, else a non-blank product string, else `VID:PID`.
fn offer_name((vid, pid): Id, product: Option<&str>) -> String {
    known(vid, pid)
        .map(|device| device.name.to_owned())
        .or_else(|| {
            product
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("{vid:04X}:{pid:04X}"))
}

#[cfg(windows)]
pub use win::{Registration, find_offers, interface_name, listen, on_window_message, register};

#[cfg(windows)]
mod win {
    use std::mem::offset_of;
    use std::sync::OnceLock;

    use tao::event_loop::EventLoopProxy;
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DBT_DEVICEARRIVAL, DBT_DEVTYP_DEVICEINTERFACE, DEV_BROADCAST_DEVICEINTERFACE_W,
        DEV_BROADCAST_HDR, DEVICE_NOTIFY_ALL_INTERFACE_CLASSES, DEVICE_NOTIFY_WINDOW_HANDLE,
        HDEVNOTIFY, PBT_APMRESUMEAUTOMATIC, RegisterDeviceNotificationW,
        UnregisterDeviceNotification, WM_DEVICECHANGE, WM_POWERBROADCAST,
    };

    use super::{Id, Offer, Settings, offers, parse_interface_name};
    use crate::app::UserEvent;
    use crate::config::Config;
    use crate::log;

    /// Where the heartbeat window's procedure sends plug-in and wake events.
    static PROXY: OnceLock<EventLoopProxy<UserEvent>> = OnceLock::new();

    /// The device-notification registration; dropping it unregisters.
    pub struct Registration(HDEVNOTIFY);

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: the handle came from RegisterDeviceNotificationW and is released once.
            if let Err(error) = unsafe { UnregisterDeviceNotification(self.0) } {
                log::line(&format!(
                    "plug-in notifications: unregister failed: {error}"
                ));
            }
        }
    }

    /// Routes `window`'s plug-in and wake messages to the event loop and registers it for every
    /// device interface arrival; `None` (logged) when the registration fails, and wake still works.
    pub fn listen(window: HWND, proxy: EventLoopProxy<UserEvent>) -> Option<Registration> {
        // A second call keeps the first proxy; both reach the same event loop.
        let _ = PROXY.set(proxy);
        register(window)
    }

    /// Registers `window` (a top-level window of this thread) for every device interface
    /// arrival and removal; `None` (logged) when the registration fails.
    pub fn register(window: HWND) -> Option<Registration> {
        let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
            ..Default::default()
        };
        // SAFETY: `window` is a live top-level window of this thread; the filter outlives the call.
        let registered = unsafe {
            RegisterDeviceNotificationW(
                HANDLE(window.0),
                (&raw const filter).cast(),
                DEVICE_NOTIFY_WINDOW_HANDLE | DEVICE_NOTIFY_ALL_INTERFACE_CLASSES,
            )
        };
        match registered {
            Ok(handle) => Some(Registration(handle)),
            Err(error) => {
                log::line(&format!("plug-in notifications unavailable: {error}"));
                None
            }
        }
    }

    /// Called by the heartbeat window procedure for every message: an interface arrival becomes
    /// `UserEvent::Plugged`, a resume from sleep `UserEvent::Woke`. Never blocks.
    pub fn on_window_message(message: u32, wparam: WPARAM, lparam: LPARAM) {
        let event = match message {
            WM_DEVICECHANGE if wparam.0 == DBT_DEVICEARRIVAL as usize => {
                // SAFETY: for DBT_DEVICEARRIVAL `lParam` is null or a DEV_BROADCAST_HDR valid
                // during the call.
                let Some(id) = (unsafe { interface_name(lparam) })
                    .as_deref()
                    .and_then(parse_interface_name)
                else {
                    return;
                };
                UserEvent::Plugged(id)
            }
            WM_POWERBROADCAST if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize => UserEvent::Woke,
            _ => return,
        };
        if let Some(proxy) = PROXY.get() {
            // A closed event loop means the app is exiting.
            let _ = proxy.send_event(event);
        }
    }

    /// The interface path of a device-interface arrival or removal; callers parse it and drop
    /// it, never log it (R10).
    ///
    /// # Safety
    /// `lparam` is null or points at a DEV_BROADCAST_HDR of `dbch_size` bytes.
    pub unsafe fn interface_name(lparam: LPARAM) -> Option<String> {
        let header = lparam.0 as *const DEV_BROADCAST_HDR;
        if header.is_null() {
            return None;
        }
        // SAFETY: non-null and valid per the caller.
        let header = unsafe { &*header };
        if header.dbch_devicetype != DBT_DEVTYP_DEVICEINTERFACE {
            return None;
        }
        let name_offset = offset_of!(DEV_BROADCAST_DEVICEINTERFACE_W, dbcc_name);
        let units = (header.dbch_size as usize).checked_sub(name_offset)? / 2;
        // SAFETY: a device-interface broadcast holds `dbch_size` bytes, the name filling the
        // rest; the name's alignment is that of the struct.
        let name = unsafe {
            std::slice::from_raw_parts(
                (header as *const DEV_BROADCAST_HDR)
                    .cast::<u8>()
                    .add(name_offset)
                    .cast::<u16>(),
                units,
            )
        };
        let length = name.iter().position(|&unit| unit == 0).unwrap_or(units);
        Some(String::from_utf16_lossy(&name[..length]))
    }

    /// Runs on the refresh worker: the offers among the present devices that are in `only` (the
    /// plugged-in ids), from one listing that opens nothing; none, unlisted, while detection is
    /// off.
    pub fn find_offers(only: &[Id], config: &Config) -> Vec<Offer> {
        if !config.detect_new_devices {
            return Vec::new();
        }
        let _span = log::span("find offers");
        let listing = door::list();
        offers(
            &listing.present_all(),
            only,
            &Settings::from_config(config),
            |(vid, pid)| listing.product(vid, pid),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::custom;

    #[test]
    fn interface_names_give_their_ids_in_any_case() {
        let cases = [
            (
                r"\\?\HID#VID_373B&PID_1216&MI_01&Col02#8&1a2b3c&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}",
                Some((0x373B, 0x1216)),
            ),
            (
                r"\\?\USB#VID_3554&PID_fa09#5&abc&0&3#{a5dcbf10-6530-11d2-901f-00c04fb951ed}",
                Some((0x3554, 0xFA09)),
            ),
            (
                r"\\?\hid#vid_391d&pid_2001&mi_02#9&x&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}",
                Some((0x391D, 0x2001)),
            ),
            (
                r"\\?\Usb#Vid_3837&Pid_6011#serial#{a5dcbf10-6530-11d2-901f-00c04fb951ed}",
                Some((0x3837, 0x6011)),
            ),
            // Bluetooth spells the id differently: never a USB plug-in.
            (
                r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002046d_PID&b023#9&x",
                None,
            ),
            (
                r"\\?\ROOT#SYSTEM#0000#{97ebaacc-95bd-11d0-a3ea-00a0c9223196}",
                None,
            ),
            (r"\\?\USB#VID_37ZB&PID_1216#x", None),
            (r"\\?\USB#VID_373B&PID_12", None),
            (r"\\?\USB#VID_373B#x", None),
            ("", None),
        ];
        for (name, expected) in cases {
            assert_eq!(parse_interface_name(name), expected, "{name}");
        }
    }

    #[test]
    fn bluetooth_interfaces_are_told_apart_from_usb_ones() {
        let bluetooth = [
            r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002046d_PID&b023#9&x",
            r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b13#a&b",
            r"\\?\BTHENUM#{0000110b-0000-1000-8000-00805f9b34fb}_LOCALMFG&0002#7&c",
            r"\\?\BTHLEDevice#{0000180f-0000-1000-8000-00805f9b34fb}_Dev_VID&02#8&d",
        ];
        for name in bluetooth {
            assert!(is_bluetooth_interface(name), "{name}");
        }
        assert!(!is_bluetooth_interface(
            r"\\?\HID#VID_373B&PID_1216&MI_01#8&1a2b3c&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));
    }

    #[test]
    fn arrivals_settle_two_seconds_after_the_last_one_per_id() {
        let start = Instant::now();
        let mut arrivals = Arrivals::default();
        assert_eq!(arrivals.next_due(), None);
        arrivals.arrived((1, 1), start);
        arrivals.arrived((2, 2), start + Duration::from_millis(500));
        // A second arrival of the same id restarts its wait.
        arrivals.arrived((1, 1), start + Duration::from_secs(1));
        assert_eq!(
            arrivals.next_due(),
            Some(start + Duration::from_millis(500) + SETTLE)
        );
        assert_eq!(
            arrivals.quiet_at(),
            Some(start + Duration::from_secs(1) + SETTLE)
        );
        assert!(arrivals.take_due(start + SETTLE).is_empty());
        assert_eq!(
            arrivals.take_due(start + Duration::from_millis(2500)),
            [(2, 2)]
        );
        assert_eq!(arrivals.next_due(), Some(start + Duration::from_secs(3)));
        assert_eq!(arrivals.take_due(start + Duration::from_secs(9)), [(1, 1)]);
        assert_eq!(arrivals.next_due(), None);
        assert_eq!(arrivals.quiet_at(), None);
        assert!(arrivals.take_due(start + Duration::from_secs(9)).is_empty());
        // A device that leaves before it settles stops waiting.
        arrivals.arrived((3, 3), start);
        arrivals.forget((3, 3));
        assert_eq!(arrivals.next_due(), None);
    }

    #[test]
    fn an_id_refreshes_at_most_once_per_gap() {
        let start = Instant::now();
        let mut arrivals = Arrivals::default();
        assert!(arrivals.may_refresh((1, 1), start));
        assert!(!arrivals.may_refresh((1, 1), start + REFRESH_GAP - Duration::from_millis(1)));
        // Another id has its own gap.
        assert!(arrivals.may_refresh((2, 2), start + Duration::from_secs(1)));
        assert!(arrivals.may_refresh((1, 1), start + REFRESH_GAP));
    }

    /// MCHOSE V9 Pro 2, a verified catalogue row.
    const VERIFIED: Id = (0x3837, 0x6011);

    /// An unverified catalogue id, read from the catalogue so the test follows its data.
    fn unverified() -> Id {
        let device = protocol::KNOWN
            .iter()
            .find(|device| !device.verified && !is_recovery(device.vid, device.pid))
            .expect("an unverified catalogue row");
        (device.vid, device.pid)
    }

    /// Settings with detection on and the given lists.
    fn settings(allow: &[Id], custom: &[Device], declined: &[Id]) -> Settings {
        Settings {
            allow: allow.to_vec(),
            custom: custom.to_vec(),
            declined: declined.to_vec(),
            detect_new: true,
        }
    }

    #[test]
    fn plan_decision_table() {
        let unverified = unverified();
        let unknown_family = (0x3554, 0x7777);
        let customs = [custom(0x1234, 0x5678, Family::Compx)];
        let plain = settings(&[], &[], &[]);

        assert_eq!(plan(VERIFIED, &plain), Plan::Refresh);
        assert_eq!(
            plan(unverified, &settings(&[unverified], &[], &[])),
            Plan::Refresh
        );
        assert_eq!(
            plan((0x1234, 0x5678), &settings(&[], &customs, &[])),
            Plan::Refresh
        );
        // Queried devices refresh even when an old `declined` entry names them.
        assert_eq!(
            plan(VERIFIED, &settings(&[], &[], &[VERIFIED])),
            Plan::Refresh
        );

        assert_eq!(plan(unverified, &plain), Plan::Offer);
        assert_eq!(plan(unknown_family, &plain), Plan::Offer);

        assert_eq!(
            plan(unverified, &settings(&[], &[], &[unverified])),
            Plan::Ignore
        );
        assert_eq!(
            plan(unknown_family, &settings(&[], &[], &[unknown_family])),
            Plan::Ignore
        );
        // Recovery ids are never candidates, even when allowed; foreign VIDs are, and `offers`
        // keeps them only when they name a receiver.
        assert_eq!(plan((0x3554, 0xF2FF), &plain), Plan::Ignore);
        assert_eq!(
            plan((0x3554, 0xF405), &settings(&[(0x3554, 0xF405)], &[], &[])),
            Plan::Ignore
        );
        assert_eq!(plan((0x3710, 0x5405), &plain), Plan::Offer);
        assert_eq!(plan((0x046D, 0xC52B), &plain), Plan::Offer);
    }

    #[test]
    fn wireless_receivers_are_offered_by_name_other_usb_gear_is_not() {
        let receiver = (0x25A7, 0xFA61);
        let webcam = (0x046D, 0x0825);
        let present = [
            presence(receiver, 0, 0),
            presence(webcam, 0, 0),
            presence((0x1234, 0x0001), 0, 0),
            presence((0x1234, 0x0002), 0, 0),
            presence((0x1234, 0x0003), 0, 0),
            presence((0x3554, 0xF2FF), 0, 0),
        ];
        let product = |id: Id| match id {
            (0x25A7, 0xFA61) => Some("Wireless-Receiver"),
            (0x1234, 0x0001) => Some("USB DONGLE"),
            (0x1234, 0x0002) => Some("2.4G Mouse"),
            (0x1234, 0x0003) => Some("USB Keyboard"),
            // A recovery id stays ignored whatever it calls itself.
            (0x3554, 0xF2FF) => Some("Receiver"),
            _ => Some("HD Webcam C270"),
        };
        let plain = settings(&[], &[], &[]);
        let mut ids: Vec<Id> = offers(&present, &plugged(&present), &plain, product)
            .iter()
            .map(|offer| offer.id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, [(0x1234, 0x0001), (0x1234, 0x0002), receiver]);
        let declined = settings(&[], &[], &[receiver]);
        assert!(
            offers(&present, &[receiver], &declined, product).is_empty(),
            "a declined receiver is never offered again"
        );
        assert!(!is_receiver(None));
    }

    #[test]
    fn detection_off_offers_nothing_but_queried_devices_still_refresh() {
        let unverified = unverified();
        let customs = [custom(0x1234, 0x5678, Family::Compx)];
        let off = Settings {
            detect_new: false,
            ..settings(&[unverified], &customs, &[])
        };
        assert_eq!(plan(VERIFIED, &off), Plan::Refresh);
        assert_eq!(plan(unverified, &off), Plan::Refresh);
        assert_eq!(plan((0x1234, 0x5678), &off), Plan::Refresh);
        let bare = Settings {
            detect_new: false,
            ..Settings::default()
        };
        assert_eq!(plan(unverified, &bare), Plan::Ignore);
        assert_eq!(plan((0x3554, 0x7777), &bare), Plan::Ignore);
        let present = [presence(unverified, 1, 0), presence((0x3554, 0x7777), 1, 0)];
        assert!(offers(&present, &plugged(&present), &bare, |_| None).is_empty());
    }

    /// Every present id, as if all had just been plugged in.
    fn plugged(present: &[Presence]) -> Vec<Id> {
        present.iter().map(|p| (p.vid, p.pid)).collect()
    }

    fn presence(id: Id, openable: usize, serial: usize) -> Presence {
        Presence {
            vid: id.0,
            pid: id.1,
            hid: openable + 1,
            openable,
            serial,
        }
    }

    #[test]
    fn a_device_is_offered_on_plug_in_only_until_it_was_asked_about() {
        let id = (0x3554, 0x7777);
        let present = [presence(id, 1, 0)];
        let mut asked: Vec<Id> = Vec::new();
        // Plugged in, never asked: one offer.
        let first = offers(&present, &[id], &settings(&[], &[], &asked), |_| None);
        assert_eq!(first.len(), 1);
        // Any answer (OK, Not now, Esc) records the id, so the next plug-in offers nothing.
        asked.push(id);
        let second = offers(&present, &[id], &settings(&[], &[], &asked), |_| None);
        assert!(second.is_empty());
        // Present but not plugged in (for example at app start): never offered.
        let fresh = settings(&[], &[], &[]);
        assert!(offers(&present, &[], &fresh, |_| None).is_empty());
    }

    #[test]
    fn offers_need_a_query_channel_and_take_the_best_name() {
        let unverified = unverified();
        let unknown_family = (0x3554, 0x7777);
        let unnamed = (0x391D, 0x7777);
        let atk_serial = protocol::KNOWN
            .iter()
            .map(|device| (device.vid, device.pid, device.family))
            .find(|&(vid, pid, family)| family == Family::AtkSerial && (vid, pid) != unverified)
            .map(|(vid, pid, _)| (vid, pid))
            .expect("an ATK serial catalogue row");
        for id in [unknown_family, unnamed, (0x3554, 0x8888), (0x373B, 0x8888)] {
            assert!(known(id.0, id.1).is_none(), "{id:04X?} is in the catalogue");
        }
        let present = [
            presence(VERIFIED, 1, 0),
            presence(unverified, 1, 0),
            presence(unknown_family, 2, 0),
            presence(unnamed, 1, 0),
            // Only system collections: nothing to query.
            presence((0x3554, 0x8888), 0, 0),
            // A serial port on an unknown id is never ATK serial.
            presence((0x373B, 0x8888), 0, 1),
            presence(atk_serial, 0, 1),
            presence((0x046D, 0xC52B), 3, 0),
        ];
        let product = |id: Id| match id {
            (0x3554, 0x7777) => Some(" Gaming Mouse "),
            (0x391D, 0x7777) => Some("  "),
            _ => Some("product string"),
        };
        let plain = settings(&[], &[], &[]);
        let found = offers(&present, &plugged(&present), &plain, product);
        let ids: Vec<Id> = found.iter().map(|offer| offer.id).collect();
        let mut expected = vec![unverified, unknown_family, unnamed];
        // A verified ATK serial row refreshes instead.
        let atk_serial_verified = protocol::known(atk_serial.0, atk_serial.1)
            .unwrap()
            .verified;
        if !atk_serial_verified {
            expected.push(atk_serial);
        }
        expected.sort_unstable();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, expected);

        let name_of = |id: Id| {
            found
                .iter()
                .find(|offer| offer.id == id)
                .map(|offer| offer.name.clone())
        };
        let catalogue_name = protocol::known(unverified.0, unverified.1).unwrap().name;
        assert_eq!(name_of(unverified).as_deref(), Some(catalogue_name));
        assert_eq!(name_of(unknown_family).as_deref(), Some("Gaming Mouse"));
        assert_eq!(name_of(unnamed).as_deref(), Some("391D:7777"));

        // `only` limits the check to the plugged-in ids; declined ones drop out.
        let only = [unknown_family, VERIFIED];
        let found = offers(&present, &only, &plain, product);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, unknown_family);
        let declined = settings(&[], &[], &[unknown_family]);
        let found = offers(&present, &only, &declined, product);
        assert!(found.is_empty());
    }
}
