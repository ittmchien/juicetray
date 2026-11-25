use std::collections::HashMap;
use std::ptr;

use objc2::AllocAnyThread;
use objc2_app_kit::{
    NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use strip_render::Pixmap;
use ui_model::{DeviceView, DisplayOptions, IconChoice};

use crate::symbols::symbol_for_device;

/// Tried when a missing `airpods*` symbol is requested (older macOS lacks the newer glyphs).
const FALLBACK_SYMBOL: &str = "airpods";

/// System symbols rasterised once at the icon's pixel size, keyed by requested name.
/// `None` marks a symbol that renders nowhere, so the strip draws Earbuds without retrying.
pub struct SymbolCache {
    px: u32,
    images: HashMap<String, Option<Pixmap>>,
}

impl SymbolCache {
    pub fn new(px: u32) -> Self {
        Self {
            px,
            images: HashMap::new(),
        }
    }

    /// The options the strip draws with: `Auto` on an AirPods or Beats device becomes its model's
    /// symbol, and any symbol the strip will draw is rendered into the cache first.
    pub fn resolve(&mut self, device: &DeviceView, options: DisplayOptions) -> DisplayOptions {
        match &options.icon {
            IconChoice::Symbol(name) => {
                self.ensure(name);
                options
            }
            IconChoice::Auto => match symbol_for_device(&device.name, device.kind) {
                Some(name) => {
                    self.ensure(name);
                    DisplayOptions {
                        icon: IconChoice::Symbol(name.to_owned()),
                        ..options
                    }
                }
                None => options,
            },
            _ => options,
        }
    }

    pub fn images(&self) -> &HashMap<String, Option<Pixmap>> {
        &self.images
    }

    /// Renders `name` once and remembers the outcome, `None` included; only a missing `airpods*`
    /// symbol falls back to the generic AirPods glyph, any other falls to the Earbuds icon.
    fn ensure(&mut self, name: &str) {
        if self.images.contains_key(name) {
            return;
        }
        let image = rasterize(name, self.px).or_else(|| {
            name.starts_with("airpods")
                .then(|| rasterize(FALLBACK_SYMBOL, self.px))
                .flatten()
        });
        if image.is_none() {
            eprintln!("system symbol {name} unavailable; using the built-in icon");
        }
        self.images.insert(name.to_owned(), image);
    }
}

/// The symbol drawn aspect-fit and centred in a `px` square; the alpha channel is the shape.
/// `None` when the system has no such symbol or the bitmap cannot be created.
fn rasterize(name: &str, px: u32) -> Option<Pixmap> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let side = px as isize;
    // SAFETY: null planes ask AppKit to allocate the pixel buffer itself.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;

    let natural = image.size();
    if natural.width <= 0.0 || natural.height <= 0.0 {
        return None;
    }
    let edge = px as f64;
    let scale = (edge / natural.width).min(edge / natural.height);
    let (width, height) = (natural.width * scale, natural.height * scale);
    let target = NSRect::new(
        NSPoint::new((edge - width) / 2.0, (edge - height) / 2.0),
        NSSize::new(width, height),
    );
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        target,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
    NSGraphicsContext::restoreGraphicsState_class();

    // Only the alpha channel is kept, stored as premultiplied black, so the `Pixmap` is valid
    // whatever the bitmap's colour layout; the strip uses the alpha as a tint mask.
    let data = rep.bitmapData();
    if data.is_null() {
        return None;
    }
    let row_len = px as usize * 4;
    let stride = usize::try_from(rep.bytesPerRow()).ok()?;
    if stride < row_len {
        return None;
    }
    let mut pixmap = Pixmap::new(px, px)?;
    for (row, target_row) in pixmap.data_mut().chunks_exact_mut(row_len).enumerate() {
        // SAFETY: the rep holds `px` rows of `stride >= row_len` bytes behind `data`.
        let source = unsafe { std::slice::from_raw_parts(data.add(row * stride), row_len) };
        let pixels = source.as_chunks::<4>().0;
        for (target, pixel) in target_row.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
            *target = [0, 0, 0, pixel[3]];
        }
    }
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use ui_model::{DeviceKind, Level, Path};

    use super::*;

    #[test]
    fn airpods_symbol_rasterizes_to_a_partial_alpha_mask() {
        let Some(image) = rasterize("airpods", 36) else {
            // The symbol set is part of the OS; a host without it has nothing to check.
            return;
        };
        let alphas: Vec<u8> = image.pixels().iter().map(|pixel| pixel.alpha()).collect();
        assert!(alphas.iter().any(|&alpha| alpha > 200));
        assert!(alphas.contains(&0));
    }

    fn earbuds(name: &str) -> DeviceView {
        DeviceView {
            id: "bt:1".to_owned(),
            name: name.to_owned(),
            kind: DeviceKind::Earbuds,
            path: Path::Bluetooth,
            level: Level::Percent(50),
            charging: false,
            connected: true,
            updated_at: SystemTime::UNIX_EPOCH,
            stale: false,
            note: None,
        }
    }

    #[test]
    fn resolve_maps_auto_to_the_model_symbol_and_caches_unknown_as_none() {
        let mut cache = SymbolCache::new(36);
        let auto = cache.resolve(&earbuds("AirPods Pro"), DisplayOptions::default());
        assert_eq!(auto.icon, IconChoice::Symbol("airpods.pro".to_owned()));
        assert!(cache.images().contains_key("airpods.pro"));

        let missing = DisplayOptions {
            icon: IconChoice::Symbol("definitely.not.a.symbol".to_owned()),
            ..DisplayOptions::default()
        };
        let resolved = cache.resolve(&earbuds("AirPods Pro"), missing.clone());
        assert_eq!(resolved, missing);
        assert!(matches!(
            cache.images().get("definitely.not.a.symbol"),
            Some(None)
        ));
    }

    #[test]
    fn unknown_symbol_is_not_rendered() {
        assert!(rasterize("definitely.not.a.symbol", 36).is_none());
    }
}
