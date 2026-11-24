<div align="center">

<img src="docs/assets/icon.png" width="72" alt="Juicetray icon">

# Juicetray

**Battery levels of your wireless mice, headsets and keyboards, right next to the clock —
without each vendor's software.**

[![Release](https://img.shields.io/github/v/release/ittmchien/juicetray?include_prereleases&label=release)](https://github.com/ittmchien/juicetray/releases)
[![CI](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6?logo=windows)](#-platforms)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

**English** · [Tiếng Việt](README.vi.md)

</div>

> [!NOTE]
> **Beta, Windows only for now.** Bluetooth devices and the verified receivers below work on
> Windows; a macOS release follows once it has been tested on a Mac.

## ✨ Features

- 🔋 **Battery on the taskbar** — one small icon per device with a gauge and its level, on every
  monitor's taskbar; hover for a summary, right-click for the menu.
- 🎨 **Status colours** — the gauge turns green while charging, yellow below 50 % and red at your
  low-battery threshold; low devices also say *needs charge* in the menu.
- 🖱️ **2.4 GHz receivers and cables** — read straight from the receiver; no vendor app has to run.
- 📶 **Every Bluetooth device** — headsets, mice, keyboards, controllers… whatever Windows already
  knows the battery of.
- 🗂️ **Grouped and sorted** — devices grouped by kind, highest level first, offline ones last and
  marked ⊘; hide offline devices after a delay you choose.
- 🔔 **Low-battery alert** — a notification at the threshold you pick (5–30 % or custom).
- 🛠️ **Per-device options** — show or hide, percentage, label, icon, and icon colours for a light
  and a dark taskbar.
- 🪶 **Featherweight** — about 10 MB of RAM, ~0.05 % CPU at idle, a 2 MB portable exe; it
  pauses auto-refresh on laptop battery unless you ask otherwise.
- 🔄 **Updates in one click** — checks GitHub Releases once a day; **Update now** verifies the
  download's SHA-256 before swapping the exe.
- 🛡️ **Read-only and gentle** — never changes a device setting; see Safety below.

## ⬇️ Install

1. Download `juicetray-windows-x86_64.exe` from the
   [latest release](https://github.com/ittmchien/juicetray/releases/latest).
2. Put it in a folder you can write to (for example `%LOCALAPPDATA%\Programs\Juicetray`) and run
   it. It is portable: no installer, nothing else to set up.
3. Right-click the strip or tray icon → **Start at login** to start it with Windows.

Settings live in `%APPDATA%\ittmchien\juicetray\config\config.toml`, so replacing the exe keeps
them. Windows SmartScreen may warn about an unsigned app: **More info → Run anyway**.

## 🎧 Supported devices

**📶 Bluetooth — all devices.** Any connected Bluetooth device whose battery level Windows already
reads, any brand. Juicetray only reads the value Windows stores; it sends nothing to the device.

**🔌 2.4 GHz receiver / USB cable** — read straight from the receiver.

### ✅ Verified on real hardware

Battery read correctly on the owner's devices; these are read automatically.

| | Device | Receiver | Notes |
|---|---|---|---|
| 🎧 | MCHOSE V9 Pro 2 | `3837:6011` | |
| 🎧 | ATK N9 Ultra | `373B:1204` | Close ATK HUB first: it holds the receiver's port |
| 🖱️ | ATK X1 SE | `373B:1216` | |

### 🧪 Read once, not yet checked against the vendor app

| | Device | Receiver | Notes |
|---|---|---|---|
| 🖱️ | VGN Dragonfly 3 Master+ | `391D:1A05` | Answers only while the mouse is awake |
| ⌨️ | AULA F75 | `3554:FA09` | Only on 2.4 GHz; on cable the keyboard reports no level |

### ➕ Other devices

Many ATK, VXE and VGN mice use the same protocols. Until a model is verified, Juicetray never
queries it on its own. To try one, use **Add unsupported device…** in the menu (coming in the
next release): it detects the device when you plug it in, sends one read-only battery query, and
adds it once you confirm the level. If it does not work, please
[open an issue](https://github.com/ittmchien/juicetray/issues) — the window fills in the details
for you.

`allow` in the config file still works for hand-added devices:

```toml
allow = ["391D:1A05"]
```

## 💻 Platforms

| Platform | Status |
|---|---|
| 🪟 Windows 10 / 11 (x86_64) | ✅ Released |
| 🍎 macOS (Apple silicon and Intel) | 🚧 Builds in CI, not released yet |

## 🔄 Updates

Juicetray checks [GitHub Releases](https://github.com/ittmchien/juicetray/releases) for a newer
version at most once a day (restarting the app does not check again). Turn it off with **Check for updates
automatically** in the menu, or check by hand with **Check for updates…**. The request sends
nothing but the version in the User-Agent (`juicetray/<version>`).

**Update now** downloads the new exe, checks its SHA-256 against the release's
`SHA256SUMS.txt`, swaps it in and restarts; if anything fails, the old exe stays in place. When
the exe's folder is not writable it opens the download page instead.

## 🛡️ Safety

- Only read-only battery and online-status queries, one at a time, with no retry loops.
- Never changes a device setting.
- Bluetooth levels come from what Windows already stores, so no query reaches those devices.
- A receiver or cable device is queried at most once per 10 s (mice) or 5 min (headsets),
  whatever the auto-refresh setting.

## 🩺 Diagnostics

| Command | What it does |
|---|---|
| `juicetray scan` | Lists Bluetooth, HID and serial devices; writes nothing; redacts serial numbers. |
| `juicetray probe <VID:PID>` | Sends one read-only battery query to a supported device and prints the raw reply. Close the vendor app first; refuses within the device's floor. Optional: **Add unsupported device…** in the menu tries a device without it. |
| `juicetray probe --online <VID:PID>` | Sends the read-only online-status query instead (Compx and ATK N9 Ultra, same floor) and prints `online: yes/no/unsupported`. |
| `juicetray listen <VID:PID> [seconds]` | Only reads (1–60 s, default 10) and prints each report with its time in ms; nothing is sent. |

`juicetray.log` sits next to `query-log.toml` in `%LOCALAPPDATA%\ittmchien\juicetray\data`. A
release build writes it only when something goes wrong (panic, hang dump, error); set
`JUICETRAY_TRACE=1` to trace every step (debug builds always do).

## 🤝 Contributing

`main` holds released code only. Development happens on `dev`:

```sh
git clone -b dev https://github.com/ittmchien/juicetray
```

- Open pull requests against `dev`.
- When forking, untick **"Copy the `main` branch only"** so your fork gets `dev` too.

### Add a device model

A model is one line in its brand's file under `crates/protocol/devices/` (for example
`atk.toml`), with `vid` and `pid` as 4 hex digits, `name`, `family`, `kind` (`mouse`, `headset`,
`keyboard`, `gamepad` or `other`), `connection` (`receiver` or `cable`) and `verified`. `family`
is one of `Compx`, `Report13`, `Mchose`, `AtkSerial`, `AtkBitmouse`, `VgnKc2` or `RongYuanF7`;
a model that needs another protocol needs a new family in code first. The build checks every line (unknown family or kind,
bad hex, duplicate VID:PID) and fails with the file and entry named. Open the pull request against
`dev` with `verified = false`; a model becomes `verified = true` only after a hardware check
(`juicetray probe <VID:PID>` read the same level as the vendor app).

## 📜 License

[GPL-3.0-or-later](LICENSE). The Windows exe prints all bundled licences with
`juicetray licenses`; the macOS app keeps them in `Contents/Resources/licenses/`.

## 🙏 Credits

Protocol notes from M-HUB-linux-port (MCHOSE V9 Pro 2), the OpenMouse device catalogue (ATK),
the Linux `hid-kysona` driver, [Joule](https://github.com/DillanCidral/Joule) and
[sanshoku](https://github.com/ushineko/sanshoku). Only protocol facts are used; no code is copied.

Device icons: [Material Symbols](https://github.com/google/material-design-icons) by Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Numbers on macOS (fallback font;
Windows uses its own system fonts): [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
