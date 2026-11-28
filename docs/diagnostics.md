# Diagnostics

Back to the [README](../README.md).

| Command | What it does |
|---|---|
| `juicetray scan` | Lists Bluetooth, HID and serial devices; writes nothing; redacts serial numbers. |
| `juicetray probe <VID:PID>` | Sends one read-only battery query to a supported device and prints the raw reply. Close the vendor app first; refuses within the device's floor. Optional: **Add device…** in the menu tries a device without it. Receiver tool (Windows). |
| `juicetray probe --online <VID:PID>` | Sends the read-only online-status query instead (Compx and ATK N9 Ultra, same floor) and prints `online: yes/no/unsupported`. Receiver tool (Windows). |
| `juicetray listen <VID:PID> [seconds]` | Only reads (1–60 s, default 10) and prints each report with its time in ms; nothing is sent. Receiver tool (Windows). |

On macOS there is no `juicetray` on PATH: run
`/Applications/Juicetray.app/Contents/MacOS/juicetray scan` (or `… licenses`).

**Help → Open log folder** opens it. `juicetray.log` sits next to `query-log.toml` in `%LOCALAPPDATA%\ittmchien\juicetray\data`
(macOS: `~/Library/Application Support/io.github.ittmchien.juicetray/`). Dev and beta builds
trace every step; an official release writes only when something goes wrong (panic, hang dump
on Windows, error) unless started with `JUICETRAY_TRACE=1`.
