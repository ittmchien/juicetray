<div align="center">

<img src="docs/assets/icon.png" width="72" alt="Juicetray icon">

# Juicetray

**Don't get caught low on juice.** 🧃

Juicetray keeps the battery ("juice") of your wireless mice, headsets and keyboards in your
tray — right next to the clock, without each vendor's software.

[![Release](https://img.shields.io/github/v/release/ittmchien/juicetray?include_prereleases&label=release)](https://github.com/ittmchien/juicetray/releases)
[![CI](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011%20%7C%20macOS%2013%2B-0078D6?logo=windows)](#-platforms)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

**English** · [Tiếng Việt](README.vi.md)

</div>

> [!NOTE]
> **Beta.** Bluetooth devices and the verified receivers below work on Windows. **macOS supports
> Bluetooth devices only** for now (install with Homebrew); receivers and cables are Windows-only.

## ✨ Features

- 🔋 **Battery on the taskbar / menu bar** — (Windows) one small icon per device with a gauge and
  its level, on every monitor's taskbar; hover for a summary, right-click for the menu. On macOS,
  one menu-bar item draws all devices; click it to open the menu.
- 🎨 **Status colours** — the gauge turns green while charging, yellow below 50 % and red at your
  low-battery threshold; low devices also say *needs charge* in the menu.
- 🖱️ **2.4 GHz receivers and cables** (Windows) — read straight from the receiver; no vendor app
  has to run.
- 📶 **Every Bluetooth device** — headsets, mice, keyboards, controllers… whatever Windows or macOS
  reports the battery of.
- 🗂️ **Grouped and sorted** — devices grouped by kind, highest level first, offline ones last and
  marked *disconnected*; hide offline devices after a delay you choose.
- 🔔 **Low-battery alert** — a notification at the threshold you pick (5–30 % or custom).
- 🛠️ **Per-device options** — show or hide, percentage, label, icon, and icon colours for a light
  and a dark taskbar / menu bar. On macOS, AirPods and Beats earbuds show Apple's own symbol.
- 🪶 **Featherweight** — about 10 MB of RAM, ~0.05 % CPU at idle, a 2 MB portable exe on Windows;
  it pauses auto-refresh on laptop battery unless you turn on **Refresh on battery power**.
- 🔄 **Updates in one click** (Windows) — checks GitHub Releases once a day; **Update now**
  verifies the download's SHA-256 before swapping the exe. On macOS: `brew upgrade`.
- 🛡️ **Read-only and gentle** — never changes a device setting; see Safety below.

## ⬇️ Install

### 🪟 Windows

1. Download `juicetray-windows-x86_64.exe` from the
   [latest release](https://github.com/ittmchien/juicetray/releases/latest).
2. Put it in a folder you can write to (for example `%LOCALAPPDATA%\Programs\Juicetray`) and run
   it. It is portable: no installer, nothing else to set up.
3. Right-click the strip or tray icon → **Settings → Start at login** to start it with Windows.

Settings live in `%APPDATA%\ittmchien\juicetray\config\config.toml`, so replacing the exe keeps
them. Windows SmartScreen may warn about an unsigned app: **More info → Run anyway**.

### <img src="https://cdn.simpleicons.org/apple/8b949e" height="16" alt=""> macOS (beta, Homebrew)

Homebrew installs the prebuilt universal app from the GitHub release: about a 3 MB download, no
Rust toolchain, done in a few seconds. Homebrew downloads it with curl and adds no quarantine
flag, so Gatekeeper does not block it:

```sh
brew tap ittmchien/tap
brew trust ittmchien/tap   # needed on Homebrew 7+
brew install juicetray
cp -R "$(brew --prefix)/opt/juicetray/Juicetray.app" /Applications/
```

Open **Juicetray** from Applications and allow Bluetooth when asked. **Settings → Start at login**
works too (macOS may ask you to approve it in System Settings › General › Login Items). Settings
live in `~/Library/Application Support/io.github.ittmchien.juicetray/config.toml`.

To update, quit Juicetray, then:

```sh
brew upgrade juicetray
rm -rf /Applications/Juicetray.app && cp -R "$(brew --prefix)/opt/juicetray/Juicetray.app" /Applications/
```

## 🎧 Supported devices

**📶 Bluetooth — all devices.** Any connected Bluetooth device whose battery level Windows or
macOS reports, any brand. Juicetray only reads the value the OS reports; it sends nothing to the
device.

**🔌 2.4 GHz receiver / USB cable** (Windows only) — read straight from the receiver.

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

**Windows only.** Many ATK, VXE and VGN mice use the same protocols. Until a model is verified,
Juicetray never queries it on its own. To try one, use **Add device…** in the menu, or **OK** on
the "New device found" popup (shown when you plug in a supported brand or any wireless receiver
while Juicetray runs, never at start-up, and only once per device whatever you answer): it
detects the device, sends one read-only battery query when you press **Try**, and adds it once
you confirm the level. If it does not work, press **Send report** to send the details through a
[Google Form](https://forms.gle/Qp5CeBGEiPg2UtkX6) (no account needed), or **Report on GitHub**
to [open an issue](https://github.com/ittmchien/juicetray/issues) with them filled in. Only the
device's name, USB ID and interfaces are sent — nothing personal.

📋 **[Device list](https://docs.google.com/spreadsheets/d/e/2PACX-1vRTKUOHRmpLeXhjxL93T5HwuYsJKnzMHSUhN2AWslNCB938EpQH2YkMeNubTcpvdxpHC2o7Dw4OHfS7/pubhtml)**
— every model Juicetray knows (**Supported**) and the devices users asked for, with their status
(**Requested**).

`allow` in the config file still works for hand-added devices:

```toml
allow = ["391D:1A05"]
```

## 💻 Platforms

| Platform | Status |
|---|---|
| 🪟 Windows 10 / 11 (x86_64) | ✅ Released |
| <img src="https://cdn.simpleicons.org/apple/8b949e" height="16" alt=""> macOS 13+ (Apple silicon and Intel) | 🧪 Beta via Homebrew — Bluetooth only, no receivers/cables yet |

## 🔄 Updates

On Windows, Juicetray checks [GitHub Releases](https://github.com/ittmchien/juicetray/releases)
for a newer version at most once a day (restarting the app does not check again). Turn it off
with **Check for updates automatically** under **Settings**, or check by hand with **Check for
updates…**. The request sends nothing but the version in the User-Agent (`juicetray/<version>`).

On Windows, **Update now** downloads the new exe, checks its SHA-256 against the release's
`SHA256SUMS.txt`, swaps it in and restarts; if anything fails, the old exe stays in place. When
the exe's folder is not writable it opens the download page instead.

On macOS the app does not check by itself: **Check for updates…** opens the releases page, and
Homebrew installs update with `brew upgrade juicetray` (see Install).

## ⚠️ Known issues

- **"Windows protected your PC"** — the exe is not code-signed yet (see Install). Installing with
  winget (package under review) will avoid the warning.
- Some receivers answer only while the device is awake (VGN Dragonfly 3 Master+), and some
  keyboards report no level over a cable (AULA F75).

## 🛡️ Safety

- Only read-only battery and online-status queries, one at a time, with no retry loops.
- Never changes a device setting.
- Bluetooth levels come from what Windows or macOS reports, so no query reaches those devices.
- A receiver or cable device is queried at most once per 10 s (mice) or 5 min (headsets),
  whatever the auto-refresh setting.
- While a brand's app is open (MCHOSE HUB, ATK HUB, VGN VHUB), Juicetray pauses that brand's
  receivers and cables: nothing is opened or sent, and the menu keeps the last level marked
  `paused (… open)`. Reading resumes on the next refresh after the app closes.

## 🩺 Diagnostics

| Command | What it does |
|---|---|
| `juicetray scan` | Lists Bluetooth, HID and serial devices; writes nothing; redacts serial numbers. |
| `juicetray probe <VID:PID>` | Sends one read-only battery query to a supported device and prints the raw reply. Close the vendor app first; refuses within the device's floor. Optional: **Add device…** in the menu tries a device without it. Receiver tool (Windows). |
| `juicetray probe --online <VID:PID>` | Sends the read-only online-status query instead (Compx and ATK N9 Ultra, same floor) and prints `online: yes/no/unsupported`. Receiver tool (Windows). |
| `juicetray listen <VID:PID> [seconds]` | Only reads (1–60 s, default 10) and prints each report with its time in ms; nothing is sent. Receiver tool (Windows). |

On macOS there is no `juicetray` on PATH: run
`/Applications/Juicetray.app/Contents/MacOS/juicetray scan` (or `… licenses`).

`juicetray.log` sits next to `query-log.toml` in `%LOCALAPPDATA%\ittmchien\juicetray\data`
(macOS: `~/Library/Application Support/io.github.ittmchien.juicetray/`). Dev and beta builds
trace every step; an official release writes only when something goes wrong (panic, hang dump
on Windows, error) unless started with `JUICETRAY_TRACE=1`.

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
is one of `Compx`, `Report13`, `Mchose`, `AtkSerial`, `AtkBitmouse`, `VgnKc2`, `RongYuanF7`,
`Driveall` or `Generic`; a model whose protocol none of them covers can often be a `Generic` row
(below), otherwise it needs a new family in code first. The build checks every line (unknown
family or kind, bad hex, duplicate VID:PID) and fails with the file and entry named. Open the pull
request against `dev` with `verified = false`; a model becomes `verified = true` only after a
hardware check (`juicetray probe <VID:PID>` read the same level as the vendor app).

#### Generic rows

A `Generic` row carries its own query instead of family code. Its extra fields are all required
(except the last three) and are refused on any other family:

- `usage_page`, `usage`: the vendor collection, 4 hex digits (`FF00`–`FFFF` only; page `008C`
  and system collections are refused).
- `report`: `output` (answered by an input report) or `feature` (SetFeature, then GetFeature of
  the same id); `report_bytes`: that report's size exactly as `juicetray scan` prints it
  (`out 05:33` → 33).
- `request`: the one frame sent, hex, report id first, at most 64 bytes and no longer than the
  report; it is zero-padded to the report.
- `reply_prefix`: hex bytes a valid reply starts with, report id first (an output row needs a
  byte after the id); other replies are ignored.
- `level_byte`: index of the level in the reply, report id = byte 0 (also when the id is 0).
- optional `charging_byte` + `charging_mask` (together; masked nonzero = charging) and `scale`
  (default 100; percent = raw × 100 / scale, anything over 100 is rejected).

```toml
{ vid = "1234", pid = "5678", name = "Example 2.4G dongle", family = "Generic", kind = "mouse", connection = "receiver", verified = false, usage_page = "FF00", usage = "0001", report = "output", report_bytes = 33, request = "05 01", reply_prefix = "05 81", level_byte = 3, charging_byte = 4, charging_mask = 0x01 }
```

The example is made up: it sends output report `05 01` (padded to 33 bytes) once per query and
reads byte 3 of an input report starting `05 81` as the percent. Like ATK serial, RongYuan F7
and Driveall, a Generic row is used only for its own catalogue VID:PID; a custom device or the
Add device window never gets it. One fixed frame per query, no retries, 10 s query floor.

**From a device report to a row.** The Add device report lists every collection of the device
as `scan` prints it: `HID if 2 FF00:0001 vendor · Usb · in 05:33 out 05:33` gives `usage_page`,
`usage`, `report` and `report_bytes`, and the descriptor hex below it lets a reviewer check the
sizes. The request and the reply layout come from what the brand's app sends and receives (a
USB capture); `level_byte` is the reply byte that equals the percent the brand's app shows.
Pick a request the brand's app sends only to read status, never a setter.

## 📜 License

[GPL-3.0-or-later](LICENSE). The Windows exe prints all bundled licences with
`juicetray licenses`; the macOS app keeps them in `Contents/Resources/licenses/`.

## 🙏 Credits

Device icons: [Material Symbols](https://github.com/google/material-design-icons) by Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Fallback font for the numbers when the
system font cannot be used: [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
