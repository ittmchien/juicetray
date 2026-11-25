## What's new in v0.4.2

### ✨ New
- **macOS: updates in one click.** Juicetray now checks GitHub Releases once a day on macOS too (turn it off with **Settings → Check for updates automatically**; on a MacBook running on battery the check waits for AC unless **Refresh on battery power** is on). When a newer version exists, the menu shows **Update available**, and **Check for updates…** answers in a dialog.
- **Update now runs Homebrew for you.** For a Homebrew install, **Update now** runs `brew upgrade ittmchien/tap/juicetray`, replaces `/Applications/Juicetray.app` with the new copy and restarts. A failed step shows the reason and keeps the running version. Without Homebrew, the dialog opens the download page.
- **macOS: Move to Applications.** Started from anywhere outside Applications (such as Homebrew's folder), Juicetray asks to move itself there: **Move** copies the app into Applications and restarts from the copy (the Homebrew copy stays, since updates need it); **Not now** is remembered and not asked again (copy `Juicetray.app` into Applications yourself to move later). Installing is now `brew install juicetray`, then `open "$(brew --prefix)/opt/juicetray/Juicetray.app"` and **Move**.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: use **Update now** in the app (v0.4.2 and later), or `brew upgrade juicetray`, then open `$(brew --prefix)/opt/juicetray/Juicetray.app` and choose **Move** (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
