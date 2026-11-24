<div align="center">

<img src="docs/assets/icon.png" width="72" alt="Biểu tượng Juicetray">

# Juicetray

**Xem mức pin chuột, tai nghe, bàn phím không dây ngay cạnh đồng hồ trên taskbar — không cần
cài phần mềm của từng hãng.**

[![Release](https://img.shields.io/github/v/release/ittmchien/juicetray?include_prereleases&label=release)](https://github.com/ittmchien/juicetray/releases)
[![CI](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6?logo=windows)](#-nền-tảng)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

[English](README.md) · **Tiếng Việt**

</div>

> [!NOTE]
> **Bản beta, hiện chỉ có Windows.** Thiết bị Bluetooth và các receiver đã xác minh bên dưới
> chạy được trên Windows; bản macOS sẽ phát hành sau khi kiểm trên máy Mac.

## ✨ Tính năng

- 🔋 **Pin ngay trên taskbar** — mỗi thiết bị một icon nhỏ kèm thanh pin và %, hiện trên
  taskbar của mọi màn hình; rê chuột để xem tóm tắt, chuột phải để mở menu.
- 🎨 **Màu theo trạng thái** — thanh pin xanh lá khi đang sạc, vàng khi dưới 50 %, đỏ khi tới
  ngưỡng pin yếu bạn đặt; thiết bị pin yếu còn hiện chữ *needs charge* trong menu.
- 🖱️ **Receiver 2.4 GHz và cáp** — đọc thẳng từ receiver; không cần chạy app của hãng.
- 📶 **Mọi thiết bị Bluetooth** — tai nghe, chuột, bàn phím, tay cầm… miễn Windows đã biết mức pin.
- 🗂️ **Nhóm và sắp xếp** — chia nhóm theo loại, pin cao đứng trước, thiết bị offline xuống cuối và
  có dấu ⊘; tự ẩn thiết bị offline sau khoảng thời gian bạn chọn.
- 🔔 **Cảnh báo pin yếu** — thông báo khi chạm ngưỡng bạn chọn (5–30 % hoặc tự nhập).
- 🛠️ **Tuỳ chỉnh từng thiết bị** — hiện/ẩn, phần trăm, nhãn, icon, màu icon cho taskbar sáng và tối.
- 🪶 **Siêu nhẹ** — khoảng 10 MB RAM, ~0,05 % CPU lúc rảnh, file exe portable 2 MB; tự dừng
  auto-refresh khi laptop chạy pin, trừ khi bạn bật lại.
- 🔄 **Cập nhật một chạm** — kiểm tra GitHub Releases mỗi ngày; **Update now** kiểm SHA-256 của
  file tải về rồi mới thay exe.
- 🛡️ **Chỉ đọc, nhẹ nhàng** — không bao giờ đổi cài đặt thiết bị; xem phần An toàn bên dưới.

## ⬇️ Cài đặt

1. Tải `juicetray-windows-x86_64.exe` từ
   [bản phát hành mới nhất](https://github.com/ittmchien/juicetray/releases/latest).
2. Đặt vào một thư mục ghi được (ví dụ `%LOCALAPPDATA%\Programs\Juicetray`) rồi chạy. App là
   bản portable: không cần cài, không cần thiết lập gì thêm.
3. Chuột phải vào dải icon hoặc icon khay → **Settings → Start at login** để chạy cùng Windows.

Cài đặt lưu ở `%APPDATA%\ittmchien\juicetray\config\config.toml`, nên thay file exe không mất
cài đặt. Windows SmartScreen có thể cảnh báo app chưa ký số: bấm **More info → Run anyway**.

## 🎧 Thiết bị hỗ trợ

**📶 Bluetooth — mọi thiết bị.** Bất kỳ thiết bị Bluetooth đang kết nối nào mà Windows đã đọc
được mức pin, hãng nào cũng được. Juicetray chỉ đọc giá trị Windows đã lưu, không gửi gì tới
thiết bị.

**🔌 Receiver 2.4 GHz / cáp USB** — đọc thẳng từ receiver.

### ✅ Đã kiểm trên thiết bị thật

Đọc đúng mức pin trên thiết bị của chủ dự án; các thiết bị này được đọc tự động.

| | Thiết bị | Receiver | Ghi chú |
|---|---|---|---|
| 🎧 | MCHOSE V9 Pro 2 | `3837:6011` | |
| 🎧 | ATK N9 Ultra | `373B:1204` | Đóng ATK HUB trước: nó giữ cổng của receiver |
| 🖱️ | ATK X1 SE | `373B:1216` | |

### 🧪 Đã đọc được một lần, chưa đối chiếu với app của hãng

| | Thiết bị | Receiver | Ghi chú |
|---|---|---|---|
| 🖱️ | VGN Dragonfly 3 Master+ | `391D:1A05` | Chỉ trả lời khi chuột đang thức |
| ⌨️ | AULA F75 | `3554:FA09` | Chỉ khi dùng 2.4 GHz; cắm cáp thì bàn phím không báo mức pin |

### ➕ Thiết bị khác

Nhiều chuột ATK, VXE và VGN dùng chung giao thức. Khi một model chưa được xác minh, Juicetray
không bao giờ tự hỏi pin nó. Muốn thử, dùng **Add unsupported device…** trong menu (Windows),
hoặc bấm **OK** trên popup "New device found" (hiện với các hãng được hỗ trợ và với mọi
receiver không dây bạn cắm vào): app nhận ra thiết bị khi bạn cắm vào, gửi một
lệnh hỏi pin chỉ-đọc khi bạn bấm **Try**, và thêm thiết bị khi bạn xác nhận mức pin đúng. Nếu
không được, bấm **Report on GitHub** để
[mở issue](https://github.com/ittmchien/juicetray/issues) với thông tin điền sẵn (không kèm thư
mục người dùng, tên máy hay số serial của thiết bị).

`allow` trong file config vẫn dùng được cho thiết bị thêm tay:

```toml
allow = ["391D:1A05"]
```

## 💻 Nền tảng

| Nền tảng | Trạng thái |
|---|---|
| 🪟 Windows 10 / 11 (x86_64) | ✅ Đã phát hành |
| 🍎 macOS (Apple silicon và Intel) | 🚧 CI có build, chưa phát hành |

## 🔄 Cập nhật

Juicetray kiểm tra [GitHub Releases](https://github.com/ittmchien/juicetray/releases) mỗi ngày
tối đa một lần (mở lại app cũng không kiểm thêm). Tắt bằng mục **Check for updates automatically** trong
**Settings**, hoặc kiểm tay bằng **Check for updates…**. Yêu cầu chỉ gửi số phiên bản trong User-Agent
(`juicetray/<version>`).

**Update now** tải exe mới, kiểm SHA-256 với `SHA256SUMS.txt` của bản phát hành, thay file rồi
khởi động lại; lỗi ở bất kỳ bước nào thì exe cũ được giữ nguyên. Nếu thư mục chứa exe không ghi
được thì app mở trang tải về thay vào đó.

## 🛡️ An toàn

- Chỉ gửi lệnh hỏi pin và hỏi trạng thái kết nối dạng chỉ-đọc, mỗi lần một lệnh, không lặp lại
  khi lỗi.
- Không bao giờ đổi cài đặt của thiết bị.
- Mức pin Bluetooth lấy từ dữ liệu Windows đã lưu, nên không có lệnh nào tới các thiết bị đó.
- Thiết bị qua receiver hoặc cáp được hỏi tối đa 10 giây một lần (chuột) hoặc 5 phút một lần
  (tai nghe), bất kể cài đặt auto-refresh.

## 🩺 Chẩn đoán

| Lệnh | Tác dụng |
|---|---|
| `juicetray scan` | Liệt kê thiết bị Bluetooth, HID và cổng serial; không ghi gì; che số serial. |
| `juicetray probe <VID:PID>` | Gửi một lệnh hỏi pin chỉ-đọc tới thiết bị được hỗ trợ và in phản hồi thô. Đóng app của hãng trước; từ chối nếu chưa hết khoảng chờ của thiết bị. Không bắt buộc: mục **Add unsupported device…** trong menu thử thiết bị mà không cần lệnh này. |
| `juicetray probe --online <VID:PID>` | Gửi lệnh hỏi trạng thái kết nối chỉ-đọc (Compx và ATK N9 Ultra, cùng khoảng chờ) và in `online: yes/no/unsupported`. |
| `juicetray listen <VID:PID> [seconds]` | Chỉ đọc (1–60 giây, mặc định 10) và in từng report kèm thời điểm tính bằng ms; không gửi gì. |

`juicetray.log` nằm cạnh `query-log.toml` trong `%LOCALAPPDATA%\ittmchien\juicetray\data`. Bản
release chỉ ghi khi có sự cố (panic, treo, lỗi); đặt `JUICETRAY_TRACE=1` để ghi từng bước (bản
debug luôn ghi).

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
`family` là một trong `Compx`, `Report13`, `Mchose`, `AtkSerial`, `AtkBitmouse`, `VgnKc2` hoặc
`RongYuanF7`; model cần
giao thức khác thì phải thêm family vào code trước. Bước build kiểm từng dòng (family hoặc kind
lạ, hex sai, VID:PID trùng) và báo lỗi kèm tên file và vị trí dòng. Mở pull request vào `dev`
với `verified = false`; model chỉ được đổi thành `verified = true` sau khi kiểm trên thiết bị
thật (`juicetray probe <VID:PID>` đọc ra cùng mức pin với app của hãng).

## 📜 Giấy phép

[GPL-3.0-or-later](LICENSE). Bản exe Windows in toàn bộ giấy phép đi kèm bằng
`juicetray licenses`; app macOS lưu chúng trong `Contents/Resources/licenses/`.

## 🙏 Ghi công

Ghi chú giao thức từ M-HUB-linux-port (MCHOSE V9 Pro 2), danh mục thiết bị OpenMouse (ATK),
driver Linux `hid-kysona`, [Joule](https://github.com/DillanCidral/Joule) và
[sanshoku](https://github.com/ushineko/sanshoku). Chỉ dùng thông tin giao thức; không chép code.

Icon thiết bị: [Material Symbols](https://github.com/google/material-design-icons) của Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Chữ số trên macOS (font dự phòng;
Windows dùng font hệ thống): [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
