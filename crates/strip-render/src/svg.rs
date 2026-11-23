//! SVG icons: the built-in Material Symbols set and the owner's own files, one rendering path.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, bail};
use resvg::usvg;
use tiny_skia::{Pixmap, Transform};
use ui_model::DeviceKind;

/// Largest SVG file the app accepts.
const MAX_SVG_BYTES: u64 = 1024 * 1024;

/// A parsed SVG that can be drawn at any size.
pub struct SvgIcon {
    tree: usvg::Tree,
}

impl SvgIcon {
    /// Parses SVG text; broken, empty or gzip-compressed input is an error.
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let tree =
            usvg::Tree::from_data(bytes, &no_external_resources()).context("SVG does not parse")?;
        if tree.root().abs_layer_bounding_box().is_none() {
            bail!("SVG draws nothing");
        }
        Ok(Self { tree })
    }

    /// Draws the icon scaled to fit the `size` square at (`x`, `y`), centred, aspect kept.
    pub(crate) fn render(&self, target: &mut Pixmap, x: f32, y: f32, size: f32) {
        let image = self.tree.size();
        let scale = (size / image.width()).min(size / image.height());
        let transform = Transform::from_scale(scale, scale).post_translate(
            x + (size - image.width() * scale) / 2.0,
            y + (size - image.height() * scale) / 2.0,
        );
        resvg::render(&self.tree, transform, &mut target.as_mut());
    }
}

/// Reads and parses an SVG file of at most 1 MB.
pub fn load_svg(path: &Path) -> anyhow::Result<SvgIcon> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut bytes = Vec::new();
    // One byte past the limit tells an oversized file from one that is exactly the limit.
    file.take(MAX_SVG_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("cannot read {}", path.display()))?;
    if bytes.len() as u64 > MAX_SVG_BYTES {
        bail!("{} is larger than 1 MB", path.display());
    }
    SvgIcon::parse(&bytes).with_context(|| path.display().to_string())
}

/// Material Symbols Rounded icon of a device kind, parsed once.
pub(crate) fn builtin_icon(kind: DeviceKind) -> &'static SvgIcon {
    macro_rules! embedded {
        ($file:literal) => {{
            static ICON: LazyLock<SvgIcon> = LazyLock::new(|| {
                SvgIcon::parse(include_bytes!(concat!("../assets/icons/", $file)))
                    .expect("embedded icon is valid")
            });
            &*ICON
        }};
    }
    match kind {
        DeviceKind::Headset => embedded!("headphones.svg"),
        DeviceKind::Earbuds => embedded!("earbuds.svg"),
        DeviceKind::Mouse => embedded!("mouse.svg"),
        DeviceKind::Keyboard => embedded!("keyboard.svg"),
        DeviceKind::Gamepad => embedded!("stadia_controller.svg"),
        DeviceKind::Other => embedded!("bluetooth.svg"),
    }
}

/// Parse options under which no image, file or nested SVG is ever loaded.
fn no_external_resources() -> usvg::Options<'static> {
    usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}
