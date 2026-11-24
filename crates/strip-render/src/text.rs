//! Number and caption text: the font choice (`TextStyle`) and glyph-outline layout.

use std::fs::File;
use std::path::Path as FsPath;
#[cfg(feature = "embedded-font")]
use std::sync::LazyLock;

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, Point};
use memmap2::Mmap;
use tiny_skia::{Path, PathBuilder};

/// Fallback number size, relative to the bitmap height.
const FALLBACK_TEXT_SIZE: f32 = 0.58;
/// Fallback only; TrueType fonts always declare their units per em.
const DEFAULT_UNITS_PER_EM: f32 = 1000.0;

#[cfg(feature = "embedded-font")]
static EMBEDDED_FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(include_bytes!("../assets/Inter-Medium.ttf"))
        .expect("embedded Inter font is valid")
});

/// Where the number's font comes from.
#[derive(Debug)]
pub enum FontData {
    /// Inter Medium, compiled in (feature `embedded-font`).
    #[cfg(feature = "embedded-font")]
    Embedded,
    /// A read-only memory-mapped system font file, already checked to parse.
    Mapped(Mmap),
    /// No usable font: numbers and captions are left out, icons and gauges still draw.
    Missing,
}

/// Font and size of the strip's numbers and captions.
#[derive(Debug)]
pub struct TextStyle {
    pub font: FontData,
    /// Pixels per em; the renderer clamps it so the number never overflows the strip.
    pub size_px: f32,
}

impl TextStyle {
    /// The number size used when the OS gives none: 0.58 × the bitmap height.
    pub fn default_size_px(height_px: u32) -> f32 {
        FALLBACK_TEXT_SIZE * height_px as f32
    }

    /// Embedded Inter Medium at the default size.
    #[cfg(feature = "embedded-font")]
    pub fn fallback(height_px: u32) -> Self {
        Self {
            font: FontData::Embedded,
            size_px: Self::default_size_px(height_px),
        }
    }

    /// A style without a font; the strip draws no text.
    pub fn without_font(size_px: f32) -> Self {
        Self {
            font: FontData::Missing,
            size_px,
        }
    }

    /// Memory-maps the font at `path` at `size_px`; the reason when the file is missing or
    /// unparsable.
    pub fn try_from_font_file(path: &FsPath, size_px: f32) -> Result<Self, String> {
        map_font(path).map(|map| Self {
            font: FontData::Mapped(map),
            size_px,
        })
    }

    /// `try_from_font_file`; a missing or unparsable file gives `fallback`.
    #[cfg(feature = "embedded-font")]
    pub fn from_font_file(path: &FsPath, size_px: f32, height_px: u32) -> Self {
        match Self::try_from_font_file(path, size_px) {
            Ok(style) => style,
            Err(reason) => {
                eprintln!(
                    "font {} unusable ({reason}); using embedded Inter",
                    path.display()
                );
                Self::fallback(height_px)
            }
        }
    }

    /// The parsed font, `None` for a style without one.
    pub(crate) fn font_ref(&self) -> Option<FontRef<'_>> {
        match &self.font {
            #[cfg(feature = "embedded-font")]
            FontData::Embedded => Some(embedded_font()),
            FontData::Mapped(map) => Some(
                FontRef::try_from_slice(map).expect("mapped font was checked when it was opened"),
            ),
            FontData::Missing => None,
        }
    }
}

#[cfg(feature = "embedded-font")]
pub(crate) fn embedded_font() -> FontRef<'static> {
    EMBEDDED_FONT.clone()
}

fn map_font(path: &FsPath) -> Result<Mmap, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    // SAFETY: the map is read-only and system fonts are not rewritten in place while mapped.
    let map = unsafe { Mmap::map(&file) }.map_err(|error| error.to_string())?;
    FontRef::try_from_slice(&map).map_err(|error| error.to_string())?;
    Ok(map)
}

/// An outline of laid-out text with origin (0, 0) on the baseline, plus its advance width.
pub(crate) struct Run {
    pub path: Option<Path>,
    pub advance: f32,
}

/// Lays `text` out left to right at `px` pixels per em, glyph outlines in y-down pixel space.
pub(crate) fn layout_text(font: &FontRef<'_>, text: &str, px: f32) -> Run {
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
pub(crate) fn digit_height(font: &FontRef<'_>, px: f32) -> f32 {
    // ab_glyph names the bounds y-down: `min.y` is the glyph's top, `max.y` its bottom.
    font.outline(font.glyph_id('0')).map_or(0.0, |outline| {
        (outline.bounds.min.y - outline.bounds.max.y) * unit_scale(font, px)
    })
}

/// Multiplier from font units to pixels at `px` pixels per em.
fn unit_scale(font: &FontRef<'_>, px: f32) -> f32 {
    px / font.units_per_em().unwrap_or(DEFAULT_UNITS_PER_EM)
}
