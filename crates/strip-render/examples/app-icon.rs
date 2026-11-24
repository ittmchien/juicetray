//! Writes the app icon: `target/AppIcon.iconset/*.png` (for `iconutil`) and `packaging/windows/juicetray.ico`.

use std::{fs, path::Path};

use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode,
    Stroke, Transform,
};

/// Apple iconset file name and pixel size of each entry.
const ICONSET: [(&str, u32); 10] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];
const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// Largest ICO entry stored as PNG; smaller ones are BMP so every resource compiler accepts them.
const ICO_PNG_FROM: u32 = 256;

/// Rounded square with a green gradient and a white battery (outline, terminal, charge bar).
fn render_icon(size: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).expect("icon size is non-zero");
    let s = size as f32;
    let unit = Transform::from_scale(s, s);

    let fill = |pixmap: &mut Pixmap, path: &tiny_skia::Path, paint: &Paint| {
        pixmap.fill_path(path, paint, FillRule::Winding, unit, None);
    };

    // Background: inset rounded square, lighter at the top (gradient in unit coordinates, scaled by `unit`).
    let background = Paint {
        anti_alias: true,
        shader: LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(0.0, 1.0),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(0x3A, 0xB5, 0x7B, 255)),
                GradientStop::new(1.0, Color::from_rgba8(0x24, 0x8A, 0x58, 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .expect("gradient endpoints differ"),
        ..Paint::default()
    };
    fill(
        &mut pixmap,
        &rounded_rect(0.06, 0.06, 0.88, 0.88, 0.2),
        &background,
    );

    let mut white = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    white.set_color_rgba8(255, 255, 255, 255);

    // Battery body is an outline thick enough to survive 16 px; the terminal sits on its right.
    let outline = Stroke {
        width: 0.075,
        ..Stroke::default()
    };
    pixmap.stroke_path(
        &rounded_rect(0.18, 0.32, 0.52, 0.36, 0.07),
        &white,
        &outline,
        unit,
        None,
    );
    fill(
        &mut pixmap,
        &rounded_rect(0.755, 0.44, 0.07, 0.12, 0.02),
        &white,
    );
    fill(
        &mut pixmap,
        &rounded_rect(0.275, 0.415, 0.29, 0.17, 0.02),
        &white,
    );
    pixmap
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let mut b = PathBuilder::new();
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
    b.finish().expect("rounded rect is a valid path")
}

/// 32-bit BMP image data of an ICO entry: DIB header, bottom-up BGRA rows, empty AND mask.
fn ico_bmp(pixmap: &Pixmap) -> Vec<u8> {
    let (w, h) = (pixmap.width(), pixmap.height());
    let mut out = Vec::new();
    out.extend(40u32.to_le_bytes());
    out.extend(w.to_le_bytes());
    out.extend((h * 2).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]);
    for row in pixmap.pixels().chunks(w as usize).rev() {
        for px in row {
            let c = px.demultiply();
            out.extend([c.blue(), c.green(), c.red(), c.alpha()]);
        }
    }
    out.extend(vec![0u8; (w.div_ceil(32) * 4 * h) as usize]);
    out
}

/// ICO container: header, one directory entry per image, then the image data.
fn write_ico(path: &Path) {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|&size| {
            let pixmap = render_icon(size);
            let data = if size >= ICO_PNG_FROM {
                pixmap.encode_png().expect("encode png")
            } else {
                ico_bmp(&pixmap)
            };
            (size, data)
        })
        .collect();

    let mut out = Vec::new();
    out.extend([0u8, 0, 1, 0]);
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        // A stored dimension of 0 means 256.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([dim, dim, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &images {
        out.extend(data);
    }
    fs::write(path, out).expect("write ico");
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let iconset = root.join("target/AppIcon.iconset");
    fs::create_dir_all(&iconset).expect("create iconset dir");
    for (name, size) in ICONSET {
        render_icon(size)
            .save_png(iconset.join(name))
            .expect("write iconset png");
    }
    write_ico(&root.join("packaging/windows/juicetray.ico"));
    println!(
        "wrote {} and packaging/windows/juicetray.ico",
        iconset.display()
    );
}
