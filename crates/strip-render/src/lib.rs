//! Draws battery strip segments to straight-alpha RGBA bitmaps with tiny-skia: vector glyphs plus Inter text.

mod glyphs;

use std::sync::LazyLock;

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, Point};
use glyphs::{Mark, Part, PartStyle, kind_parts, mark_parts};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};
use ui_model::{Segment, SegmentStyle};

// Proportions, all relative to the bitmap height; sized to read clearly in a 22 pt menu bar.
const GLYPH_SIZE: f32 = 0.88;
const CAPTION_SIZE: f32 = 0.38;
const TEXT_SIZE: f32 = 0.78;
const MARK_SIZE: f32 = 0.6;
const MARK_GAP: f32 = 0.05;
/// Space between a segment's glyph/caption and its number.
const INNER_GAP: f32 = 0.15;
/// Space between two segments.
const SEGMENT_GAP: f32 = 0.5;
const EDGE_PADDING: f32 = 0.25;
/// Share of a square icon the number may fill horizontally.
const SQUARE_TEXT_FILL: f32 = 0.9;

/// Opacity of the ink for dimmed and placeholder segments.
const DIMMED_ALPHA: u8 = 115;
/// Fallback only; TrueType fonts always declare their units per em.
const DEFAULT_UNITS_PER_EM: f32 = 1000.0;

static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(include_bytes!("../assets/Inter-Medium.ttf"))
        .expect("embedded Inter font is valid")
});

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

/// Glyph, optional caption and number (plus charging / full mark) of every segment, left to right.
pub fn render_strip(segments: &[Segment], height_px: u32, theme: Theme) -> Bitmap {
    let height = height_px as f32;
    let layouts: Vec<SegmentLayout> = segments
        .iter()
        .map(|segment| SegmentLayout::new(segment, height))
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
            if let Some(mut layer) = Canvas::new(width, height_px) {
                draw_segment(&mut layer, segment, layout, x, height, theme);
                canvas.composite(&layer, f32::from(DIMMED_ALPHA) / 255.0);
            }
        } else {
            draw_segment(&mut canvas, segment, layout, x, height, theme);
        }
        x += layout.width + SEGMENT_GAP * height;
    }
    canvas.into_bitmap()
}

/// Only the number (no `%`, no glyph), shrunk to fit and tinted by the segment style.
pub fn render_square(segment: &Segment, size_px: u32, theme: Theme) -> Bitmap {
    let Some(mut canvas) = Canvas::new(size_px, size_px) else {
        return Bitmap::empty();
    };
    let size = size_px as f32;
    let text = segment.text.trim_end_matches('%');

    // Advance grows linearly with the font size, so one probe layout gives the fitting size.
    let unit_advance = layout_text(text, 1.0).advance;
    let px = if unit_advance > 0.0 {
        (TEXT_SIZE * size).min(SQUARE_TEXT_FILL * size / unit_advance)
    } else {
        TEXT_SIZE * size
    };
    let run = layout_text(text, px);
    let palette = Palette::new(segment.style, theme);
    let origin = Transform::from_translate(
        (size - run.advance) / 2.0,
        size / 2.0 + digit_height(px) / 2.0,
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

/// A segment's text runs and total width at a given bitmap height.
struct SegmentLayout {
    caption: Option<Run>,
    text: Run,
    mark: Option<Mark>,
    width: f32,
}

impl SegmentLayout {
    fn new(segment: &Segment, height: f32) -> Self {
        let caption = segment
            .caption
            .as_deref()
            .map(|caption| layout_text(caption, CAPTION_SIZE * height));
        let text = layout_text(&segment.text, TEXT_SIZE * height);
        let mark = match segment.style {
            SegmentStyle::Charging => Some(Mark::Bolt),
            SegmentStyle::Full => Some(Mark::Check),
            _ => None,
        };

        let mut width = GLYPH_SIZE * height + INNER_GAP * height + text.advance;
        if let Some(caption) = &caption {
            width += caption.advance;
        }
        if mark.is_some() {
            width += (MARK_GAP + MARK_SIZE) * height;
        }
        Self {
            caption,
            text,
            mark,
            width,
        }
    }
}

fn draw_segment(
    canvas: &mut Canvas,
    segment: &Segment,
    layout: &SegmentLayout,
    x: f32,
    height: f32,
    theme: Theme,
) {
    let palette = Palette::new(segment.style, theme);
    let glyph_size = GLYPH_SIZE * height;
    let glyph_top = (height - glyph_size) / 2.0;
    let glyph_bottom = glyph_top + glyph_size;
    let mut cursor = x;

    // The kind glyph is vertically centred; its caption sits at the glyph's bottom right.
    canvas.draw_parts(
        &kind_parts(segment.kind),
        palette.ink,
        cursor,
        glyph_top,
        glyph_size,
    );
    cursor += glyph_size;
    if let Some(caption) = &layout.caption {
        if let Some(path) = &caption.path {
            canvas.fill(
                path,
                palette.ink,
                Transform::from_translate(cursor, glyph_bottom),
            );
        }
        cursor += caption.advance;
    }
    cursor += INNER_GAP * height;

    // Only low batteries colour the number; charging and full are shown by the mark.
    let text_color = match segment.style {
        SegmentStyle::Low => palette.accent.unwrap_or(palette.ink),
        _ => palette.ink,
    };
    let baseline = height / 2.0 + digit_height(TEXT_SIZE * height) / 2.0;
    if let Some(path) = &layout.text.path {
        canvas.fill(
            path,
            text_color,
            Transform::from_translate(cursor, baseline),
        );
    }
    cursor += layout.text.advance;

    if let (Some(mark), Some(color)) = (layout.mark, palette.accent) {
        let mark_size = MARK_SIZE * height;
        let mark_top = (height - mark_size) / 2.0;
        canvas.draw_parts(
            &mark_parts(mark),
            color,
            cursor + MARK_GAP * height,
            mark_top,
            mark_size,
        );
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

impl Palette {
    fn new(style: SegmentStyle, theme: Theme) -> Self {
        let (r, g, b) = match theme {
            Theme::Light => (0, 0, 0),
            Theme::Dark => (255, 255, 255),
        };
        let accent = match style {
            SegmentStyle::Low => Some(Color::from_rgba8(0xE5, 0x48, 0x4D, 255)),
            SegmentStyle::Charging => Some(Color::from_rgba8(0x3E, 0x8E, 0xED, 255)),
            SegmentStyle::Full => Some(Color::from_rgba8(0x30, 0xA4, 0x6C, 255)),
            _ => None,
        };
        Self {
            ink: Color::from_rgba8(r, g, b, 255),
            accent,
        }
    }
}

/// An outline of laid-out text with origin (0, 0) on the baseline, plus its advance width.
struct Run {
    path: Option<Path>,
    advance: f32,
}

/// Lays `text` out left to right at `px` pixels per em, glyph outlines in y-down pixel space.
fn layout_text(text: &str, px: f32) -> Run {
    let font = &*FONT;
    let scale = unit_scale(font, px);
    let mut builder = PathBuilder::new();
    let mut pen = 0.0;
    let mut previous: Option<GlyphId> = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(previous) = previous {
            pen += font.kern_unscaled(previous, id) * scale;
        }
        if let Some(outline) = font.outline(id) {
            add_outline(&mut builder, &outline.curves, pen, scale);
        }
        pen += font.h_advance_unscaled(id) * scale;
        previous = Some(id);
    }
    Run {
        path: builder.finish(),
        advance: pen,
    }
}

/// Appends font-unit curves (y up) as pixel-space contours shifted by `pen`.
fn add_outline(builder: &mut PathBuilder, curves: &[OutlineCurve], pen: f32, scale: f32) {
    let map = |p: &Point| (pen + p.x * scale, -p.y * scale);
    let mut end: Option<&Point> = None;
    for curve in curves {
        let start = match curve {
            OutlineCurve::Line(a, _)
            | OutlineCurve::Quad(a, _, _)
            | OutlineCurve::Cubic(a, _, _, _) => a,
        };
        // A curve that does not continue the previous one begins a new contour.
        if end != Some(start) {
            if end.is_some() {
                builder.close();
            }
            let (x, y) = map(start);
            builder.move_to(x, y);
        }
        end = Some(match curve {
            OutlineCurve::Line(_, p) => {
                let (x, y) = map(p);
                builder.line_to(x, y);
                p
            }
            OutlineCurve::Quad(_, c, p) => {
                let ((cx, cy), (x, y)) = (map(c), map(p));
                builder.quad_to(cx, cy, x, y);
                p
            }
            OutlineCurve::Cubic(_, c1, c2, p) => {
                let ((c1x, c1y), (c2x, c2y), (x, y)) = (map(c1), map(c2), map(p));
                builder.cubic_to(c1x, c1y, c2x, c2y, x, y);
                p
            }
        });
    }
    if end.is_some() {
        builder.close();
    }
}

/// Height of the digit `0` at `px` pixels per em, used to centre numbers vertically.
fn digit_height(px: f32) -> f32 {
    let font = &*FONT;
    // ab_glyph names the bounds y-down: `min.y` is the glyph's top, `max.y` its bottom.
    font.outline(font.glyph_id('0')).map_or(0.0, |outline| {
        (outline.bounds.min.y - outline.bounds.max.y) * unit_scale(font, px)
    })
}

/// Multiplier from font units to pixels at `px` pixels per em.
fn unit_scale(font: &FontRef<'_>, px: f32) -> f32 {
    px / font.units_per_em().unwrap_or(DEFAULT_UNITS_PER_EM)
}

struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    /// `None` when a side is zero.
    fn new(width: u32, height: u32) -> Option<Self> {
        Pixmap::new(width, height).map(|pixmap| Self { pixmap })
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

    /// Draws unit-square parts scaled to `size` pixels with the square's top-left at (`x`, `y`).
    fn draw_parts(&mut self, parts: &[Part], color: Color, x: f32, y: f32, size: f32) {
        let transform = Transform::from_scale(size, size).post_translate(x, y);
        for part in parts {
            match part.style {
                PartStyle::Fill => self.fill(&part.path, color, transform),
                PartStyle::Stroke(width) => {
                    let stroke = Stroke {
                        width,
                        line_cap: LineCap::Round,
                        line_join: LineJoin::Round,
                        ..Stroke::default()
                    };
                    self.pixmap
                        .stroke_path(&part.path, &paint(color), &stroke, transform, None);
                }
            }
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
    use super::*;
    use ui_model::{DeviceKind, IconChoice, IconColor};

    fn segment(style: SegmentStyle) -> Segment {
        Segment {
            kind: DeviceKind::Headset,
            caption: None,
            text: "42%".to_owned(),
            style,
            icon: IconChoice::Kind(DeviceKind::Headset),
            color: IconColor::Auto,
            gauge: None,
        }
    }

    fn has_pixel(bitmap: &Bitmap, matches: impl Fn(u8, u8, u8) -> bool) -> bool {
        bitmap
            .pixels()
            .iter()
            .any(|p| p[3] > 200 && matches(p[0], p[1], p[2]))
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
            .map(|s| render_strip(s, 44, Theme::Light).width)
            .collect();
        assert!(widths[0] < widths[1] && widths[1] < widths[2], "{widths:?}");
    }

    #[test]
    fn height_is_requested() {
        for height in [24, 44, 96] {
            let bitmap = render_strip(&[segment(SegmentStyle::Normal)], height, Theme::Dark);
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
        let bitmap = render_strip(&[segment(SegmentStyle::Normal)], 44, Theme::Light);
        assert!(bitmap.pixels().iter().any(|p| p[3] > 0));
    }

    #[test]
    fn low_segment_has_red_pixels() {
        let bitmap = render_strip(&[segment(SegmentStyle::Low)], 44, Theme::Light);
        assert!(has_pixel(&bitmap, |r, g, b| r > 180 && g < 90 && b < 90));
    }

    #[test]
    fn full_segment_has_green_pixels() {
        let bitmap = render_strip(&[segment(SegmentStyle::Full)], 44, Theme::Dark);
        assert!(has_pixel(&bitmap, |r, g, b| g > 140 && r < 90 && b < 140));
    }

    #[test]
    fn charging_segment_has_blue_pixels() {
        let bitmap = render_strip(&[segment(SegmentStyle::Charging)], 44, Theme::Light);
        assert!(has_pixel(&bitmap, |r, g, b| b > 200 && r < 100 && g < 170));
    }

    #[test]
    fn placeholder_renders() {
        let placeholder = Segment {
            kind: DeviceKind::Other,
            caption: None,
            text: "–".to_owned(),
            style: SegmentStyle::Placeholder,
            icon: IconChoice::Kind(DeviceKind::Other),
            color: IconColor::Auto,
            gauge: None,
        };
        let bitmap = render_strip(&[placeholder], 44, Theme::Light);
        assert!(bitmap.width > 0);
        assert!(bitmap.pixels().iter().any(|p| p[3] > 0));
    }

    #[test]
    fn output_is_straight_alpha() {
        // Dimmed white ink is about 45 % alpha; straight output keeps its colour at full white.
        let bitmap = render_strip(&[segment(SegmentStyle::Dimmed)], 44, Theme::Dark);
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
    fn square_number_fits_even_for_100() {
        let full = Segment {
            text: "100%".to_owned(),
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
}
