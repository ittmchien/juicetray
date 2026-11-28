# Install and settings

Back to the [README](../README.md).

## Settings file

- Windows: `%APPDATA%\ittmchien\juicetray\config\config.toml` (replacing the exe keeps it).
- macOS: `~/Library/Application Support/io.github.ittmchien.juicetray/config.toml`.

## macOS

Homebrew installs the prebuilt universal app from the GitHub release (about 3 MB, no Rust toolchain). It downloads with curl and adds no quarantine flag, so Gatekeeper does not block it.

Juicetray asks to move itself to Applications: **Move** copies the app there and restarts from the
copy (the Homebrew copy stays, since updates need it); **Not now** is remembered and not asked
again. To move later, copy `Juicetray.app` into Applications yourself. Allow Bluetooth when asked.
**Settings → Start at login** works too (macOS may ask you to approve it in System Settings ›
General › Login Items). Settings live in
`~/Library/Application Support/io.github.ittmchien.juicetray/config.toml`.

## Features in detail

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
- 🛠️ **Per-device options** — click a device in the menu: show or hide it, its percentage, its
  name under the icon, the icon (Generic, Headset, Mouse… or your own SVG) and icon colours, with
  **Custom** picks for both or for a light / dark taskbar / menu bar. On macOS, AirPods and Beats
  earbuds show Apple's own symbol.
- 🪶 **Featherweight** — about 10 MB of RAM, ~0.05 % CPU at idle, a 2 MB portable exe on Windows;
  it pauses auto-refresh on laptop battery unless you turn on **Refresh on battery power**.
- 🔄 **Updates in one click** — checks GitHub Releases at every start and once a day, and asks
  at most once a day after **Later**; **Update now** verifies the
  download's SHA-256 before swapping the exe on Windows, and runs `brew upgrade` on macOS.
- 🛡️ **Read-only and gentle** — never changes a device setting; see Safety in the [README](../README.md#%EF%B8%8F-safety).
