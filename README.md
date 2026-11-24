# Juicetray

Battery levels of your wireless mice and headsets, right next to the clock — without each
vendor's software.

> **Status:** beta. Bluetooth devices and the confirmed receivers below work on Windows; the
> macOS build is compiled by CI.

## Supported devices

**Bluetooth: all devices.** Any connected Bluetooth device whose battery level the
operating system already reads (headsets, mice, keyboards, controllers, …), any brand.
Juicetray only reads the value the OS stores; it sends nothing to the device.

**2.4 GHz receiver / USB cable** — read straight from the receiver, no vendor app needed.

### Verified on real hardware

The level matches the vendor app; these are read automatically.

**Headsets**

| Device | Receiver | Notes |
|---|---|---|
| MCHOSE V9 Pro 2 | `3837:6011` | |
| ATK N9 Ultra | `373B:1204` | Close ATK HUB first: it holds the receiver's port |

**Mice**

| Device | Receiver | Notes |
|---|---|---|
| ATK X1 SE | `373B:1216` | |

### Read once, not yet checked against the vendor app

**Mice**

| Device | Receiver | Notes |
|---|---|---|
| VGN Dragonfly 3 Master+ | `391D:1A05` | Answers only while the mouse is awake |

**Keyboards**

| Device | Receiver | Notes |
|---|---|---|
| AULA F75 | `3554:FA09` | Only on 2.4 GHz; on cable the keyboard reports no level |

### Other devices

Many ATK, VXE and VGN mice use the same protocols. Until a model is verified, Juicetray never
queries it on its own. To try one, use **Add unsupported device…** in the menu (coming in the
next release): it detects the device when you plug it in, sends one read-only battery query,
and adds it once you confirm the level. If it does not work, please
[open an issue](https://github.com/ittmchien/juicetray/issues) — the window fills in the
details for you.

`allow` in the config file still works for hand-added devices:

```toml
allow = ["373B:1204"]
```

## Platforms

- Windows 10/11 (x86_64)
- macOS (Apple silicon and Intel)

## Safety

Juicetray only sends read-only battery queries, one at a time, with no retry loops. It never
changes a device setting. Bluetooth levels are read from what the OS already stores, so no
query reaches those devices. A receiver or cable device is queried at most once per 10 s
(mice) or 5 min (headsets), whatever the auto-refresh setting.

## Diagnostics

- `juicetray.log` sits next to `query-log.toml`. A release build writes it only when something
  goes wrong (panic, hang dump, error); run with `JUICETRAY_TRACE=1` to trace every step
  (debug builds always do).
- `juicetray scan` lists Bluetooth, HID and serial devices, writes nothing, and redacts serial
  numbers.
- `juicetray probe <VID:PID>` (optional; the menu's Check battery now confirms a device without
  it) sends one read-only battery query to a supported device and prints the raw reply. Close
  the vendor app first; probe refuses within the device's floor.
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
