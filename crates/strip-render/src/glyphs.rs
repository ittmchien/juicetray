//! Vector shapes in a unit square (0..1 on both axes): device kind glyphs and the charging / full marks.

use tiny_skia::{Path, PathBuilder};
use ui_model::DeviceKind;

/// Line weight of outlined device glyphs, in unit-square coordinates.
const LINE: f32 = 0.09;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartStyle {
    Fill,
    /// Stroke width in unit-square coordinates.
    Stroke(f32),
}

pub struct Part {
    pub path: Path,
    pub style: PartStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Bolt,
    Check,
}

/// The shapes that make up the glyph of a device kind.
pub fn kind_parts(kind: DeviceKind) -> Vec<Part> {
    let parts = match kind {
        DeviceKind::Headset => vec![
            // Headband arc between two filled ear cups.
            part(PartStyle::Stroke(LINE), |b| {
                b.move_to(0.14, 0.66);
                b.cubic_to(0.14, -0.1, 0.86, -0.1, 0.86, 0.66);
            }),
            part(PartStyle::Fill, |b| {
                rounded_rect(b, 0.04, 0.5, 0.23, 0.46, 0.1)
            }),
            part(PartStyle::Fill, |b| {
                rounded_rect(b, 0.73, 0.5, 0.23, 0.46, 0.1)
            }),
        ],
        DeviceKind::Earbuds => vec![
            // Two buds, each a round head with a stem on the inner side.
            part(PartStyle::Fill, |b| b.push_circle(0.26, 0.3, 0.2)),
            part(PartStyle::Fill, |b| {
                rounded_rect(b, 0.34, 0.3, 0.12, 0.64, 0.06)
            }),
            part(PartStyle::Fill, |b| b.push_circle(0.74, 0.3, 0.2)),
            part(PartStyle::Fill, |b| {
                rounded_rect(b, 0.54, 0.3, 0.12, 0.64, 0.06)
            }),
        ],
        DeviceKind::Mouse => vec![
            // Capsule body split into buttons by two lines.
            part(PartStyle::Stroke(LINE), |b| {
                rounded_rect(b, 0.22, 0.06, 0.56, 0.88, 0.28);
                b.move_to(0.5, 0.06);
                b.line_to(0.5, 0.4);
                b.move_to(0.22, 0.4);
                b.line_to(0.78, 0.4);
            }),
        ],
        DeviceKind::Keyboard => vec![
            // Outline with a row of keys and a space bar.
            part(PartStyle::Stroke(LINE), |b| {
                rounded_rect(b, 0.04, 0.2, 0.92, 0.6, 0.1)
            }),
            part(PartStyle::Fill, |b| {
                for i in 0..4 {
                    rect(b, 0.2 + 0.17 * i as f32, 0.32, 0.1, 0.1);
                }
                rect(b, 0.3, 0.56, 0.4, 0.1);
            }),
        ],
        DeviceKind::Gamepad => vec![
            // Rounded body with a d-pad on the left and two buttons on the right.
            part(PartStyle::Stroke(LINE), |b| {
                rounded_rect(b, 0.04, 0.24, 0.92, 0.52, 0.24)
            }),
            part(PartStyle::Fill, |b| cross(b, 0.28, 0.5, 0.1, 0.035)),
            part(PartStyle::Fill, |b| {
                b.push_circle(0.68, 0.56, 0.055);
                b.push_circle(0.8, 0.44, 0.055);
            }),
        ],
        DeviceKind::Other => vec![
            // Battery outline with terminal and a partial charge bar.
            part(PartStyle::Stroke(LINE), |b| {
                rounded_rect(b, 0.04, 0.26, 0.8, 0.48, 0.08)
            }),
            part(PartStyle::Fill, |b| rect(b, 0.88, 0.42, 0.08, 0.16)),
            part(PartStyle::Fill, |b| rect(b, 0.15, 0.37, 0.3, 0.26)),
        ],
    };
    parts.into_iter().flatten().collect()
}

/// The shapes of a charging bolt or a full-battery check mark.
pub fn mark_parts(mark: Mark) -> Vec<Part> {
    let part = match mark {
        Mark::Bolt => part(PartStyle::Fill, |b| {
            b.move_to(0.62, 0.0);
            b.line_to(0.18, 0.56);
            b.line_to(0.46, 0.56);
            b.line_to(0.36, 1.0);
            b.line_to(0.82, 0.4);
            b.line_to(0.54, 0.4);
            b.close();
        }),
        Mark::Check => part(PartStyle::Stroke(0.16), |b| {
            b.move_to(0.1, 0.55);
            b.line_to(0.38, 0.85);
            b.line_to(0.92, 0.2);
        }),
    };
    part.into_iter().collect()
}

fn part(style: PartStyle, build: impl FnOnce(&mut PathBuilder)) -> Option<Part> {
    let mut builder = PathBuilder::new();
    build(&mut builder);
    builder.finish().map(|path| Part { path, style })
}

fn rect(b: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32) {
    b.move_to(x, y);
    b.line_to(x + w, y);
    b.line_to(x + w, y + h);
    b.line_to(x, y + h);
    b.close();
}

/// Rectangle whose corners are quadratic curves of radius `r`.
fn rounded_rect(b: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    b.move_to(x + r, y);
    b.line_to(x + w - r, y);
    b.quad_to(x + w, y, x + w, y + r);
    b.line_to(x + w, y + h - r);
    b.quad_to(x + w, y + h, x + w - r, y + h);
    b.line_to(x + r, y + h);
    b.quad_to(x, y + h, x, y + h - r);
    b.line_to(x, y + r);
    b.quad_to(x, y, x + r, y);
    b.close();
}

/// Plus sign centred on (`cx`, `cy`) with arms of half-length `arm` and half-thickness `half`.
fn cross(b: &mut PathBuilder, cx: f32, cy: f32, arm: f32, half: f32) {
    let outline = [
        (-half, -arm),
        (half, -arm),
        (half, -half),
        (arm, -half),
        (arm, half),
        (half, half),
        (half, arm),
        (-half, arm),
        (-half, half),
        (-arm, half),
        (-arm, -half),
        (-half, -half),
    ];
    for (i, (dx, dy)) in outline.into_iter().enumerate() {
        if i == 0 {
            b.move_to(cx + dx, cy + dy);
        } else {
            b.line_to(cx + dx, cy + dy);
        }
    }
    b.close();
}
