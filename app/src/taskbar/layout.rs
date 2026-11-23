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
    if !is_usable_child(taskbar, notify_area) {
        return Placement::Fallback;
    }
    dock_left_of(taskbar, notify_area.x, strip_w, strip_h)
}

/// Docks a secondary taskbar's strip left of its clock, or at the bar's right end without one.
/// Both input rects are in screen coordinates; the result is relative to `taskbar`'s origin.
pub fn place_secondary_strip(
    taskbar: Rect,
    clock: Option<Rect>,
    strip_w: i32,
    strip_h: i32,
) -> Placement {
    let edge = clock
        .filter(|clock| is_usable_child(taskbar, *clock))
        .map_or(taskbar.right(), |clock| clock.x);
    dock_left_of(taskbar, edge, strip_w, strip_h)
}

/// The task-button container's rect shrunk to end at `strip_x`, or `None` when it already ends
/// there or the strip starts left of it; both are in the taskbar's client coordinates.
pub fn shrink_to_clear(container: Rect, strip_x: i32) -> Option<Rect> {
    (container.right() > strip_x && strip_x > container.x).then_some(Rect {
        w: strip_x - container.x,
        ..container
    })
}

/// Explorer's own rect for a task-button container: its left, top and height kept, its right
/// edge at `edge` (the sibling it reaches), so a container left shrunk by a crash is healed.
/// `None` on a vertical bar or an edge outside the container's reach; all in client coordinates
/// of the taskbar, whose client size is `bar`.
pub fn natural_container(bar: Rect, container: Rect, edge: i32) -> Option<Rect> {
    (bar.w > bar.h && edge > container.x && edge <= bar.w).then_some(Rect {
        w: edge - container.x,
        ..container
    })
}

fn is_usable_child(taskbar: Rect, child: Rect) -> bool {
    child.w > 0 && child.h > 0 && taskbar.contains(child)
}

/// Strip ending at screen x `edge`, vertically centred, relative to `taskbar`'s origin.
fn dock_left_of(taskbar: Rect, edge: i32, strip_w: i32, strip_h: i32) -> Placement {
    let horizontal = taskbar.w > taskbar.h;
    // A strip wider than a quarter of the bar would cover the app buttons.
    if !horizontal || strip_w <= 0 || strip_w > taskbar.w / 4 {
        return Placement::Fallback;
    }
    let x = edge - taskbar.x - strip_w;
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

    /// Second display right of the primary, as measured on a Windows 10 19045 machine.
    const SECONDARY: Rect = Rect {
        x: 1920,
        y: 1040,
        w: 1920,
        h: 40,
    };

    #[test]
    fn secondary_docks_left_of_clock() {
        let clock = Rect {
            x: 3776,
            y: 1040,
            w: 64,
            h: 40,
        };
        assert_eq!(
            place_secondary_strip(SECONDARY, Some(clock), 120, 30),
            Placement::Docked(Rect {
                x: 1736,
                y: 5,
                w: 120,
                h: 30
            })
        );
    }

    #[test]
    fn secondary_without_clock_docks_at_right_end() {
        let docked = Placement::Docked(Rect {
            x: 1800,
            y: 5,
            w: 120,
            h: 30,
        });
        assert_eq!(place_secondary_strip(SECONDARY, None, 120, 30), docked);
        // A clock outside the bar is ignored like a missing one.
        let stray = Rect {
            x: 0,
            y: 0,
            w: 64,
            h: 40,
        };
        assert_eq!(
            place_secondary_strip(SECONDARY, Some(stray), 120, 30),
            docked
        );
    }

    #[test]
    fn vertical_or_narrow_secondary_falls_back() {
        let vertical = Rect {
            x: 1920,
            y: 0,
            w: 60,
            h: 1080,
        };
        assert_eq!(
            place_secondary_strip(vertical, None, 20, 30),
            Placement::Fallback
        );
        assert_eq!(
            place_secondary_strip(SECONDARY, None, 600, 30),
            Placement::Fallback
        );
    }

    /// `ReBarWindow32` in client coordinates, as measured on a Windows 10 19045 machine.
    const REBAR: Rect = Rect {
        x: 46,
        y: 0,
        w: 1517,
        h: 40,
    };

    #[test]
    fn shrinks_container_to_end_at_strip() {
        assert_eq!(
            shrink_to_clear(REBAR, 1443),
            Some(Rect {
                x: 46,
                y: 0,
                w: 1397,
                h: 40
            })
        );
    }

    #[test]
    fn container_already_clear_is_left_alone() {
        assert_eq!(shrink_to_clear(REBAR, 1563), None);
        assert_eq!(shrink_to_clear(REBAR, 1600), None);
    }

    #[test]
    fn strip_left_of_container_is_left_alone() {
        assert_eq!(shrink_to_clear(REBAR, 46), None);
        assert_eq!(shrink_to_clear(REBAR, 10), None);
    }

    /// Primary taskbar client area, as measured on a Windows 10 19045 machine.
    const BAR: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 40,
    };

    #[test]
    fn natural_container_reaches_the_sibling_edge() {
        // `TrayNotifyWnd` starts at 1563, exactly where Explorer ends the rebar.
        assert_eq!(natural_container(BAR, REBAR, 1563), Some(REBAR));
    }

    #[test]
    fn natural_container_heals_a_crash_shrunk_container() {
        let shrunk = Rect { w: 1428, ..REBAR };
        assert_eq!(natural_container(BAR, shrunk, 1563), Some(REBAR));
    }

    #[test]
    fn natural_container_rejects_vertical_bar_and_bad_edges() {
        let vertical = Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 1080,
        };
        assert_eq!(natural_container(vertical, REBAR, 50), None);
        assert_eq!(natural_container(BAR, REBAR, 46), None);
        assert_eq!(natural_container(BAR, REBAR, 1921), None);
    }
}
