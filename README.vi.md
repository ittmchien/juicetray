# Juicetray

[English](README.md) · **Tiếng Việt**

Xem mức pin chuột, tai nghe không dây ngay cạnh đồng hồ trên taskbar, không cần cài phần mềm
của từng hãng.

> **Trạng thái:** beta. Thiết bị Bluetooth và các receiver đã xác nhận bên dưới chạy được trên
> Windows; bản macOS được CI build.

## Thiết bị hỗ trợ

**Bluetooth: mọi thiết bị.** Bất kỳ thiết bị Bluetooth đang kết nối nào mà hệ điều hành đã đọc
được mức pin (tai nghe, chuột, bàn phím, tay cầm, …), hãng nào cũng được. Juicetray chỉ đọc
giá trị hệ điều hành đã lưu, không gửi gì tới thiết bị.

**Receiver 2.4 GHz / cáp USB** — đọc thẳng từ receiver, không cần app của hãng.

### Đã kiểm trên thiết bị thật

Đọc đúng mức pin trên thiết bị của chủ dự án; các thiết bị này được đọc tự động.

**Tai nghe**

| Thiết bị | Receiver | Ghi chú |
|---|---|---|
| MCHOSE V9 Pro 2 | `3837:6011` | |
| ATK N9 Ultra | `373B:1204` | Đóng ATK HUB trước: nó giữ cổng của receiver |

**Chuột**

| Thiết bị | Receiver | Ghi chú |
|---|---|---|
| ATK X1 SE | `373B:1216` | |

### Đã đọc được một lần, chưa đối chiếu với app của hãng

**Chuột**

| Thiết bị | Receiver | Ghi chú |
|---|---|---|
| VGN Dragonfly 3 Master+ | `391D:1A05` | Chỉ trả lời khi chuột đang thức |

**Bàn phím**

| Thiết bị | Receiver | Ghi chú |
|---|---|---|
| AULA F75 | `3554:FA09` | Chỉ khi dùng 2.4 GHz; cắm cáp thì bàn phím không báo mức pin |

### Thiết bị khác

Nhiều chuột ATK, VXE và VGN dùng chung giao thức. Khi một model chưa được xác minh, Juicetray
không bao giờ tự hỏi pin nó. Muốn thử, dùng **Add unsupported device…** trong menu (có ở bản
phát hành tới): app nhận ra thiết bị khi bạn cắm vào, gửi một lệnh hỏi pin chỉ-đọc, và thêm
thiết bị khi bạn xác nhận mức pin đúng. Nếu không được, vui lòng
[mở issue](https://github.com/ittmchien/juicetray/issues) — cửa sổ sẽ điền sẵn thông tin.

`allow` trong file config vẫn dùng được cho thiết bị thêm tay:

```toml
allow = ["391D:1A05"]
```

## Nền tảng

- Windows 10/11 (x86_64)
- macOS (Apple silicon và Intel)

## An toàn

Juicetray chỉ gửi lệnh hỏi pin và hỏi trạng thái kết nối dạng chỉ-đọc, mỗi lần một lệnh, không
lặp lại khi lỗi. App không bao giờ đổi cài đặt của thiết bị. Mức pin Bluetooth được đọc từ dữ
liệu hệ điều hành đã lưu, nên không có lệnh nào tới các thiết bị đó. Thiết bị qua receiver hoặc
cáp được hỏi tối đa 10 giây một lần (chuột) hoặc 5 phút một lần (tai nghe), bất kể cài đặt
auto-refresh.

## Chẩn đoán

- `juicetray.log` nằm cạnh `query-log.toml`. Bản release chỉ ghi khi có sự cố (panic, treo,
  lỗi); chạy với `JUICETRAY_TRACE=1` để ghi từng bước (bản debug luôn ghi).
- `juicetray scan` liệt kê thiết bị Bluetooth, HID và cổng serial, không ghi gì, và che số
  serial.
- `juicetray probe <VID:PID>` (không bắt buộc; mục Check battery now trong menu xác nhận thiết
  bị mà không cần lệnh này) gửi một lệnh hỏi pin chỉ-đọc tới thiết bị được hỗ trợ và in phản hồi
  thô. Đóng app của hãng trước; probe từ chối nếu chưa hết khoảng chờ của thiết bị.
- `juicetray probe --online <VID:PID>` gửi lệnh hỏi trạng thái kết nối chỉ-đọc (chỉ Compx và
  ATK N9 Ultra, cùng khoảng chờ) và in `online: yes/no/unsupported`.
- `juicetray listen <VID:PID> [seconds]` chỉ đọc (1-60 giây, mặc định 10) và in từng report kèm
  thời điểm tính bằng ms, để xem app của hãng nhận về gì; không gửi gì.

## Đóng góp

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
`family` là một trong `Compx`, `Report13`, `Mchose`, `AtkSerial` hoặc `AtkBitmouse`; model cần
giao thức khác thì phải thêm family vào code trước. Bước build kiểm từng dòng (family hoặc kind
lạ, hex sai, VID:PID trùng) và báo lỗi kèm tên file và vị trí dòng. Mở pull request vào `dev`
với `verified = false`; model chỉ được đổi thành `verified = true` sau khi kiểm trên thiết bị
thật (`juicetray probe <VID:PID>` đọc ra cùng mức pin với app của hãng).

## Giấy phép

[GPL-3.0-or-later](LICENSE). Bản exe Windows in toàn bộ giấy phép đi kèm bằng
`juicetray licenses`; app macOS lưu chúng trong `Contents/Resources/licenses/`.

## Ghi công

Ghi chú giao thức từ M-HUB-linux-port (MCHOSE V9 Pro 2), danh mục thiết bị OpenMouse (ATK),
driver Linux `hid-kysona`, [Joule](https://github.com/DillanCidral/Joule) và
[sanshoku](https://github.com/ushineko/sanshoku). Chỉ dùng thông tin giao thức; không chép
code.

Icon thiết bị: [Material Symbols](https://github.com/google/material-design-icons) của Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Chữ số trên macOS (font dự phòng;
Windows dùng font hệ thống): [Inter](https://rsms.me/inter/)
([OFL-1.1](crates/strip-render/assets/OFL.txt)).
