use ui_model::DeviceKind;

/// Symbol an AirPods pick falls back to when the device name says no model.
pub const DEFAULT_SYMBOL: &str = "airpods.gen4";

/// The SF Symbol that matches the device's model by name: AirPods Pro / Max / other AirPods,
/// or Beats earbuds; `None` for any other device.
pub fn symbol_for_device(name: &str, kind: DeviceKind) -> Option<&'static str> {
    let name = name.to_lowercase();
    if name.contains("airpods pro") {
        return Some("airpods.pro");
    }
    if name.contains("airpods max") {
        return Some("airpods.max");
    }
    if name.contains("airpods") {
        return Some(DEFAULT_SYMBOL);
    }
    if name.contains("beats") && kind == DeviceKind::Earbuds {
        return Some("beats.earphones");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_follows_model_name() {
        let symbol = |name| symbol_for_device(name, DeviceKind::Earbuds);
        assert_eq!(symbol("Alex's AirPods Pro"), Some("airpods.pro"));
        assert_eq!(symbol("AirPods Max"), Some("airpods.max"));
        assert_eq!(symbol("AirPods"), Some("airpods.gen4"));
        assert_eq!(symbol("Beats Fit Pro"), Some("beats.earphones"));
        assert_eq!(symbol("Galaxy Buds"), None);
    }

    #[test]
    fn beats_headphones_keep_their_kind_icon() {
        assert_eq!(symbol_for_device("Beats Studio", DeviceKind::Headset), None);
        assert_eq!(
            symbol_for_device("AirPods Max", DeviceKind::Headset),
            Some("airpods.max")
        );
    }
}
