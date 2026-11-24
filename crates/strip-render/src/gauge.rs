//! The vertical battery gauge: rounded outline, top cap and a fill proportional to the level.

use tiny_skia::{Path, PathBuilder};

// Proportions, relative to the bitmap height.
pub(crate) const WIDTH: f32 = 0.36;
pub(crate) const HEIGHT: f32 = 0.8;
pub(crate) const CAP_WIDTH: f32 = 0.16;
pub(crate) const CAP_HEIGHT: f32 = 0.06;
pub(crate) const OUTLINE: f32 = 0.07;
pub(crate) const CORNER_RADIUS: f32 = 0.1;

/// Gauge shapes with the gauge's top-left corner at (0, 0).
pub(crate) struct Gauge {
    /// Centre line of the outline stroke.
    pub frame: Path,
    pub outline_width: f32,
    pub cap: Path,
    /// `None` at 0 %.
    pub fill: Option<Path>,
}

/// Shapes of a gauge for a bitmap `height` pixels tall, filled to `level` percent (capped at 100).
pub(crate) fn gauge(height: f32, level: u8) -> Option<Gauge> {
    let width = WIDTH * height;
    let cap_height = CAP_HEIGHT * height;
    let body_height = HEIGHT * height - cap_height;
    let outline = OUTLINE * height;
    let radius = CORNER_RADIUS * height;

    // The stroke is centred on its path, so the frame sits half an outline inside the outer edge.
    let half = outline / 2.0;
    let frame = path(|b| {
        rounded_rect(
            b,
            half,
            cap_height + half,
            width - outline,
            body_height - outline,
            radius - half,
        );
    })?;
    let cap_width = CAP_WIDTH * height;
    let cap = path(|b| {
        rect(
            b,
            (width - cap_width) / 2.0,
            0.0,
            cap_width,
            cap_height + outline,
        )
    })?;

    // The fill starts one corner radius inside the edge, so its corners stay within the rounded frame.
    let inner_width = width - 2.0 * radius;
    let inner_height = body_height - 2.0 * radius;
    let fill_height = inner_height * f32::from(level.min(100)) / 100.0;
    let fill = if fill_height > 0.0 {
        path(|b| {
            rect(
                b,
                radius,
                cap_height + radius + inner_height - fill_height,
                inner_width,
                fill_height,
            );
        })
    } else {
        None
    };
    Some(Gauge {
        frame,
        outline_width: outline,
        cap,
        fill,
    })
}

fn path(build: impl FnOnce(&mut PathBuilder)) -> Option<Path> {
    let mut builder = PathBuilder::new();
    build(&mut builder);
    builder.finish()
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
