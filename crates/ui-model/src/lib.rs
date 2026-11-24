//! Pure view model shared by the tray app: device data in, strip segments and menu text out.

mod caption;
mod device;
mod display;
mod strip;
mod text;

pub use caption::captions;
pub use device::{
    DeviceKind, DeviceView, Level, Path, UnknownDeviceKind, group_label, group_order, sort_grouped,
    sort_weakest_first,
};
pub use display::{DisplayOptions, IconChoice, IconColor};
pub use strip::{Segment, SegmentStyle, build_strip};
pub use text::{EMPTY_MENU_TEXT, menu_line, tooltip_compact};
