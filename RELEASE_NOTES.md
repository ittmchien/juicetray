## What's new in v0.4.1

### 🛠 Fixed
- **macOS: a headset that reconnects shows its battery again.** macOS kept an old value inside the running app; Juicetray now reads Bluetooth in a short-lived helper, so the level is always current.
- **macOS: the menu-bar strip is white on a dark menu bar from the start.** It no longer stays black until the menu is opened, and it follows menu-bar tint changes.
- **macOS: Start at login can be turned on.** It no longer shows "(app bundle only)" for an app that was never registered.

### 🎨 Improved
- **Refresh when a Bluetooth device connects or disconnects** (Windows and macOS), and after the Mac wakes from sleep — no waiting for the next auto-refresh.
- **Refresh on battery power** now works on macOS too: auto-refresh pauses on a MacBook running on battery unless you allow it.
- **Homebrew installs the prebuilt app** (about 3 MB, a few seconds, no build tools): `brew tap ittmchien/tap`, `brew trust ittmchien/tap`, `brew install juicetray`. See *Install → macOS* in the README.
- macOS now covers Bluetooth devices only; receivers and cables stay Windows-only for now.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: `brew upgrade juicetray` (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
