<div align="center">

<img src="docs/assets/icon.png" width="72" alt="Biểu tượng Juicetray">

# Juicetray

**Đừng để chuột, tai nghe cạn pin giữa trận.** 🧃

Juicetray hiện mức pin chuột, tai nghe, bàn phím không dây trên khay hệ thống / thanh menu — ngay
cạnh đồng hồ, không cần cài phần mềm của từng hãng.

[![Release](https://img.shields.io/github/v/release/ittmchien/juicetray?include_prereleases&label=release)](https://github.com/ittmchien/juicetray/releases)
[![CI](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011%20%7C%20macOS%2013%2B-0078D6?logo=windows)](#-nền-tảng)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

[English](README.md) · **Tiếng Việt**

</div>

> [!NOTE]
> **Bản beta.** Thiết bị Bluetooth và các receiver đã xác minh bên dưới chạy được trên Windows.
> **macOS hiện chỉ hỗ trợ thiết bị Bluetooth** (cài bằng Homebrew); receiver và cáp chỉ có trên
> Windows.

## ✨ Tính năng

- 🔋 **Pin ngay trên taskbar / thanh menu** — (Windows) mỗi thiết bị một icon nhỏ kèm thanh pin
  và %, hiện trên taskbar của mọi màn hình; rê chuột để xem tóm tắt, chuột phải để mở menu. Trên
  macOS, một mục trên thanh menu vẽ tất cả thiết bị; bấm vào để mở menu.
- 🎨 **Màu theo trạng thái** — thanh pin xanh lá khi đang sạc, vàng khi dưới 50 %, đỏ khi tới
  ngưỡng pin yếu bạn đặt; thiết bị pin yếu còn hiện chữ *needs charge* trong menu.
- 🖱️ **Receiver 2.4 GHz và cáp** (Windows) — đọc thẳng từ receiver; không cần chạy app của hãng.
- 📶 **Mọi thiết bị Bluetooth** — tai nghe, chuột, bàn phím, tay cầm… miễn Windows hoặc macOS báo
  được mức pin.
- 🗂️ **Nhóm và sắp xếp** — chia nhóm theo loại, pin cao đứng trước, thiết bị offline xuống cuối và
  ghi *disconnected*; tự ẩn thiết bị offline sau khoảng thời gian bạn chọn.
- 🔔 **Cảnh báo pin yếu** — thông báo khi chạm ngưỡng bạn chọn (5–30 % hoặc tự nhập).
- 🛠️ **Tuỳ chỉnh từng thiết bị** — bấm vào thiết bị trong menu: hiện/ẩn, phần trăm, tên dưới icon,
  icon (Generic, Headset, Mouse… hoặc SVG của bạn) và màu icon, kèm mục **Custom** để chọn màu
  chung hoặc riêng cho taskbar / thanh menu sáng và tối. Trên macOS, AirPods và tai Beats hiện đúng
  biểu tượng của Apple.
- 🪶 **Siêu nhẹ** — khoảng 10 MB RAM, ~0,05 % CPU lúc rảnh, file exe portable 2 MB trên Windows;
  tự dừng auto-refresh khi laptop chạy pin, trừ khi bạn bật **Refresh on battery power**.
- 🔄 **Cập nhật một chạm** — kiểm tra GitHub Releases mỗi ngày; **Update now** kiểm SHA-256 của
  file tải về rồi mới thay exe trên Windows, và chạy `brew upgrade` trên macOS.
- 🛡️ **Chỉ đọc, nhẹ nhàng** — không bao giờ đổi cài đặt thiết bị; xem phần An toàn bên dưới.

## ⬇️ Cài đặt

### 🪟 Windows

1. Tải `juicetray-windows-x86_64.exe` từ
   [bản phát hành mới nhất](https://github.com/ittmchien/juicetray/releases/latest).
2. Đặt vào một thư mục ghi được (ví dụ `%LOCALAPPDATA%\Programs\Juicetray`) rồi chạy. App là
   bản portable: không cần cài, không cần thiết lập gì thêm.
3. Chuột phải vào dải icon hoặc icon khay → **Settings → Start at login** để chạy cùng Windows.

Cài đặt lưu ở `%APPDATA%\ittmchien\juicetray\config\config.toml`, nên thay file exe không mất
cài đặt. Windows SmartScreen có thể cảnh báo app chưa ký số: bấm **More info → Run anyway**.

### <img src="https://cdn.simpleicons.org/apple/8b949e" height="16" alt=""> macOS (beta, Homebrew)

Homebrew cài app universal dựng sẵn từ bản phát hành trên GitHub: tải khoảng 3 MB, không cần bộ
công cụ Rust, xong trong vài giây. Homebrew tải bằng curl và không gắn cờ quarantine, nên
Gatekeeper không chặn:

```sh
brew tap ittmchien/tap
brew trust ittmchien/tap   # cần trên Homebrew 7+
brew install juicetray
open "$(brew --prefix)/opt/juicetray/Juicetray.app"
```

Juicetray hỏi có chuyển vào Applications không: **Move** chép app vào đó và khởi động lại từ bản
chép (bản của Homebrew vẫn còn, vì cập nhật cần đến nó); **Not now** được ghi nhớ và không hỏi lại.
Muốn chuyển sau thì tự chép `Juicetray.app` vào Applications. Cho phép Bluetooth khi được hỏi.
**Settings → Start at login** cũng dùng được (macOS có thể yêu cầu bạn duyệt trong System Settings
› General › Login Items). Cài đặt lưu ở
`~/Library/Application Support/io.github.ittmchien.juicetray/config.toml`.

Cập nhật: bấm **Update now** khi Juicetray báo có bản mới (xem phần Cập nhật). Cập nhật tay: thoát
Juicetray, chạy `brew upgrade juicetray`, mở lại app như trên rồi chọn **Move** để thay bản trong
Applications.

## 🎧 Thiết bị hỗ trợ

**📶 Bluetooth — mọi thiết bị.** Bất kỳ thiết bị Bluetooth đang kết nối nào mà Windows hoặc macOS
báo được mức pin, hãng nào cũng được. Juicetray chỉ đọc giá trị hệ điều hành báo, không gửi gì
tới thiết bị.

**🔌 Receiver 2.4 GHz / cáp USB** (chỉ Windows) — đọc thẳng từ receiver.

### ✅ Đã kiểm trên thiết bị thật

Đọc đúng mức pin trên thiết bị của chủ dự án; các thiết bị này được đọc tự động.

| | Thiết bị | Receiver | Ghi chú |
|---|---|---|---|
| 🎧 | MCHOSE V9 Pro 2 | `3837:6011` | |
| 🎧 | ATK N9 Ultra | `373B:1204` | Đóng ATK HUB trước: nó giữ cổng của receiver |
| 🖱️ | ATK X1 SE | `373B:1216` | |
| 🖱️ | ATK X1 SE Nearlink | `373B:1299` | Chuột cắm dây USB, không qua receiver |

### 🧪 Đã đọc được một lần, chưa đối chiếu với app của hãng

| | Thiết bị | Receiver | Ghi chú |
|---|---|---|---|
| 🖱️ | VGN Dragonfly 3 Master+ | `391D:1A05` | Chỉ trả lời khi chuột đang thức |
| ⌨️ | AULA F75 | `3554:FA09` | Chỉ khi dùng 2.4 GHz; cắm cáp thì bàn phím không báo mức pin |

### ➕ Thiết bị khác

**Chỉ Windows.** Nhiều chuột ATK, VXE và VGN dùng chung giao thức. Khi một model chưa được xác
minh, Juicetray không bao giờ tự hỏi pin nó. Muốn thử, dùng **Add device…** trong menu, hoặc bấm
**OK** trên popup "New device found" (hiện khi bạn cắm một hãng được hỗ trợ hoặc receiver không
dây bất kỳ lúc Juicetray đang chạy, không bao giờ hiện lúc khởi động, và chỉ một lần cho mỗi
thiết bị dù bạn trả lời gì): app nhận ra thiết bị, gửi một lệnh hỏi pin chỉ-đọc khi bạn bấm
**Try**, và thêm thiết bị khi bạn xác nhận mức pin đúng. Nếu không được, bấm **Send report** để
gửi thông tin qua [Google Form](https://forms.gle/Qp5CeBGEiPg2UtkX6) (không cần tài khoản),
hoặc **Report on GitHub** để [mở issue](https://github.com/ittmchien/juicetray/issues) với thông
tin điền sẵn. Chỉ gửi tên thiết bị, USB ID và danh sách interface, các byte đã gửi và đọc được
trong lần thử, tên app hãng đang chạy và USB ID của các thiết bị khác cùng hãng — không có thông
tin cá nhân.

📋 **[Danh sách thiết bị](https://docs.google.com/spreadsheets/d/e/2PACX-1vRTKUOHRmpLeXhjxL93T5HwuYsJKnzMHSUhN2AWslNCB938EpQH2YkMeNubTcpvdxpHC2o7Dw4OHfS7/pubhtml)**
— mọi model Juicetray biết (**Supported**) và các thiết bị người dùng đã yêu cầu kèm trạng thái
(**Requested**).

`allow` trong file config vẫn dùng được cho thiết bị thêm tay:

```toml
allow = ["391D:1A05"]
```

## 💻 Nền tảng

| Nền tảng | Trạng thái |
|---|---|
| 🪟 Windows 10 / 11 (x86_64) | ✅ Đã phát hành |
| <img src="https://cdn.simpleicons.org/apple/8b949e" height="16" alt=""> macOS 13+ (Apple silicon và Intel) | 🧪 Beta qua Homebrew — chỉ Bluetooth, chưa có receiver/cáp |

## 🔄 Cập nhật

Juicetray kiểm tra [GitHub Releases](https://github.com/ittmchien/juicetray/releases) mỗi ngày
tối đa một lần (mở lại app cũng không kiểm thêm). Tắt bằng mục **Check for updates
automatically** trong **Settings**, hoặc kiểm tay bằng **Check for updates…**. Yêu cầu chỉ gửi số
phiên bản trong User-Agent (`juicetray/<version>`).

Trên Windows, **Update now** tải exe mới, kiểm SHA-256 với `SHA256SUMS.txt` của bản phát hành,
thay file rồi khởi động lại; lỗi ở bất kỳ bước nào thì exe cũ được giữ nguyên. Nếu thư mục chứa
exe không ghi được thì app mở trang tải về thay vào đó.

Trên macOS, nếu Juicetray được cài bằng Homebrew, **Update now** chạy
`brew upgrade ittmchien/tap/juicetray`, thay `/Applications/Juicetray.app` bằng bản mới (chép ra
cạnh đó trước, nên chép lỗi thì app cũ vẫn nguyên) rồi khởi động lại. Nếu app đang chạy từ thư mục
của Homebrew thì khởi động lại từ đó và hỏi chuyển vào Applications. Lỗi ở bất kỳ bước nào thì một
hộp thoại báo lý do và bản đang chạy được giữ nguyên. Không cài bằng Homebrew thì app mở trang tải
về thay vào đó.

Trong lúc **Update now** chạy, một thông báo báo bắt đầu cập nhật và đầu menu hiện bước hiện tại
(*Downloading…* hoặc *Updating … via Homebrew…*, rồi *Installing…*, sau đó app tự khởi động lại); mục **Check for
updates…** bị làm mờ tới khi xong.

## ⚠️ Lỗi đã biết

- **"Windows protected your PC"** — exe chưa được ký số (xem phần Cài đặt). Cài bằng winget (gói
  đang chờ duyệt) sẽ không bị cảnh báo này.
- Một số receiver chỉ trả lời khi thiết bị đang thức (VGN Dragonfly 3 Master+), và một số bàn phím
  không báo mức pin khi cắm cáp (AULA F75).

## 🛡️ An toàn

- Chỉ gửi lệnh hỏi pin và hỏi trạng thái kết nối dạng chỉ-đọc, mỗi lần một lệnh, không lặp lại
  khi lỗi.
- Không bao giờ đổi cài đặt của thiết bị.
- Mức pin Bluetooth lấy từ giá trị Windows hoặc macOS báo, nên không có lệnh nào tới các thiết
  bị đó.
- Thiết bị qua receiver hoặc cáp được hỏi tối đa 10 giây một lần (chuột) hoặc 5 phút một lần
  (tai nghe), bất kể cài đặt auto-refresh.
- Khi app của một hãng đang mở (MCHOSE HUB, ATK HUB, VGN VHUB), Juicetray tạm dừng receiver và
  cáp của hãng đó: không mở, không gửi gì, menu giữ mức pin cuối kèm `paused (… open)`. Lần
  refresh đầu tiên sau khi app đóng sẽ đọc lại bình thường.

## 🩺 Chẩn đoán

| Lệnh | Tác dụng |
|---|---|
| `juicetray scan` | Liệt kê thiết bị Bluetooth, HID và cổng serial; không ghi gì; che số serial. |
| `juicetray probe <VID:PID>` | Gửi một lệnh hỏi pin chỉ-đọc tới thiết bị được hỗ trợ và in phản hồi thô. Đóng app của hãng trước; từ chối nếu chưa hết khoảng chờ của thiết bị. Không bắt buộc: mục **Add device…** trong menu thử thiết bị mà không cần lệnh này. Công cụ cho receiver (Windows). |
| `juicetray probe --online <VID:PID>` | Gửi lệnh hỏi trạng thái kết nối chỉ-đọc (Compx và ATK N9 Ultra, cùng khoảng chờ) và in `online: yes/no/unsupported`. Công cụ cho receiver (Windows). |
| `juicetray listen <VID:PID> [seconds]` | Chỉ đọc (1–60 giây, mặc định 10) và in từng report kèm thời điểm tính bằng ms; không gửi gì. Công cụ cho receiver (Windows). |

Trên macOS không có `juicetray` trong PATH: chạy
`/Applications/Juicetray.app/Contents/MacOS/juicetray scan` (hoặc `… licenses`).

`juicetray.log` nằm cạnh `query-log.toml` trong `%LOCALAPPDATA%\ittmchien\juicetray\data`
(macOS: `~/Library/Application Support/io.github.ittmchien.juicetray/`). Bản dev và beta ghi từng
bước; bản phát hành chính thức chỉ ghi khi có sự cố (panic, hang dump
trên Windows, lỗi), trừ khi chạy với
`JUICETRAY_TRACE=1`.

## 🤝 Đóng góp

`main` chỉ chứa code đã phát hành. Việc phát triển diễn ra trên `dev`:

```sh
git clone -b dev https://github.com/ittmchien/juicetray
```

- Mở pull request vào `dev`.
- Khi fork, bỏ chọn **"Copy the `main` branch only"** để fork có cả `dev`.

### Thêm một model thiết bị

Mỗi model là một dòng trong file của hãng đó dưới `crates/protocol/devices/` (ví dụ
`atk.toml`), gồm `vid` và `pid` (4 chữ số hex), `name`, `family`, `kind` (`mouse`, `headset`,
`keyboard`, `gamepad` hoặc `other`), `connection` (`receiver` hoặc `cable`) và `verified`.
`family` là một trong `Compx`, `Report13`, `Mchose`, `AtkSerial`, `AtkBitmouse`, `VgnKc2`,
`RongYuanF7`, `Driveall` hoặc `Generic`; model có giao thức không family nào ở trên hỗ trợ thường
có thể là một dòng `Generic` (bên dưới), nếu không thì phải thêm family vào code trước. Bước build
kiểm từng dòng (family hoặc kind lạ, hex sai, VID:PID trùng) và báo lỗi kèm tên file và vị trí
dòng. Mở pull request vào `dev` với `verified = false`; model chỉ được đổi thành
`verified = true` sau khi kiểm trên thiết bị thật (`juicetray probe <VID:PID>` đọc ra cùng mức pin
với app của hãng).

#### Dòng Generic

Một dòng `Generic` tự mang lệnh hỏi của nó thay vì code của family. Các trường thêm đều bắt buộc
(trừ ba trường cuối) và bị từ chối ở mọi family khác:

- `usage_page`, `usage`: collection riêng của hãng, 4 chữ số hex (chỉ `FF00`–`FFFF`; page `008C`
  và các collection hệ thống bị từ chối).
- `report`: `output` (trả lời bằng một input report) hoặc `feature` (SetFeature, rồi GetFeature
  cùng id); `report_bytes`: kích thước report đó, đúng như `juicetray scan` in ra
  (`out 05:33` → 33).
- `request`: frame duy nhất được gửi, dạng hex, report id đứng đầu, tối đa 64 byte và không dài
  hơn report; phần thiếu được đệm số 0 cho đủ report.
- `reply_prefix`: các byte hex mà phản hồi hợp lệ bắt đầu bằng, report id đứng đầu (dòng `output`
  cần ít nhất một byte sau id); phản hồi khác bị bỏ qua.
- `level_byte`: vị trí byte mức pin trong phản hồi, report id = byte 0 (kể cả khi id là 0).
- tuỳ chọn `charging_byte` + `charging_mask` (đi cùng nhau; kết quả AND khác 0 = đang sạc) và
  `scale` (mặc định 100; phần trăm = giá trị thô × 100 / scale, trên 100 bị loại).

```toml
{ vid = "1234", pid = "5678", name = "Example 2.4G dongle", family = "Generic", kind = "mouse", connection = "receiver", verified = false, usage_page = "FF00", usage = "0001", report = "output", report_bytes = 33, request = "05 01", reply_prefix = "05 81", level_byte = 3, charging_byte = 4, charging_mask = 0x01 }
```

Ví dụ trên là bịa: mỗi lần hỏi nó gửi output report `05 01` (đệm đủ 33 byte) một lần và đọc
byte 3 của input report bắt đầu bằng `05 81` làm phần trăm. Giống ATK serial, RongYuan F7 và
Driveall, dòng Generic chỉ dùng cho đúng VID:PID của nó trong catalogue; thiết bị custom hay cửa
sổ Add device không bao giờ dùng nó. Mỗi lần hỏi một frame cố định, không lặp lại, khoảng chờ
10 giây.

**Từ báo cáo thiết bị tới một dòng.** Báo cáo của Add device liệt kê mọi collection của thiết bị
như `scan` in ra: `HID if 2 FF00:0001 vendor · Usb · in 05:33 out 05:33` cho ra `usage_page`,
`usage`, `report` và `report_bytes`, còn đoạn hex descriptor bên dưới giúp người review kiểm lại
kích thước. Lệnh hỏi và cấu trúc phản hồi lấy từ những gì app của hãng gửi và nhận (bắt gói
USB); `level_byte` là byte phản hồi bằng đúng phần trăm app của hãng hiển thị. Chọn lệnh mà app
của hãng chỉ gửi để đọc trạng thái, không bao giờ là lệnh ghi cài đặt.

## 📜 Giấy phép

[GPL-3.0-or-later](LICENSE). Bản exe Windows in toàn bộ giấy phép đi kèm bằng
`juicetray licenses`; app macOS lưu chúng trong `Contents/Resources/licenses/`.

## 🙏 Ghi công

Icon thiết bị: [Material Symbols](https://github.com/google/material-design-icons) của Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Font dự phòng cho chữ số khi không
dùng được font hệ thống: [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
