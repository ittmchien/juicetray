# Adding a device

Back to the [README](../README.md).

## Trying a device (Windows)

**Windows only.** Many ATK, VXE and VGN mice use the same protocols. Until a model is verified,
Juicetray never queries it on its own. To try one, use **Add device…** in the menu, or **OK** on
the "New device found" popup (shown when you plug in a supported brand or any wireless receiver
while Juicetray runs, never at start-up, and only once per device whatever you answer): it
detects the device, sends one read-only battery query when you press **Try**, and adds it once
you confirm the level. If it does not work, press **Send report** to send the details through a
[Google Form](https://forms.gle/Qp5CeBGEiPg2UtkX6) (no account needed), or **Report on GitHub**
to [open an issue](https://github.com/ittmchien/juicetray/issues) with them filled in. From the
moment such a device is plugged in (or while the Add device window is detecting), Juicetray listens
to it for 30 s, read-only (nothing is sent), and the report includes what the device sent on its
own, often its level or link state. Do not type during those 30 s: for a device with a keyboard or
keypad interface (most receivers have one), reports sent within half a second of a key press on any
keyboard are dropped (only counted), so typed keys stay out of the report (keys typed into an
administrator window or on the sign-in screen may not be seen, so do not type there either). A
report button pressed before the end waits for it (**Stop** ends it early). Nothing is listened to
while the brand's own app is open. Only the device's name, USB ID and interfaces, the bytes sent and
read during the try, the reports the device sent by itself during that listen, the names of running
vendor apps and the USB IDs of other devices of the same brand are sent — nothing personal.

When the plugged-in device is not in the catalogue your version was built with, the popup first
looks for it in the catalogue files on GitHub (see [Updates](updates.md)). If a row there matches and its
family works for an added device, the popup shows that row's name and **OK** adds the device at
once, without the Add device window; otherwise **OK** opens the window as above. A device you
were asked about before updating is not offered again: add it with **Add device…**.

Hand-added devices can also go in `allow` in the config file:

```toml
allow = ["391D:1A05"]
```

## Add a model to the catalogue

A model is one line in its brand's file under `crates/protocol/devices/` (for example
`atk.toml`), with `vid` and `pid` as 4 hex digits, `name`, `family`, `kind` (`mouse`, `headset`,
`keyboard`, `gamepad` or `other`), `connection` (`receiver` or `cable`) and `verified`. `family`
is one of `Compx`, `Report13`, `Mchose`, `AtkSerial`, `AtkBitmouse`, `VgnKc2`, `RongYuanF7`,
`Driveall`, `Xctech` or `Generic`; a model whose protocol none of them covers can often be a `Generic`
row (below), otherwise it needs a new family in code first. The build checks every line (unknown
family or kind, bad hex, duplicate VID:PID) and fails with the file and entry named. Open the pull
request against `dev` with `verified = false`; a model becomes `verified = true` only after a
hardware check (`juicetray probe <VID:PID>` read the same level as the vendor app).

Released apps fetch only the catalogue files they were built with when they look up an unknown
device, so put a new brand on an existing family VID into that VID's existing file; a new file
reaches only later releases.

## Generic rows

A `Generic` row carries its own query instead of family code. Its extra fields are all required
(except the last three) and are refused on any other family:

- `usage_page`, `usage`: the vendor collection, 4 hex digits (`FF00`–`FFFF` only; page `008C`
  and system collections are refused).
- `report`: `output` (answered by an input report) or `feature` (SetFeature, then GetFeature of
  the same id); `report_bytes`: that report's size exactly as `juicetray scan` prints it
  (`out 05:33` → 33).
- `request`: the one frame sent, hex, report id first, at most 64 bytes and no longer than the
  report; it is zero-padded to the report.
- `reply_prefix`: hex bytes a valid reply starts with, report id first (an output row needs a
  byte after the id); other replies are ignored.
- `level_byte`: index of the level in the reply, report id = byte 0 (also when the id is 0).
- optional `charging_byte` + `charging_mask` (together; masked nonzero = charging) and `scale`
  (default 100; percent = raw × 100 / scale, anything over 100 is rejected).

```toml
{ vid = "1234", pid = "5678", name = "Example 2.4G dongle", family = "Generic", kind = "mouse", connection = "receiver", verified = false, usage_page = "FF00", usage = "0001", report = "output", report_bytes = 33, request = "05 01", reply_prefix = "05 81", level_byte = 3, charging_byte = 4, charging_mask = 0x01 }
```

The example is made up: it sends output report `05 01` (padded to 33 bytes) once per query and
reads byte 3 of an input report starting `05 81` as the percent. Like ATK serial, RongYuan F7,
Driveall and Xctech, a Generic row is used only for its own catalogue VID:PID; a custom device or
the Add device window never gets it. One fixed frame per query, no retries, 10 s query floor.

## From a device report to a row

 The Add device report lists every collection of the device
as `scan` prints it: `HID if 2 FF00:0001 vendor · Usb · in 05:33 out 05:33` gives `usage_page`,
`usage`, `report` and `report_bytes`, and the descriptor hex below it lets a reviewer check the
sizes. On Windows each line except page 008C ends with `caps in N out N feat N` (or
`caps unknown (…)` when Windows cannot read them), the longest report of each kind as Windows
reports it (id byte included, read without sending anything), so with several report ids it is
not a valid `report_bytes`. The request and the reply layout come from what the brand's app sends
and receives (a USB capture); `level_byte` is the reply byte that equals the percent the brand's
app shows. Pick a request the brand's app sends only to read status, never a setter. The
`Listen:` lines show the input reports the device sent by itself after plug-in (time, repeat
count, first 32 bytes): a byte that tracks the level there often gives `level_byte` without a
capture.
