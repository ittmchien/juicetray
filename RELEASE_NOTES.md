## What's new in v0.4.3

### ✨ New
- **ATK BITMOUSE mice on their USB cable.** A BITMOUSE mouse plugged in by its cable (no receiver) now shows its battery. Juicetray sends the query the way the mouse expects on a cable, and when a non-verified device stays silent it asks the device whether it is on a cable or a receiver and tries once more the right way. The **ATK X1 SE Nearlink** (`373B:1299`) on its cable is verified against ATK HUB; the **ATK X1 SE NK** (`373B:130E`) is listed as a cable mouse too.
- **Devices first in the menu.** The tray menu lists your devices at the top, then **Refresh now** and **Settings**, then **Check for updates…** and **Quit**. Settings are grouped; each device's submenu starts with an info row and greys its options while the device is hidden. **Show label** is now **Show name under icon**, the Bluetooth icon is called **Generic**, and the custom icon colours sit in a **Custom** submenu (your saved choices are kept).
- **Update progress.** While **Update now** runs, a notification says it started and the top of the menu shows the current step; **Check for updates…** is greyed out until it finishes.

### 🛠️ Improved
- **Add device remembers the connection.** When you try an unlisted device and Juicetray detects that it is on a cable, **Add** preselects *cable* (you can still change it). The [Try] button no longer asks you to confirm a USB receiver first.
- **Fuller device reports.** A device report now includes the exact bytes sent and read during the try, the detected device type, which vendor app was open, and the other plugged-in devices of the same brand (their USB id and catalogue name only). Still nothing personal.

Windows: download `juicetray-windows-x86_64.exe` below, or use **Update now** in the app. macOS: use **Update now** in the app, or `brew upgrade juicetray`, then open `$(brew --prefix)/opt/juicetray/Juicetray.app` and choose **Move** (a zip downloaded in a browser is blocked by Gatekeeper; use Homebrew).
