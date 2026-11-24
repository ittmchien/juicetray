use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use strip_render::{SvgIcon, load_svg};
use ui_model::{DisplayOptions, IconChoice};

/// Parsed custom SVG icons by path; paths that failed to load are remembered too, so a broken
/// file is read once, not on every redraw.
#[derive(Default)]
pub struct SvgCache {
    /// Shared with the Windows taskbar thread's content; a change copies it only while shared.
    loaded: Arc<HashMap<String, SvgIcon>>,
    failed: HashSet<String>,
}

impl SvgCache {
    /// The options the strip draws with: a custom SVG that cannot load (missing, over 1 MB,
    /// broken) falls back to the device kind's built-in icon.
    pub fn resolve(&mut self, options: DisplayOptions) -> DisplayOptions {
        let IconChoice::Svg(path) = &options.icon else {
            return options;
        };
        if self.is_usable(path) {
            options
        } else {
            DisplayOptions {
                icon: IconChoice::Auto,
                ..options
            }
        }
    }

    pub fn icons(&self) -> &Arc<HashMap<String, SvgIcon>> {
        &self.loaded
    }

    /// Drops what is known about `path`, so the next use reads the file again.
    pub fn forget(&mut self, path: &str) {
        Arc::make_mut(&mut self.loaded).remove(path);
        self.failed.remove(path);
    }

    fn is_usable(&mut self, path: &str) -> bool {
        if self.loaded.contains_key(path) {
            return true;
        }
        if self.failed.contains(path) {
            return false;
        }
        match load_svg(Path::new(path)) {
            Ok(icon) => {
                Arc::make_mut(&mut self.loaded).insert(path.to_owned(), icon);
                true
            }
            Err(error) => {
                eprintln!("SVG icon unusable ({error:#}); using the built-in icon");
                self.failed.insert(path.to_owned());
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ui_model::IconColor;

    use super::*;

    fn svg_options(path: &str) -> DisplayOptions {
        DisplayOptions {
            show_percent: false,
            show_label: true,
            icon: IconChoice::Svg(path.to_owned()),
            color_light: IconColor::Original,
            color_dark: IconColor::Original,
        }
    }

    #[test]
    fn svg_missing_uses_builtin() {
        let mut cache = SvgCache::default();
        let resolved = cache.resolve(svg_options("no/such/icon.svg"));
        assert_eq!(resolved.icon, IconChoice::Auto);
        assert!(!resolved.show_percent);
        assert!(cache.failed.contains("no/such/icon.svg"));
        assert!(cache.icons().is_empty());
        // Remembered as failed: resolved again without reading the file.
        assert_eq!(
            cache.resolve(svg_options("no/such/icon.svg")).icon,
            IconChoice::Auto
        );
    }

    #[test]
    fn svg_loaded_once_and_forgotten() {
        let path =
            std::env::temp_dir().join(format!("juicetray-svg-test-{}.svg", std::process::id()));
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#,
        )
        .unwrap();
        let path_text = path.to_str().unwrap().to_owned();
        let mut cache = SvgCache::default();

        let resolved = cache.resolve(svg_options(&path_text));
        fs::remove_file(&path).unwrap();

        assert_eq!(resolved, svg_options(&path_text));
        assert!(cache.icons().contains_key(&path_text));
        // Cached: the deleted file is not read again.
        assert_eq!(
            cache.resolve(svg_options(&path_text)),
            svg_options(&path_text)
        );
        cache.forget(&path_text);
        assert_eq!(
            cache.resolve(svg_options(&path_text)).icon,
            IconChoice::Auto
        );
    }

    #[test]
    fn builtin_icons_untouched() {
        let mut cache = SvgCache::default();
        let options = DisplayOptions::default();
        assert_eq!(cache.resolve(options.clone()), options);
        assert!(cache.failed.is_empty());
    }
}
