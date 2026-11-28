<div align="center">

<img src="docs/assets/icon.png" width="72" alt="Biểu tượng Juicetray">

# Juicetray

**Đừng để chuột, tai nghe cạn pin giữa trận.** 🧃

Juicetray hiện mức pin chuột, tai nghe, bàn phím không dây trên khay hệ thống / thanh menu — ngay
cạnh đồng hồ, không cần cài phần mềm của từng hãng.

[![Release](https://img.shields.io/github/v/release/ittmchien/juicetray?include_prereleases&label=release)](https://github.com/ittmchien/juicetray/releases)
[![CI](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/ittmchien/juicetray/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011%20%7C%20macOS%2013%2B-0078D6?logo=windows)](#%EF%B8%8F-cài-đặt)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

[English](README.md) · **Tiếng Việt**

</div>

> [!NOTE]
> **Bản beta.** Windows 10 / 11 đọc được thiết bị Bluetooth, receiver 2.4 GHz và cáp. macOS 13+
> (Homebrew) hiện chỉ đọc thiết bị Bluetooth.

## ✨ Tính năng

- 🔋 **Pin ngay cạnh đồng hồ** — mỗi thiết bị một icon trên mọi taskbar (Windows), hoặc một mục
  trên thanh menu (macOS).
- 🎨 **Màu theo trạng thái** — xanh lá khi sạc, vàng dưới 50 %, đỏ khi tới ngưỡng pin yếu.
- 🖱️ **Receiver 2.4 GHz và cáp** (Windows) — đọc thẳng từ receiver, không cần app của hãng.
- 📶 **Mọi thiết bị Bluetooth** mà Windows hoặc macOS báo được mức pin.
- 🔔 **Cảnh báo pin yếu** theo ngưỡng bạn chọn.
- 🛠️ **Tuỳ chỉnh từng thiết bị** — ẩn thiết bị, phần trăm hoặc tên; chọn icon và màu.
- 🪶 **Siêu nhẹ** — khoảng 10 MB RAM, ~0,05 % CPU lúc rảnh, file exe portable 2 MB.
- 🔄 **Cập nhật một chạm** — kiểm mỗi lần khởi động, xác thực trước khi cài; bấm **Later** thì
  không hỏi lại trong ngày.

## ⬇️ Cài đặt

### 🪟 Windows

1. Tải `juicetray-windows-x86_64.exe` từ
   [bản phát hành mới nhất](https://github.com/ittmchien/juicetray/releases/latest).
2. Đặt vào một thư mục ghi được (ví dụ `%LOCALAPPDATA%\Programs\Juicetray`) rồi chạy. Không cần
   cài.
3. Chuột phải icon khay → **Settings → Start at login** để chạy cùng Windows.

SmartScreen có thể cảnh báo app chưa ký số: bấm **More info → Run anyway**.

### <img src="https://cdn.simpleicons.org/apple/8b949e" height="16" alt=""> macOS (beta, Homebrew)

```sh
brew tap ittmchien/tap
brew trust ittmchien/tap   # cần trên Homebrew 7+
brew install juicetray
open "$(brew --prefix)/opt/juicetray/Juicetray.app"
```

Chọn **Move** khi được hỏi chép app vào Applications, và cho phép Bluetooth.

## 🎧 Thiết bị hỗ trợ

📋 **[Danh sách thiết bị](https://docs.google.com/spreadsheets/d/e/2PACX-1vRTKUOHRmpLeXhjxL93T5HwuYsJKnzMHSUhN2AWslNCB938EpQH2YkMeNubTcpvdxpHC2o7Dw4OHfS7/pubhtml)**
— mọi model được hỗ trợ và các thiết bị người dùng đã yêu cầu. Trong app: **Help → Supported
devices**.

- **📶 Bluetooth:** mọi thiết bị mà hệ điều hành báo được mức pin.
- **🔌 Receiver 2.4 GHz và cáp** (Windows): các model đã xác minh trong danh sách.

Thiết bị chưa có? Dùng **Add device…** trong menu để thử, hoặc gửi báo cáo từ đó để được thêm vào.
Xem [Adding a device](docs/adding-devices.md).

## ⚠️ Lỗi đã biết

- **"Windows protected your PC"** — exe chưa được ký số. Cài bằng winget (gói đang chờ duyệt) sẽ
  không bị cảnh báo này.
- Một số receiver chỉ trả lời khi thiết bị đang thức (VGN Dragonfly 3 Master+), và một số bàn phím
  không báo mức pin khi cắm cáp (AULA F75).

## 🛡️ An toàn

- Chỉ đọc: chỉ hỏi pin và trạng thái kết nối, mỗi lần một lệnh, không lặp lại. Không bao giờ đổi
  cài đặt thiết bị.
- Receiver và cáp được hỏi tối đa 10 giây một lần (chuột) hoặc 5 phút một lần (tai nghe).
- Khi app của một hãng đang mở (MCHOSE HUB, ATK HUB, VGN VHUB, M500), thiết bị của hãng đó được để
  yên và menu hiện mức pin cuối kèm `paused (… open)`.

## 📚 Tài liệu thêm (tiếng Anh)

- [Install and settings](docs/install.md) — file cài đặt, chi tiết macOS, chi tiết từng tính năng.
- [Updates](docs/updates.md) — cách kiểm tra cập nhật và **Update now** hoạt động, gửi những gì.
- [Diagnostics](docs/diagnostics.md) — file log (**Help → Open log folder**) và các lệnh `scan`,
  `probe`, `listen`.
- [Adding a device](docs/adding-devices.md) — thử thiết bị, báo cáo, và dòng danh mục.

## 🤝 Đóng góp

`main` chỉ chứa code đã phát hành; mở pull request vào `dev`
(`git clone -b dev https://github.com/ittmchien/juicetray`). Khi fork, bỏ chọn **"Copy the `main`
branch only"**.

## 📜 Giấy phép

[GPL-3.0-or-later](LICENSE). Giấy phép đi kèm: `juicetray licenses` (Windows) hoặc
`Contents/Resources/licenses/` (macOS).

## 🙏 Ghi công

Icon thiết bị: [Material Symbols](https://github.com/google/material-design-icons) của Google
([Apache-2.0](crates/strip-render/assets/icons/LICENSE)). Font dự phòng cho chữ số:
[Inter](https://rsms.me/inter/) ([OFL-1.1](crates/strip-render/assets/OFL.txt)).
