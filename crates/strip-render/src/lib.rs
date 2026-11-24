//! Draws battery strip segments to straight-alpha RGBA bitmaps with tiny-skia: SVG icons, a vertical gauge and text.

mod gauge;
mod svg;
mod text;

use std::collections::HashMap;

use ab_glyph::FontRef;
use gauge::{Gauge, gauge};
use svg::builtin_icon;
#[cfg(feature = "embedded-font")]
use text::embedded_font;
use text::{Run, digit_height, layout_text};
use tiny_skia::{
    Color, FillRule, Mask, MaskType, Paint, Path, Pixmap, PixmapPaint, Stroke, Transform,
};
use ui_model::{DeviceKind, IconChoice, IconColor, Segment, SegmentStyle};

pub use svg::{SvgIcon, load_svg};
pub use text::{FontData, TextStyle};

// Proportions, all relative to the bitmap height; sized to read clearly in a 22 pt menu bar.
const ICON_SIZE: f32 = 0.8;
const CAPTION_SIZE: f32 = 0.34;
/// Space between icon (or caption), gauge and number.
const PART_GAP: f32 = 0.12;
/// Space between two segments.
const SEGMENT_GAP: f32 = 0.45;
const EDGE_PADDING: f32 = 0.25;
/// Largest number size, so a huge OS font size never overflows the strip.
const MAX_TEXT_SIZE: f32 = 0.75;
/// Number size of the square tray icon.
#[cfg(feature = "embedded-font")]
const SQUARE_TEXT_SIZE: f32 = 0.78;
/// Share of a square icon the number may fill horizontally.
#[cfg(feature = "embedded-font")]
const SQUARE_TEXT_FILL: f32 = 0.9;

/// Opacity of the ink for dimmed and placeholder segments.
const DIMMED_ALPHA: u8 = 115;

/// Which bar the bitmap sits on: `Light` draws dark ink, `Dark` draws white ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// Straight (non-premultiplied) RGBA pixels, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Bitmap {
    /// The pixels as `[r, g, b, a]` quadruples.
    pub fn pixels(&self) -> &[[u8; 4]] {
        self.rgba.as_chunks::<4>().0
    }

    fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        }
    }
}

/// What every segment of one strip is drawn with.
struct Scene<'a> {
    height: f32,
    theme: Theme,
    svgs: &'a HashMap<String, SvgIcon>,
    /// `None` draws no text.
    font: Option<FontRef<'a>>,
    text_px: f32,
}

/// Icon, optional caption, gauge and optional number of every segment, left to right.
/// `svgs` maps `IconChoice::Svg` paths to parsed files.
pub fn render_strip(
    segments: &[Segment],
    height_px: u32,
    theme: Theme,
    svgs: &HashMap<String, SvgIcon>,
    text: &TextStyle,
) -> Bitmap {
    let height = height_px as f32;
    let scene = Scene {
        height,
        theme,
        svgs,
        font: text.font_ref(),
        text_px: text.size_px.min(MAX_TEXT_SIZE * height),
    };
    let layouts: Vec<SegmentLayout> = segments
        .iter()
        .map(|segment| SegmentLayout::new(segment, &scene))
        .collect();
    let gaps = layouts.len().saturating_sub(1) as f32 * SEGMENT_GAP * height;
    let content: f32 = layouts.iter().map(|layout| layout.width).sum();
    let width = (content + gaps + 2.0 * EDGE_PADDING * height).ceil() as u32;
    let Some(mut canvas) = Canvas::new(width, height_px) else {
        return Bitmap::empty();
    };

    let mut x = EDGE_PADDING * height;
    for (segment, layout) in segments.iter().zip(&layouts) {
        // Faded segments are drawn opaque on their own layer and composited once, so
        // overlapping shapes never stack their alpha.
        if is_faded(segment.style) {
            let mut layer = canvas.blank_layer();
            draw_segment(&mut layer, segment, layout, x, &scene);
            canvas.composite(&layer, f32::from(DIMMED_ALPHA) / 255.0);
        } else {
            draw_segment(&mut canvas, segment, layout, x, &scene);
        }
        x += layout.width + SEGMENT_GAP * height;
    }
    canvas.into_bitmap()
}

/// Only the number (the gauge's level, no `%`, no icon), shrunk to fit and tinted by the segment style.
#[cfg(feature = "embedded-font")]
pub fn render_square(segment: &Segment, size_px: u32, theme: Theme) -> Bitmap {
    let Some(mut canvas) = Canvas::new(size_px, size_px) else {
        return Bitmap::empty();
    };
    let size = size_px as f32;
    let font = embedded_font();
    // The tray icon shows a number whatever the strip's "show percentage" choice is.
    let text = segment
        .gauge
        .map_or_else(|| segment.text.clone(), |level| level.to_string());

    // Advance grows linearly with the font size, so one probe layout gives the fitting size.
    let unit_advance = layout_text(&font, &text, 1.0).advance;
    let px = if unit_advance > 0.0 {
        (SQUARE_TEXT_SIZE * size).min(SQUARE_TEXT_FILL * size / unit_advance)
    } else {
        SQUARE_TEXT_SIZE * size
    };
    let run = layout_text(&font, &text, px);
    let palette = Palette::new(segment.style, theme);
    let origin = Transform::from_translate(
        (size - run.advance) / 2.0,
        size / 2.0 + digit_height(&font, px) / 2.0,
    );
    // A single shape, so fading by colour alpha cannot stack.
    let mut color = palette.accent.unwrap_or(palette.ink);
    if is_faded(segment.style) {
        color.apply_opacity(f32::from(DIMMED_ALPHA) / 255.0);
    }
    if let Some(path) = &run.path {
        canvas.fill(path, color, origin);
    }
    canvas.into_bitmap()
}

/// A segment's text runs and total width at the scene's bitmap height.
struct SegmentLayout {
    caption: Option<Run>,
    number: Option<Run>,
    width: f32,
}

impl SegmentLayout {
    fn new(segment: &Segment, scene: &Scene) -> Self {
        let height = scene.height;
        let caption = scene
            .font
            .as_ref()
            .zip(segment.caption.as_deref())
            .map(|(font, caption)| layout_text(font, caption, CAPTION_SIZE * height));
        let number = scene
            .font
            .as_ref()
            .filter(|_| !segment.text.is_empty())
            .map(|font| layout_text(font, &segment.text, scene.text_px));

        let mut width = ICON_SIZE * height;
        if let Some(caption) = &caption {
            width += caption.advance;
        }
        if segment.gauge.is_some() {
            width += (PART_GAP + gauge::WIDTH) * height;
        }
        if let Some(number) = &number {
            width += PART_GAP * height + number.advance;
        }
        Self {
            caption,
            number,
            width,
        }
    }
}

fn draw_segment(
    canvas: &mut Canvas,
    segment: &Segment,
    layout: &SegmentLayout,
    x: f32,
    scene: &Scene,
) {
    let height = scene.height;
    let palette = Palette::new(segment.style, scene.theme);
    let icon_size = ICON_SIZE * height;
    let top = (height - icon_size) / 2.0;
    let mut cursor = x;

    let (icon, is_svg) = resolve_icon(&segment.icon, scene.svgs);
    // Each theme has its own icon colour, so the owner can pick one that reads on that bar.
    let color = match scene.theme {
        Theme::Light => segment.color_light,
        Theme::Dark => segment.color_dark,
    };
    let tint = icon_tint(color, is_svg, palette.ink);
    canvas.draw_icon(icon, cursor, top, icon_size, tint);
    cursor += icon_size;

    // The caption sits at the icon's bottom right.
    if let Some(caption) = &layout.caption {
        if let Some(path) = &caption.path {
            canvas.fill(
                path,
                palette.ink,
                Transform::from_translate(cursor, top + icon_size),
            );
        }
        cursor += caption.advance;
    }

    if let Some(shape) = segment.gauge.and_then(|level| gauge(height, level)) {
        cursor += PART_GAP * height;
        let fill_color = palette.accent.unwrap_or(palette.ink);
        canvas.draw_gauge(&shape, cursor, top, palette.ink, fill_color);
        cursor += gauge::WIDTH * height;
    }

    // The number's digits are centred vertically on the gauge.
    if let (Some(number), Some(font)) = (&layout.number, &scene.font) {
        cursor += PART_GAP * height;
        let baseline = height / 2.0 + digit_height(font, scene.text_px) / 2.0;
        if let Some(path) = &number.path {
            canvas.fill(
                path,
                palette.ink,
                Transform::from_translate(cursor, baseline),
            );
        }
    }
}

/// The icon to draw and whether it is the owner's SVG; a path missing from `svgs` draws the generic icon.
fn resolve_icon<'a>(
    choice: &IconChoice,
    svgs: &'a HashMap<String, SvgIcon>,
) -> (&'a SvgIcon, bool) {
    match choice {
        IconChoice::Kind(kind) => (builtin_icon(*kind), false),
        IconChoice::Svg(path) => svgs.get(path).map_or_else(
            || (builtin_icon(DeviceKind::Other), false),
            |icon| (icon, true),
        ),
        // Segments carry a resolved icon; `Auto` only reaches here from hand-built segments.
        IconChoice::Auto => (builtin_icon(DeviceKind::Other), false),
    }
}

/// Flat colour the icon is painted with; `None` keeps an SVG's own colours.
fn icon_tint(color: IconColor, is_svg: bool, ink: Color) -> Option<Color> {
    match color {
        IconColor::Rgb(r, g, b) => Some(Color::from_rgba8(r, g, b, 255)),
        IconColor::Original if is_svg => None,
        IconColor::Auto | IconColor::Original => Some(ink),
    }
}

/// Asleep, disconnected and placeholder segments are drawn at reduced opacity.
fn is_faded(style: SegmentStyle) -> bool {
    matches!(style, SegmentStyle::Dimmed | SegmentStyle::Placeholder)
}

/// Ink colour for the theme and the status accent of a style.
struct Palette {
    ink: Color,
    accent: Option<Color>,
}

/// (light bar, dark bar) gauge shades, each at least 3:1 against its own bar (#EEEEEE / #1F1F1F).
type AccentShades = ((u8, u8, u8), (u8, u8, u8));
const ACCENT_RED: AccentShades = ((0xD6, 0x3A, 0x3F), (0xF2, 0x68, 0x6D));
const ACCENT_YELLOW: AccentShades = ((0xA8, 0x75, 0x00), (0xFF, 0xCB, 0x47));
const ACCENT_GREEN: AccentShades = ((0x2A, 0x8F, 0x5E), (0x4D, 0xBE, 0x8A));

impl Palette {
    fn new(style: SegmentStyle, theme: Theme) -> Self {
        let (r, g, b) = match theme {
            Theme::Light => (0, 0, 0),
            Theme::Dark => (255, 255, 255),
        };
        let shades = match style {
            SegmentStyle::Low => Some(ACCENT_RED),
            SegmentStyle::Medium => Some(ACCENT_YELLOW),
            SegmentStyle::Charging | SegmentStyle::Full => Some(ACCENT_GREEN),
            _ => None,
        };
        let accent = shades.map(|(light, dark)| {
            let (r, g, b) = match theme {
                Theme::Light => light,
                Theme::Dark => dark,
            };
            Color::from_rgba8(r, g, b, 255)
        });
        Self {
            ink: Color::from_rgba8(r, g, b, 255),
            accent,
        }
    }
}

struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    /// `None` when a side is zero.
    fn new(width: u32, height: u32) -> Option<Self> {
        Pixmap::new(width, height).map(|pixmap| Self { pixmap })
    }

    /// A transparent canvas of the same size.
    fn blank_layer(&self) -> Canvas {
        let mut pixmap = self.pixmap.clone();
        pixmap.fill(Color::TRANSPARENT);
        Canvas { pixmap }
    }

    fn fill(&mut self, path: &Path, color: Color, transform: Transform) {
        self.pixmap
            .fill_path(path, &paint(color), FillRule::Winding, transform, None);
    }

    /// Draws a same-sized layer over this canvas at `opacity`.
    fn composite(&mut self, layer: &Canvas, opacity: f32) {
        let paint = PixmapPaint {
            opacity,
            ..PixmapPaint::default()
        };
        self.pixmap.draw_pixmap(
            0,
            0,
            layer.pixmap.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
    }

    /// Draws `icon` in the `size` square at (`x`, `y`); with a `tint` only its shape is kept,
    /// filled with that flat colour.
    fn draw_icon(&mut self, icon: &SvgIcon, x: f32, y: f32, size: f32, tint: Option<Color>) {
        // The icon is rendered on its own box-sized pixmap, so content outside its viewBox is
        // clipped there; one extra pixel keeps the sub-pixel offset of (`x`, `y`).
        let (left, top) = (x.floor(), y.floor());
        let side = size.ceil() as u32 + 1;
        let Some(mut box_pixmap) = Pixmap::new(side, side) else {
            return;
        };
        icon.render(&mut box_pixmap, x - left, y - top, size);
        if let Some(color) = tint {
            let shape = Mask::from_pixmap(box_pixmap.as_ref(), MaskType::Alpha);
            box_pixmap.fill(color);
            box_pixmap.apply_mask(&shape);
        }
        self.pixmap.draw_pixmap(
            left as i32,
            top as i32,
            box_pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draws the gauge with its top-left corner at (`x`, `y`): `outline` ink frame and cap, `fill_color` level.
    fn draw_gauge(&mut self, shape: &Gauge, x: f32, y: f32, outline: Color, fill_color: Color) {
        let at = Transform::from_translate(x, y);
        let stroke = Stroke {
            width: shape.outline_width,
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(&shape.frame, &paint(outline), &stroke, at, None);
        self.fill(&shape.cap, outline, at);
        if let Some(level) = &shape.fill {
            self.fill(level, fill_color, at);
        }
    }

    /// tiny-skia stores premultiplied pixels; the bitmap hands out straight alpha.
    fn into_bitmap(self) -> Bitmap {
        let rgba = self
            .pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let c = pixel.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        Bitmap {
            width: self.pixmap.width(),
            height: self.pixmap.height(),
            rgba,
        }
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    const TEST_HEIGHT: u32 = 44;
    const RECT_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20" viewBox="0 0 10 20"><rect width="10" height="20" fill="red"/></svg>"#;

    fn segment(style: SegmentStyle) -> Segment {
        Segment {
            caption: None,
            text: "42%".to_owned(),
            style,
            icon: IconChoice::Kind(DeviceKind::Headset),
            color_light: IconColor::Auto,
            color_dark: IconColor::Auto,
            gauge: Some(42),
        }
    }

    fn strip(segments: &[Segment], height: u32, theme: Theme) -> Bitmap {
        render_strip(
            segments,
            height,
            theme,
            &HashMap::new(),
            &TextStyle::fallback(height),
        )
    }

    fn svg_strip(segment: &Segment, theme: Theme) -> Bitmap {
        let svgs = HashMap::from([("t.svg".to_owned(), SvgIcon::parse(RECT_SVG).unwrap())]);
        render_strip(
            std::slice::from_ref(segment),
            TEST_HEIGHT,
            theme,
            &svgs,
            &TextStyle::fallback(TEST_HEIGHT),
        )
    }

    fn svg_segment(color: IconColor) -> Segment {
        Segment {
            icon: IconChoice::Svg("t.svg".to_owned()),
            color_light: color,
            color_dark: color,
            ..segment(SegmentStyle::Normal)
        }
    }

    fn has_pixel(bitmap: &Bitmap, matches: impl Fn(u8, u8, u8) -> bool) -> bool {
        bitmap
            .pixels()
            .iter()
            .any(|p| p[3] > 200 && matches(p[0], p[1], p[2]))
    }

    /// Pixels in the columns `x0..x1` and rows `y0..y1`.
    fn region(bitmap: &Bitmap, x0: u32, x1: u32, y0: u32, y1: u32) -> Vec<[u8; 4]> {
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| bitmap.pixels()[(y * bitmap.width + x) as usize])
            .collect()
    }

    /// Left edge of the gauge of a lone segment without caption.
    fn gauge_left(height: f32) -> f32 {
        (EDGE_PADDING + ICON_SIZE + PART_GAP) * height
    }

    /// Columns and rows strictly inside the gauge's fill area, one pixel of margin for anti-aliasing.
    fn gauge_inner(height: f32) -> (u32, u32, u32, u32) {
        let radius = gauge::CORNER_RADIUS * height;
        let left = gauge_left(height) + radius + 1.0;
        let right = gauge_left(height) + gauge::WIDTH * height - radius - 1.0;
        let top = (1.0 - gauge::HEIGHT) * height / 2.0 + gauge::CAP_HEIGHT * height + radius + 1.0;
        let bottom = (1.0 + gauge::HEIGHT) * height / 2.0 - radius - 1.0;
        (left as u32, right as u32, top as u32, bottom as u32)
    }

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "juicetray-strip-render-{}-{name}",
            std::process::id()
        ));
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Rows (first, last) holding the digit `0` of a segment, found by diffing against the
    /// same segment without a number.
    fn number_rows(style: &TextStyle) -> (u32, u32) {
        let with = Segment {
            text: "0".to_owned(),
            ..segment(SegmentStyle::Normal)
        };
        let without = Segment {
            text: String::new(),
            ..segment(SegmentStyle::Normal)
        };
        let render = |s: &Segment| {
            render_strip(
                std::slice::from_ref(s),
                TEST_HEIGHT,
                Theme::Light,
                &HashMap::new(),
                style,
            )
        };
        let (bare, full) = (render(&without), render(&with));
        // The number starts a part gap after where the bare segment's content ends.
        let first_column = (bare.width as f32 - EDGE_PADDING * TEST_HEIGHT as f32) as u32 + 1;
        let rows: Vec<u32> = (0..TEST_HEIGHT)
            .filter(|&y| {
                region(&full, first_column, full.width, y, y + 1)
                    .iter()
                    .any(|p| p[3] > 64)
            })
            .collect();
        (*rows.first().unwrap(), *rows.last().unwrap())
    }

    fn embedded_style(size_px: f32) -> TextStyle {
        TextStyle {
            font: FontData::Embedded,
            size_px,
        }
    }

    #[test]
    fn width_grows_with_segments() {
        let one = [segment(SegmentStyle::Normal)];
        let two = [segment(SegmentStyle::Normal), segment(SegmentStyle::Normal)];
        let three = [
            segment(SegmentStyle::Normal),
            segment(SegmentStyle::Normal),
            segment(SegmentStyle::Normal),
        ];
        let widths: Vec<u32> = [&one[..], &two[..], &three[..]]
            .iter()
            .map(|s| strip(s, 44, Theme::Light).width)
            .collect();
        assert!(widths[0] < widths[1] && widths[1] < widths[2], "{widths:?}");
    }

    #[test]
    fn height_is_requested() {
        for height in [24, 44, 96] {
            let bitmap = strip(&[segment(SegmentStyle::Normal)], height, Theme::Dark);
            assert_eq!(bitmap.height, height);
            assert_eq!(
                bitmap.rgba.len(),
                (bitmap.width * bitmap.height * 4) as usize
            );
        }
        let square = render_square(&segment(SegmentStyle::Normal), 32, Theme::Light);
        assert_eq!((square.width, square.height), (32, 32));
    }

    #[test]
    fn not_blank() {
        let bitmap = strip(&[segment(SegmentStyle::Normal)], 44, Theme::Light);
        assert!(bitmap.pixels().iter().any(|p| p[3] > 0));
    }

    #[test]
    fn percent_off_is_narrower() {
        let off = Segment {
            text: String::new(),
            ..segment(SegmentStyle::Normal)
        };
        let on_width = strip(&[segment(SegmentStyle::Normal)], 44, Theme::Light).width;
        let off_width = strip(&[off], 44, Theme::Light).width;
        assert!(off_width * 100 < on_width * 84, "{off_width} vs {on_width}");
    }

    #[test]
    fn low_segment_has_red_pixels() {
        let bitmap = strip(&[segment(SegmentStyle::Low)], 44, Theme::Light);
        assert!(has_pixel(&bitmap, |r, g, b| r > 180 && g < 90 && b < 90));
    }

    #[test]
    fn full_segment_has_green_pixels() {
        let full = Segment {
            gauge: Some(100),
            ..segment(SegmentStyle::Full)
        };
        let bitmap = strip(&[full], 44, Theme::Dark);
        assert!(has_pixel(&bitmap, |r, g, b| g > 140 && r < 90 && b < 140));
    }

    #[test]
    fn charging_segment_has_green_pixels() {
        let bitmap = strip(&[segment(SegmentStyle::Charging)], 44, Theme::Light);
        assert!(has_pixel(&bitmap, |r, g, b| g > 120 && r < 90 && b < 130));
    }

    #[test]
    fn medium_segment_has_yellow_pixels_on_both_themes() {
        let medium = Segment {
            gauge: Some(40),
            ..segment(SegmentStyle::Medium)
        };
        let light = strip(std::slice::from_ref(&medium), 44, Theme::Light);
        assert!(has_pixel(&light, |r, g, b| r > 150 && g > 100 && b < 40));
        let dark = strip(&[medium], 44, Theme::Dark);
        assert!(has_pixel(&dark, |r, g, b| r > 230 && g > 180 && b < 100));
    }

    /// WCAG 2.x contrast ratio of two sRGB colours.
    fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let luminance = |(r, g, b): (u8, u8, u8)| {
            let linear = |channel: u8| {
                let value = f64::from(channel) / 255.0;
                if value <= 0.03928 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        };
        let (hi, lo) = (
            luminance(a).max(luminance(b)),
            luminance(a).min(luminance(b)),
        );
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn accent_shades_read_on_their_bar() {
        let (light_bar, dark_bar) = ((0xEE, 0xEE, 0xEE), (0x1F, 0x1F, 0x1F));
        for (name, (light, dark)) in [
            ("red", ACCENT_RED),
            ("yellow", ACCENT_YELLOW),
            ("green", ACCENT_GREEN),
        ] {
            assert!(contrast(light, light_bar) >= 3.0, "{name} on light bar");
            assert!(contrast(dark, dark_bar) >= 3.0, "{name} on dark bar");
        }
    }

    #[test]
    fn gauge_low_fill_is_red() {
        let low = Segment {
            gauge: Some(10),
            ..segment(SegmentStyle::Low)
        };
        let bitmap = strip(&[low], 44, Theme::Light);
        let (x0, x1, y0, y1) = gauge_inner(44.0);
        let inner: Vec<[u8; 4]> = region(&bitmap, x0, x1, y0, y1)
            .into_iter()
            .filter(|p| p[3] > 200)
            .collect();
        assert!(!inner.is_empty());
        assert!(inner.iter().all(|p| p[0] > 180 && p[1] < 90 && p[2] < 90));
    }

    #[test]
    fn gauge_zero_draws_outline_only() {
        let empty = Segment {
            gauge: Some(0),
            ..segment(SegmentStyle::Normal)
        };
        let bitmap = strip(&[empty], 44, Theme::Light);
        let (x0, x1, y0, y1) = gauge_inner(44.0);
        let inner = region(&bitmap, x0, x1, y0, y1);
        assert!(inner.iter().all(|p| p[3] == 0));
        // The outline's left edge is there: halfway down, one pixel inside the gauge.
        let edge = region(
            &bitmap,
            gauge_left(44.0) as u32 + 1,
            gauge_left(44.0) as u32 + 2,
            22,
            23,
        );
        assert!(edge[0][3] > 200);
    }

    #[test]
    fn gauge_fill_is_proportional() {
        let filled = |level| {
            let s = Segment {
                gauge: Some(level),
                ..segment(SegmentStyle::Normal)
            };
            let bitmap = strip(&[s], 44, Theme::Light);
            let (x0, x1, y0, y1) = gauge_inner(44.0);
            region(&bitmap, x0, x1, y0, y1)
                .iter()
                .filter(|p| p[3] > 200)
                .count()
        };
        let (low, high) = (filled(20), filled(80));
        assert!(low > 0 && high > 3 * low, "{low} vs {high}");
    }

    #[test]
    fn placeholder_renders() {
        let placeholder = Segment {
            caption: None,
            text: "–".to_owned(),
            style: SegmentStyle::Placeholder,
            icon: IconChoice::Kind(DeviceKind::Other),
            color_light: IconColor::Auto,
            color_dark: IconColor::Auto,
            gauge: None,
        };
        let bitmap = strip(&[placeholder], 44, Theme::Light);
        assert!(bitmap.width > 0);
        assert!(bitmap.pixels().iter().any(|p| p[3] > 0));
    }

    #[test]
    fn output_is_straight_alpha() {
        // Dimmed white ink is about 45 % alpha, even where gauge fill and outline overlap; straight
        // output keeps its colour at full white.
        let bitmap = strip(&[segment(SegmentStyle::Dimmed)], 44, Theme::Dark);
        let max_alpha = bitmap.pixels().iter().map(|p| p[3]).max().unwrap();
        assert!((100..=125).contains(&max_alpha), "max alpha {max_alpha}");
        for p in bitmap.pixels().iter().filter(|p| p[3] >= 60) {
            assert!(
                p[..3].iter().all(|&c| c >= 245),
                "premultiplied pixel {p:?}"
            );
        }
    }

    #[test]
    fn every_kind_has_a_builtin_icon() {
        use DeviceKind::*;
        for kind in [Headset, Earbuds, Mouse, Keyboard, Gamepad, Other] {
            let icon = Segment {
                icon: IconChoice::Kind(kind),
                gauge: None,
                text: String::new(),
                ..segment(SegmentStyle::Normal)
            };
            let bitmap = strip(&[icon], 44, Theme::Light);
            assert!(has_pixel(&bitmap, |_, _, _| true), "{kind:?} draws nothing");
        }
    }

    #[test]
    fn builtin_icon_takes_the_chosen_colour() {
        let blue = Segment {
            icon: IconChoice::Kind(DeviceKind::Mouse),
            color_light: IconColor::Rgb(0x3E, 0x8E, 0xED),
            color_dark: IconColor::Rgb(0x3E, 0x8E, 0xED),
            ..segment(SegmentStyle::Normal)
        };
        let bitmap = strip(&[blue], 44, Theme::Light);
        let icon_end = ((EDGE_PADDING + ICON_SIZE) * 44.0) as u32;
        let icon = region(&bitmap, 0, icon_end, 0, 44);
        let blue_channels = [0x3E_u8, 0x8E, 0xED];
        // Partly covered pixels round-trip through 8-bit premultiplied storage, so they match
        // within 2; fully opaque pixels carry the exact colour.
        let strong: Vec<_> = icon.iter().filter(|p| p[3] > 200).collect();
        assert!(strong.iter().all(|p| {
            p[..3]
                .iter()
                .zip(blue_channels)
                .all(|(&got, want)| got.abs_diff(want) <= 2)
        }));
        let opaque: Vec<_> = icon.iter().filter(|p| p[3] == 255).collect();
        assert!(!opaque.is_empty());
        assert!(opaque.iter().all(|p| p[..3] == blue_channels));
    }

    #[test]
    fn icon_colour_follows_the_theme() {
        let split = Segment {
            color_light: IconColor::Rgb(0xE5, 0x48, 0x4D),
            color_dark: IconColor::Rgb(0x3E, 0x8E, 0xED),
            ..svg_segment(IconColor::Auto)
        };
        let is_red = |r: u8, g: u8, b: u8| r > 200 && g < 100 && b < 100;
        let is_blue = |r: u8, g: u8, b: u8| r < 100 && g > 100 && b > 200;
        let light = svg_strip(&split, Theme::Light);
        assert!(has_pixel(&light, is_red) && !has_pixel(&light, is_blue));
        let dark = svg_strip(&split, Theme::Dark);
        assert!(has_pixel(&dark, is_blue) && !has_pixel(&dark, is_red));
    }

    #[test]
    fn svg_icon_renders() {
        let bitmap = svg_strip(&svg_segment(IconColor::Auto), Theme::Light);
        let icon_start = (EDGE_PADDING * 44.0) as u32;
        let icon_end = ((EDGE_PADDING + ICON_SIZE) * 44.0).ceil() as u32;
        let columns: Vec<u32> = (icon_start..icon_end)
            .filter(|&x| region(&bitmap, x, x + 1, 0, 44).iter().any(|p| p[3] > 128))
            .collect();
        let rows: Vec<u32> = (0..44)
            .filter(|&y| {
                region(&bitmap, icon_start, icon_end, y, y + 1)
                    .iter()
                    .any(|p| p[3] > 128)
            })
            .collect();
        assert!(!columns.is_empty() && !rows.is_empty());
        let width = (columns.last().unwrap() - columns[0] + 1) as i32;
        let height = (rows.last().unwrap() - rows[0] + 1) as i32;
        assert!((width - height / 2).abs() <= 1, "{width} x {height}");
    }

    #[test]
    fn svg_tinted_with_color() {
        let blue = svg_segment(IconColor::Rgb(0x3E, 0x8E, 0xED));
        let bitmap = svg_strip(&blue, Theme::Light);
        assert!(has_pixel(&bitmap, |r, g, b| r < 100 && g > 100 && b > 200));
        assert!(!has_pixel(&bitmap, |r, g, b| r > 200 && g < 60 && b < 60));
    }

    #[test]
    fn svg_original_keeps_colors() {
        let bitmap = svg_strip(&svg_segment(IconColor::Original), Theme::Dark);
        assert!(has_pixel(&bitmap, |r, g, b| r > 200 && g < 60 && b < 60));
    }

    #[test]
    fn svg_overhang_is_clipped_to_icon_box() {
        // A rect far larger than the viewBox must not paint over gauge, number or neighbours.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20" viewBox="0 0 10 20"><rect x="-300" y="-300" width="600" height="600" fill="red"/></svg>"#;
        let svgs = HashMap::from([("t.svg".to_owned(), SvgIcon::parse(svg).unwrap())]);
        let lone_icon = Segment {
            gauge: None,
            text: String::new(),
            ..svg_segment(IconColor::Original)
        };
        let height = TEST_HEIGHT as f32;
        let bitmap = render_strip(
            std::slice::from_ref(&lone_icon),
            TEST_HEIGHT,
            Theme::Light,
            &svgs,
            &TextStyle::fallback(TEST_HEIGHT),
        );
        let (left, right) = (EDGE_PADDING * height, (EDGE_PADDING + ICON_SIZE) * height);
        let top = (1.0 - ICON_SIZE) * height / 2.0;
        let bottom = top + ICON_SIZE * height;
        // Two pixels of margin cover the sub-pixel offset and anti-aliasing.
        let outside = |x: u32, y: u32| {
            let (x, y) = (x as f32, y as f32);
            x < left - 2.0 || x > right + 2.0 || y < top - 2.0 || y > bottom + 2.0
        };
        for y in 0..bitmap.height {
            for x in 0..bitmap.width {
                let pixel = bitmap.pixels()[(y * bitmap.width + x) as usize];
                assert!(
                    !outside(x, y) || pixel[3] == 0,
                    "pixel at {x},{y}: {pixel:?}"
                );
            }
        }
        assert!(has_pixel(&bitmap, |r, g, b| r > 200 && g < 60 && b < 60));
    }

    #[test]
    fn svg_missing_draws_generic_icon() {
        let missing = Segment {
            icon: IconChoice::Svg("absent.svg".to_owned()),
            gauge: None,
            text: String::new(),
            ..segment(SegmentStyle::Normal)
        };
        assert!(has_pixel(
            &strip(&[missing], 44, Theme::Light),
            |_, _, _| true
        ));
    }

    #[test]
    fn broken_svg_rejected() {
        assert!(SvgIcon::parse(b"this is not an svg").is_err());
        let path = temp_file("broken.svg", b"\x00\x01 garbage <<<");
        assert!(load_svg(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn oversized_svg_rejected() {
        let path = temp_file("huge.svg", &vec![b' '; 1024 * 1024 + 1]);
        let error = load_svg(&path).err().expect("must be rejected");
        assert!(error.to_string().contains("1 MB"), "{error:#}");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn svg_at_the_size_limit_loads() {
        let mut bytes = RECT_SVG.to_vec();
        bytes.resize(1024 * 1024, b' ');
        let path = temp_file("limit.svg", &bytes);
        assert!(load_svg(&path).is_ok());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_svg_rejected() {
        let empty = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        assert!(SvgIcon::parse(empty).is_err());
    }

    #[test]
    fn external_image_ignored() {
        // A real SVG file on disk: the default resolver would load it as a nested image.
        let local = temp_file("nested.svg", RECT_SVG);
        let href = local.to_string_lossy().replace('\\', "/");
        let file = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="{href}" width="10" height="10"/></svg>"#
        );
        assert!(SvgIcon::parse(file.as_bytes()).is_err());
        fs::remove_file(local).unwrap();
        // The same square as an SVG data URL: a nested SVG is not loaded either.
        let nested = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxMCIgaGVpZ2h0PSIxMCI+PHJlY3Qgd2lkdGg9IjEwIiBoZWlnaHQ9IjEwIi8+PC9zdmc+" width="10" height="10"/></svg>"#;
        assert!(SvgIcon::parse(nested).is_err());
    }

    #[test]
    fn text_style_size_used() {
        let expected = digit_height(&embedded_font(), 26.0);
        let (first, last) = number_rows(&embedded_style(26.0));
        let drawn = (last - first + 1) as f32;
        assert!((drawn - expected).abs() <= 2.0, "{drawn} vs {expected}");
        // The digits sit on the gauge's vertical centre.
        let centre = (first + last) as f32 / 2.0 + 0.5;
        assert!((centre - 22.0).abs() <= 1.5, "centre {centre}");
    }

    #[test]
    fn text_clamped_to_height() {
        let limit = digit_height(&embedded_font(), MAX_TEXT_SIZE * TEST_HEIGHT as f32);
        let (first, last) = number_rows(&embedded_style(100.0));
        let drawn = (last - first + 1) as f32;
        assert!(drawn <= limit + 2.0, "{drawn} vs {limit}");
        assert!(first > 0 && last < TEST_HEIGHT - 1, "{first}..{last}");
    }

    #[test]
    fn style_without_font_draws_the_icon_but_no_text() {
        let captioned = Segment {
            caption: Some("A".to_owned()),
            ..segment(SegmentStyle::Normal)
        };
        let render = |text: &TextStyle| {
            render_strip(
                std::slice::from_ref(&captioned),
                TEST_HEIGHT,
                Theme::Light,
                &HashMap::new(),
                text,
            )
        };
        let with_text = render(&TextStyle::fallback(TEST_HEIGHT));
        let without_text = render(&TextStyle::without_font(26.0));
        assert!(without_text.width < with_text.width);
        assert!(without_text.pixels().iter().any(|p| p[3] > 0));
    }

    #[test]
    fn fallback_when_font_missing() {
        let style = TextStyle::from_font_file(Path::new("no/such/font.ttf"), 26.0, 44);
        assert!(matches!(style.font, FontData::Embedded));
        assert!((style.size_px - 0.58 * 44.0).abs() < 0.01);

        let garbage = temp_file("garbage.ttf", b"not a font");
        let style = TextStyle::from_font_file(&garbage, 26.0, 44);
        assert!(matches!(style.font, FontData::Embedded));
        fs::remove_file(garbage).unwrap();
    }

    #[test]
    fn font_file_is_mapped() {
        let inter = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/Inter-Medium.ttf");
        let style = TextStyle::from_font_file(Path::new(inter), 26.0, 44);
        assert!(matches!(style.font, FontData::Mapped(_)));
        assert_eq!(style.size_px, 26.0);
        let (first, last) = number_rows(&style);
        assert!(last > first);
    }

    #[test]
    fn square_number_fits_even_for_100() {
        let full = Segment {
            gauge: Some(100),
            ..segment(SegmentStyle::Normal)
        };
        let bitmap = render_square(&full, 32, Theme::Light);
        let columns: Vec<usize> = bitmap
            .pixels()
            .iter()
            .enumerate()
            .filter(|(_, p)| p[3] > 0)
            .map(|(i, _)| i % 32)
            .collect();
        assert!(!columns.is_empty());
        assert!(columns.iter().min() > Some(&0) && columns.iter().max() < Some(&31));
    }

    #[test]
    fn square_shows_number_when_percent_is_off() {
        let off = Segment {
            text: String::new(),
            ..segment(SegmentStyle::Normal)
        };
        let bitmap = render_square(&off, 32, Theme::Light);
        assert!(bitmap.pixels().iter().any(|p| p[3] > 0));
    }
}
