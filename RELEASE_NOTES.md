## What's new in v0.5.0

### ✨ New
- **Delux M500 (Windows).** The mouse's battery is now read through its 2.4 GHz receiver. While the mouse is off, the last known level is shown as asleep; while it is on its cable, it shows as charging.
- **Device reports hear the device (Windows).** When a new device is plugged in, Juicetray listens to it for 30 s, read-only, and adds what it sends on its own to the Add device report, so it can often be added from the report alone. Reports sent while keys are typed are dropped.

### 🛠️ Improved
- **More useful device reports (Windows).** Each collection line in a device report (Add device → Send report) now ends with the report lengths Windows lists for it, read without sending anything to the device. This makes new devices quicker to add to the catalogue.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: use **Update now** in the app, or `brew upgrade juicetray`, then open `$(brew --prefix)/opt/juicetray/Juicetray.app` and choose **Move** (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
