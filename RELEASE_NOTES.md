## What's new in v0.4.5

### 🐛 Fixed
- **No more "Where is use_default?" window (macOS).** The first notification (a low battery, an update available or starting) could open a "Choose Application" window asking for an app named `use_default`. Notifications now come from Juicetray itself, so the window no longer appears.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: use **Update now** in the app, or `brew upgrade juicetray`, then open `$(brew --prefix)/opt/juicetray/Juicetray.app` and choose **Move** (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
