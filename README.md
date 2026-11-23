# Juicetray

Battery levels of your wireless mice and headsets, right next to the clock — without each
vendor's software.

> **Status:** in development. Not usable yet.

## Supported devices

**Bluetooth: all devices.** Any connected Bluetooth device whose battery level the
operating system already reads (headsets, mice, keyboards, controllers, …), any brand.
Juicetray only reads the value the OS stores; it sends nothing to the device.

**2.4 GHz receiver / USB cable:**

| Device | Kind |
|---|---|
| VGN Dragonfly F3 | Mouse |
| ATK X1 SE | Mouse |
| ATK N9 Ultra | Headset |
| MCHOSE V9 Pro 2 | Headset |

More devices will be added over time.

## Platforms

- Windows 10/11 (x86_64)
- macOS (Apple silicon and Intel)

## Safety

Juicetray only sends read-only battery queries, one at a time, with no retry loops and at
most one automatic refresh every few minutes. It never changes a device setting.

## Contributing

`main` holds released code only. Development happens on `dev`:

```sh
git clone -b dev https://github.com/ittmchien/juicetray
```

- Open pull requests against `dev`.
- When forking, untick **"Copy the `main` branch only"** so your fork gets `dev` too.

## License

[GPL-3.0-or-later](LICENSE).

## Credits

Protocol notes from M-HUB-linux-port (MCHOSE V9 Pro 2), the OpenMouse device catalogue
(ATK) and the Linux `hid-kysona` driver.

Device icons: [Material Symbols](https://github.com/google/material-design-icons) by Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Numbers: [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
