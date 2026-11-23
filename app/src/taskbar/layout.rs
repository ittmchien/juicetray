//! Pure geometry: where the strip sits relative to the taskbar and its notification area.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(self) -> i32 {
        self.x + self.w
    }

    fn bottom(self) -> i32 {
        self.y + self.h
    }

    fn contains(self, inner: Rect) -> bool {
        inner.x >= self.x
            && inner.y >= self.y
            && inner.right() <= self.right()
            && inner.bottom() <= self.bottom()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Position relative to the taskbar's client origin.
    Docked(Rect),
    /// The strip stays hidden; the tray icon is the only display.
    Fallback,
}

/// Docks the strip immediately left of the notification area, vertically centred.
/// Both input rects are in screen coordinates; the result is relative to `taskbar`'s origin.
pub fn place_strip(taskbar: Rect, notify_area: Rect, strip_w: i32, strip_h: i32) -> Placement {
    let horizontal = taskbar.w > taskbar.h;
    let notify_usable = notify_area.w > 0 && notify_area.h > 0 && taskbar.contains(notify_area);
    // A strip wider than a quarter of the bar would cover the app buttons.
    if !horizontal || !notify_usable || strip_w <= 0 || strip_w > taskbar.w / 4 {
        return Placement::Fallback;
    }
    let x = notify_area.x - taskbar.x - strip_w;
    let y = (taskbar.h - strip_h) / 2;
    if x < 0 || y < 0 {
        return Placement::Fallback;
    }
    Placement::Docked(Rect {
        x,
        y,
        w: strip_w,
        h: strip_h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASKBAR: Rect = Rect {
        x: 0,
        y: 1040,
        w: 1920,
        h: 40,
    };
    const NOTIFY: Rect = Rect {
        x: 1700,
        y: 1040,
        w: 220,
        h: 40,
    };

    #[test]
    fn docks_left_of_notify_area() {
        assert_eq!(
            place_strip(TASKBAR, NOTIFY, 120, 30),
            Placement::Docked(Rect {
                x: 1580,
                y: 5,
                w: 120,
                h: 30
            })
        );
    }

    #[test]
    fn vertical_taskbar_falls_back() {
        let vertical = Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 1080,
        };
        let notify = Rect {
            x: 0,
            y: 980,
            w: 60,
            h: 100,
        };
        assert_eq!(place_strip(vertical, notify, 20, 30), Placement::Fallback);
    }

    #[test]
    fn too_wide_falls_back() {
        assert_eq!(place_strip(TASKBAR, NOTIFY, 600, 30), Placement::Fallback);
    }

    #[test]
    fn missing_notify_area_falls_back() {
        let missing = Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        assert_eq!(place_strip(TASKBAR, missing, 120, 30), Placement::Fallback);
    }

    #[test]
    fn notify_outside_taskbar_falls_back() {
        let outside = Rect {
            x: 1700,
            y: 0,
            w: 220,
            h: 40,
        };
        assert_eq!(place_strip(TASKBAR, outside, 120, 30), Placement::Fallback);
    }
}
