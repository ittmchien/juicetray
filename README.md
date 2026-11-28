<div align="center">

<img src="docs/assets/icon.png" width="72" alt="Juicetray icon">

# Juicetray

**Don't get caught low on juice.** 🧃

Juicetray keeps the battery ("juice") of your wireless mice, headsets and keyboards in your
tray — right next to the clock, without each vendor's software.

[![Release](https://img.shields.io/github/v/release/ittmchien/juicetray?include_prereleases&label=release)](https://github.com/ittmchien/juicetray/releases)
[![CI](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011%20%7C%20macOS%2013%2B-0078D6?logo=windows)](#%EF%B8%8F-install)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

**English** · [Tiếng Việt](README.vi.md)

</div>

> [!NOTE]
> **Beta.** Windows 10 / 11 reads Bluetooth devices, 2.4 GHz receivers and cables. macOS 13+
> (Homebrew) reads Bluetooth devices only for now.

## ✨ Features

- 🔋 **Battery next to the clock** — one icon per device on every taskbar (Windows), or one
  menu-bar item (macOS).
- 🎨 **Status colours** — green while charging, yellow below 50 %, red at your low-battery level.
- 🖱️ **2.4 GHz receivers and cables** (Windows) — read straight from the receiver, no vendor app.
- 📶 **Every Bluetooth device** whose battery Windows or macOS reports.
- 🔔 **Low-battery alert** at the level you pick.
- 🛠️ **Per-device options** — hide a device, its percentage or name; pick its icon and colours.
- 🪶 **Featherweight** — about 10 MB of RAM, ~0.05 % CPU at idle, a 2 MB portable exe.
- 🔄 **One-click updates** — checked once a day, verified before install.

## ⬇️ Install

### 🪟 Windows

1. Download `juicetray-windows-x86_64.exe` from the
   [latest release](https://github.com/ittmchien/juicetray/releases/latest).
2. Put it in a folder you can write to (for example `%LOCALAPPDATA%\Programs\Juicetray`) and run
   it. No installer needed.
3. Right-click the tray icon → **Settings → Start at login** to start it with Windows.

SmartScreen may warn about an unsigned app: **More info → Run anyway**.

### <img src="https://cdn.simpleicons.org/apple/8b949e" height="16" alt=""> macOS (beta, Homebrew)

```sh
brew tap ittmchien/tap
brew trust ittmchien/tap   # needed on Homebrew 7+
brew install juicetray
open "$(brew --prefix)/opt/juicetray/Juicetray.app"
```

Choose **Move** when asked to copy the app into Applications, and allow Bluetooth.

## 🎧 Supported devices

📋 **[Device list](https://docs.google.com/spreadsheets/d/e/2PACX-1vRTKUOHRmpLeXhjxL93T5HwuYsJKnzMHSUhN2AWslNCB938EpQH2YkMeNubTcpvdxpHC2o7Dw4OHfS7/pubhtml)**
— every supported model and the devices users asked for. In the app: **Help → Supported devices**.

- **📶 Bluetooth:** any device whose battery the OS reports.
- **🔌 2.4 GHz receivers and cables** (Windows): the verified models in the list.

Device not listed? Use **Add device…** in the menu to try it, or send a report from there so it
can be added. See [Adding a device](docs/adding-devices.md).

## ⚠️ Known issues

- **"Windows protected your PC"** — the exe is not code-signed yet. Installing with winget
  (package under review) will avoid the warning.
- Some receivers answer only while the device is awake (VGN Dragonfly 3 Master+), and some
  keyboards report no level over a cable (AULA F75).

## 🛡️ Safety

- Read-only: battery and online-status queries only, one at a time, no retries. Device settings
  are never changed.
- Receivers and cables are queried at most every 10 s (mice) or 5 min (headsets).
- While a brand's app is open (MCHOSE HUB, ATK HUB, VGN VHUB, M500), that brand's devices are left
  alone and the menu shows the last level as `paused (… open)`.

## 📚 More

- [Install and settings](docs/install.md) — settings file, macOS details, every feature in detail.
- [Updates](docs/updates.md) — how update checks and **Update now** work, and what they send.
- [Diagnostics](docs/diagnostics.md) — the log (**Help → Open log folder**) and the `scan`,
  `probe` and `listen` commands.
- [Adding a device](docs/adding-devices.md) — trying a device, reports, and catalogue rows.

## 🤝 Contributing

`main` holds released code only; open pull requests against `dev`
(`git clone -b dev https://github.com/ittmchien/juicetray`). When forking, untick **"Copy the
`main` branch only"**.

## 📜 License

[GPL-3.0-or-later](LICENSE). Bundled licences: `juicetray licenses` (Windows) or
`Contents/Resources/licenses/` (macOS).

## 🙏 Credits

Device icons: [Material Symbols](https://github.com/google/material-design-icons) by Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Fallback font for the numbers:
[Inter](https://rsms.me/inter/) ([OFL-1.1](crates/strip-render/assets/OFL.txt)).
