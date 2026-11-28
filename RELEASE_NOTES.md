## What's new in v0.5.1

### ✨ New
- **Help menu.** Right-click the tray icon → **Help** to open Juicetray on GitHub, see what's new in your version, check the list of supported devices, report a problem (on Windows the report already includes your version), or open the log folder to attach `juicetray.log` to a report.

### 🔄 Changed
- **Update check at every start.** Juicetray now checks for a new version a minute after it starts (including when your computer starts), then once a day. A new version opens the update dialog instead of a notification; choose **Later** and it won't ask again until tomorrow.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: use **Update now** in the app, or `brew upgrade juicetray`, then open `$(brew --prefix)/opt/juicetray/Juicetray.app` and choose **Move** (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
