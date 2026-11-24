//! Writes `target/preview.png`: every style, icon, colour and theme on a bar-coloured background.

use std::collections::HashMap;

use strip_render::{Bitmap, SvgIcon, TextStyle, Theme, render_square, render_strip};
use tiny_skia::Pixmap;
use ui_model::{DeviceKind, IconChoice, IconColor, Segment, SegmentStyle};

const MARGIN: u32 = 12;
/// A two-colour star standing in for an owner's SVG.
const STAR_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#E5484D" d="M12 2l3 7 7 .6-5.4 4.7 1.7 7.2L12 17.7 5.7 21.5l1.7-7.2L2 9.6 9 9z"/><circle cx="12" cy="12" r="3" fill="#FFC53D"/></svg>"##;

fn segment(
    kind: DeviceKind,
    caption: Option<&str>,
    percent: Option<u8>,
    style: SegmentStyle,
) -> Segment {
    Segment {
        caption: caption.map(str::to_owned),
        text: percent.map_or_else(|| "–".to_owned(), |p| format!("{p}%")),
        style,
        icon: IconChoice::Kind(kind),
        color_light: IconColor::Auto,
        color_dark: IconColor::Auto,
        gauge: percent,
    }
}

fn with_icon(segment: Segment, icon: IconChoice, color: IconColor) -> Segment {
    Segment {
        icon,
        color_light: color,
        color_dark: color,
        ..segment
    }
}

fn without_percent(segment: Segment) -> Segment {
    Segment {
        text: String::new(),
        ..segment
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
    let svgs = HashMap::from([(
        "star.svg".to_owned(),
        SvgIcon::parse(STAR_SVG).expect("star SVG is valid"),
    )]);
    let star = IconChoice::Svg("star.svg".to_owned());
    let blue = IconColor::Rgb(0x3E, 0x8E, 0xED);
    let rows = [
        vec![
            segment(Headset, None, Some(84), Normal),
            segment(Earbuds, None, Some(12), Low),
            segment(Mouse, None, Some(55), Charging),
            segment(Keyboard, None, Some(100), Full),
        ],
        vec![
            segment(Gamepad, Some("1"), Some(40), Dimmed),
            segment(Other, Some("XM4"), Some(70), Normal),
            segment(Headset, Some("XM5"), Some(9), Low),
            segment(Other, None, None, Placeholder),
        ],
        // Gauge levels, 0 % (empty outline) up to 100 %.
        vec![
            segment(Mouse, None, Some(0), Low),
            segment(Mouse, None, Some(25), Medium),
            segment(Mouse, None, Some(50), Normal),
            segment(Mouse, None, Some(75), Normal),
            segment(Mouse, None, Some(100), Normal),
        ],
        // Percentage off, a tinted built-in icon, an SVG in its own colours and tinted.
        vec![
            without_percent(segment(Headset, None, Some(64), Normal)),
            with_icon(
                segment(Mouse, None, Some(64), Normal),
                IconChoice::Kind(Mouse),
                blue,
            ),
            with_icon(
                segment(Other, None, Some(64), Normal),
                star.clone(),
                IconColor::Original,
            ),
            with_icon(segment(Other, None, Some(64), Normal), star, blue),
        ],
    ];

    let mut bitmaps: Vec<(Bitmap, [u8; 3])> = Vec::new();
    for (theme, background) in [(Theme::Light, [243, 243, 243]), (Theme::Dark, [32, 32, 32])] {
        for height in [96, 44] {
            for row in &rows {
                let text = TextStyle::fallback(height);
                bitmaps.push((render_strip(row, height, theme, &svgs, &text), background));
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
