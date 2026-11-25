//! Pure view model shared by the tray app: device data in, strip segments and menu text out.

mod caption;
mod device;
mod display;
mod strip;
mod text;

pub use caption::captions;
pub use device::{
    DeviceKind, DeviceView, Level, Path, UnknownDeviceKind, group_label, group_order,
    sort_by_level, sort_grouped,
};
pub use display::{DisplayOptions, IconChoice, IconColor};
pub use strip::{Segment, SegmentStyle, build_strip, needs_charge};
pub use text::{EMPTY_MENU_TEXT, menu_line, shown_age, tooltip_compact};
