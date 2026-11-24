//! HID and serial transports, and the only two ways to open a `Target`.

use std::ffi::CString;
use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use hidapi::HidDevice;
use protocol::{Family, Link};
use serialport::{ClearBuffer, DataBits, FlowControl, Parity, SerialPort, StopBits};

use crate::enumerate::{Access, HidCollection, Inventory, SerialEntry};

/// Byte channel to one opened device. Only `write.rs` calls `write`.
pub(crate) trait Transport {
    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()>;
    /// Bytes read before `timeout`; 0 means nothing arrived in time.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> anyhow::Result<usize>;
}

/// An opened device. `vid`/`pid` always come from the inventory entry it was opened from.
pub struct Target {
    pub(crate) vid: u16,
    pub(crate) pid: u16,
    pub(crate) link: Link,
    pub(crate) transport: Box<dyn Transport>,
}

const SERIAL_BAUD: u32 = 9600;

impl Inventory {
    /// Opens a HID collection that this inventory itself opened during enumeration (rule 6).
    /// The checks run before any I/O; the caller's copy of the entry decides nothing.
    pub fn open_hid(&self, collection: &HidCollection) -> anyhow::Result<Target> {
        let entry = self
            .hid
            .iter()
            .find(|entry| {
                entry.path == collection.path
                    && (entry.vid, entry.pid) == (collection.vid, collection.pid)
                    && (entry.shape.usage_page, entry.shape.usage)
                        == (collection.shape.usage_page, collection.shape.usage)
            })
            .ok_or_else(|| anyhow!("collection is not in this inventory"))?;
        if !matches!(entry.access, Access::Opened { .. }) {
            bail!("collection was not opened during enumeration (rule 6)");
        }
        // Errors name VID:PID and page:usage only: the raw path holds the device instance id (R10).
        let label = format!(
            "HID {:04X}:{:04X} {:04X}:{:04X}",
            entry.vid, entry.pid, entry.shape.usage_page, entry.shape.usage
        );
        let api = self.api.as_ref().context("HID is not available")?;
        let path = CString::new(entry.path.as_str())
            .with_context(|| format!("{label}: path contains a NUL byte"))?;
        let device = api
            .open_path(&path)
            .with_context(|| format!("could not open {label}"))?;
        Ok(Target {
            vid: entry.vid,
            pid: entry.pid,
            link: Link::Hid,
            transport: Box::new(HidTransport(device)),
        })
    }

    /// Opens a serial port of this inventory at 9600 8N1, no flow control, then sets DTR on
    /// (serialport's own DTR-on-open is best effort and Windows drives DTR low on open).
    pub fn open_serial(&self, entry: &SerialEntry) -> anyhow::Result<Target> {
        let entry = self
            .serial
            .iter()
            .find(|own| {
                own.port_name == entry.port_name && (own.vid, own.pid) == (entry.vid, entry.pid)
            })
            .ok_or_else(|| anyhow!("port is not in this inventory"))?;
        // Errors name VID:PID only: a macOS port name can contain the USB serial number (R10).
        let label = format!("serial port {:04X}:{:04X}", entry.vid, entry.pid);
        let mut port = serialport::new(&entry.port_name, SERIAL_BAUD)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .timeout(Family::AtkSerial.reply_timeout())
            .open()
            .with_context(|| {
                format!("could not open {label}: busy or unavailable (is ATK HUB open?)")
            })?;
        port.write_data_terminal_ready(true)
            .with_context(|| format!("could not set DTR on {label}"))?;
        // Host-side purge, nothing is sent: Windows keeps stale input on open, which would
        // shift the 64-byte frames.
        port.clear(ClearBuffer::Input)
            .with_context(|| format!("could not clear stale input on {label}"))?;
        Ok(Target {
            vid: entry.vid,
            pid: entry.pid,
            link: Link::Serial,
            transport: Box::new(SerialTransport(port)),
        })
    }
}

struct HidTransport(HidDevice);

impl Transport for HidTransport {
    /// Any `Ok` counts as sent: hidapi `windows-native` returns `Ok(0)` when `WriteFile`
    /// completes at once, so a length check would fail a query that went out.
    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.0.write(bytes)?;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> anyhow::Result<usize> {
        let millis = i32::try_from(timeout.as_millis())
            .unwrap_or(i32::MAX)
            .max(1);
        Ok(self.0.read_timeout(buf, millis)?)
    }
}

struct SerialTransport(Box<dyn SerialPort>);

impl Transport for SerialTransport {
    /// Bounded by the write timeout `open_serial` set; no flush.
    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.0.write_all(bytes)?;
        Ok(())
    }

    /// A timed-out read is "nothing arrived" (`TimedOut` on both Windows and POSIX).
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> anyhow::Result<usize> {
        self.0.set_timeout(timeout)?;
        match self.0.read(buf) {
            Ok(len) => Ok(len),
            Err(error) if error.kind() == ErrorKind::TimedOut => Ok(0),
            Err(error) => Err(error.into()),
        }
    }
}
