# Juicetray

Battery levels of your wireless mice and headsets, right next to the clock — without each
vendor's software.

> **Status:** beta. Bluetooth devices and the confirmed receivers below work on Windows; the
> macOS build is compiled by CI.

## Supported devices

**Bluetooth: all devices.** Any connected Bluetooth device whose battery level the
operating system already reads (headsets, mice, keyboards, controllers, …), any brand.
Juicetray only reads the value the OS stores; it sends nothing to the device.

**2.4 GHz receiver / USB cable:**

| Device | Kind | Status |
|---|---|---|
| MCHOSE V9 Pro 2 (`3837:6011` dongle) | Headset | Confirmed |
| ATK X1 SE (`373B:1216` dongle) | Mouse | Protocol known, waiting for a probe |
| ATK N9 Ultra (`373B:1204` dongle) | Headset | Protocol known; close ATK HUB, then probe |
| VGN Dragonfly 3 Master+ (`3554:FA09` dongle, `391D:1A05` cable) | Mouse | Not supported yet: no reply to the known commands |

Only confirmed devices are queried automatically. An unconfirmed device is listed but never
touched until you confirm it with `juicetray probe <VID:PID>` and add it to `allow` in the
config file:

```toml
allow = ["373B:1204"]
```

More devices will be added over time.

## Platforms

- Windows 10/11 (x86_64)
- macOS (Apple silicon and Intel)

## Safety

Juicetray only sends read-only battery queries, one at a time, with no retry loops. It never
changes a device setting. Bluetooth levels are read from what the OS already stores, so no
query reaches those devices. A receiver or cable device is queried at most once per 10 s
(mice) or 5 min (headsets), whatever the auto-refresh setting.

## Diagnostics

- `juicetray scan` lists Bluetooth, HID and serial devices, writes nothing, and redacts serial
  numbers.
- `juicetray probe <VID:PID>` sends one read-only battery query to a supported device and
  prints the raw reply. Close the vendor app first; probe refuses within the device's floor.
- `juicetray probe --online <VID:PID>` sends the read-only Compx online-status query instead
  (Compx devices only, same floor) and prints `online: yes/no/unsupported`.
- `juicetray listen <VID:PID> [seconds]` only reads (1-60 s, default 10) and prints each
  report with its time in ms, to see what a vendor app's own polling gets back; nothing is sent.

## Contributing

`main` holds released code only. Development happens on `dev`:

```sh
git clone -b dev https://github.com/ittmchien/juicetray
```

- Open pull requests against `dev`.
- When forking, untick **"Copy the `main` branch only"** so your fork gets `dev` too.

## License

[GPL-3.0-or-later](LICENSE). The Windows exe prints all bundled licences with
`juicetray licenses`; the macOS app keeps them in `Contents/Resources/licenses/`.

## Credits

Protocol notes from M-HUB-linux-port (MCHOSE V9 Pro 2), the OpenMouse device catalogue
(ATK), the Linux `hid-kysona` driver, [Joule](https://github.com/DillanCidral/Joule) and
[sanshoku](https://github.com/ushineko/sanshoku). Only protocol facts are used; no code is
copied.

Device icons: [Material Symbols](https://github.com/google/material-design-icons) by Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Numbers: [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
