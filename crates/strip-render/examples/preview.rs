//! Writes `target/preview.png`: every style, kind and theme on a bar-coloured background.

use strip_render::{Bitmap, Theme, render_square, render_strip};
use tiny_skia::Pixmap;
use ui_model::{DeviceKind, Segment, SegmentStyle};

const MARGIN: u32 = 12;

fn segment(kind: DeviceKind, caption: Option<&str>, text: &str, style: SegmentStyle) -> Segment {
    Segment {
        kind,
        caption: caption.map(str::to_owned),
        text: text.to_owned(),
        style,
    }
}

/// Alpha-composites `bitmap` onto the opaque `canvas` at (`x`, `y`).
fn blit(canvas: &mut Pixmap, bitmap: &Bitmap, x: u32, y: u32) {
    let canvas_width = canvas.width();
    let data = canvas.data_mut();
    for (i, pixel) in bitmap.pixels().iter().enumerate() {
        let (px, py) = (i as u32 % bitmap.width, i as u32 / bitmap.width);
        let at = (((y + py) * canvas_width + x + px) * 4) as usize;
        let alpha = u32::from(pixel[3]);
        for channel in 0..3 {
            let under = u32::from(data[at + channel]);
            data[at + channel] =
                ((under * (255 - alpha) + u32::from(pixel[channel]) * alpha) / 255) as u8;
        }
    }
}

fn main() {
    use DeviceKind::*;
    use SegmentStyle::*;
    let rows = [
        vec![
            segment(Headset, None, "84%", Normal),
            segment(Earbuds, None, "12%", Low),
            segment(Mouse, None, "55%", Charging),
            segment(Keyboard, None, "100%", Full),
        ],
        vec![
            segment(Gamepad, Some("1"), "40%", Dimmed),
            segment(Other, Some("XM4"), "70%", Normal),
            segment(Headset, Some("XM5"), "9%", Low),
            segment(Other, None, "–", Placeholder),
        ],
    ];

    let mut bitmaps: Vec<(Bitmap, [u8; 3])> = Vec::new();
    for (theme, background) in [(Theme::Light, [243, 243, 243]), (Theme::Dark, [32, 32, 32])] {
        for height in [96, 44] {
            for row in &rows {
                bitmaps.push((render_strip(row, height, theme), background));
            }
        }
        let squares: Vec<Bitmap> = rows
            .iter()
            .flatten()
            .map(|s| render_square(s, 32, theme))
            .collect();
        for square in squares {
            bitmaps.push((square, background));
        }
    }

    let width = bitmaps.iter().map(|(b, _)| b.width).max().unwrap_or(1) + 2 * MARGIN;
    let height: u32 = bitmaps.iter().map(|(b, _)| b.height + MARGIN).sum::<u32>() + MARGIN;
    let mut canvas = Pixmap::new(width, height).expect("preview size is non-zero");
    let mut y = MARGIN / 2;
    for (bitmap, background) in &bitmaps {
        let band = bitmap.height + MARGIN;
        for row in y - MARGIN / 2..y - MARGIN / 2 + band {
            for column in 0..width {
                let at = ((row * width + column) * 4) as usize;
                canvas.data_mut()[at..at + 4].copy_from_slice(&[
                    background[0],
                    background[1],
                    background[2],
                    255,
                ]);
            }
        }
        blit(&mut canvas, bitmap, MARGIN, y);
        y += band;
    }

    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/preview.png");
    canvas.save_png(out).expect("write preview.png");
    println!("wrote {out}");
}
