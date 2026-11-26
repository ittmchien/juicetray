## What's new in v0.4.4

### ✨ New
- **New models without waiting for a release (Windows).** When you plug in a device your version does not know yet, the "New device found" popup first looks it up in the device catalogue on GitHub. If the catalogue lists it, the popup shows its name and **OK** adds it at once, without the Add device window; it then works offline like any added device. No network, or not listed: the popup works as before. Only the public catalogue files are downloaded (the device ID is never sent), the wait is capped at a few seconds, and turning off **Detect new devices** stops it.

### 🛠️ Improved
- **Added devices keep working after an update.** A device you added yourself stays read when a later version lists the same model in its catalogue.
- **Homebrew follows every release.** The Homebrew formula is now updated automatically with each release, so `brew upgrade juicetray` sees a new version as soon as it is published.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: use **Update now** in the app, or `brew upgrade juicetray`, then open `$(brew --prefix)/opt/juicetray/Juicetray.app` and choose **Move** (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
