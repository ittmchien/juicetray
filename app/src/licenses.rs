//! Licence texts embedded in the binary, for the `licenses` subcommand.

const GPL: &str = include_str!("../../LICENSE");
const ICONS: &str = include_str!("../../crates/strip-render/assets/icons/LICENSE");
const FONT: &str = include_str!("../../crates/strip-render/assets/OFL.txt");

/// Project, icon and font licences, each under a heading line, in that order.
pub fn licenses_text() -> String {
    [
        ("Juicetray — GPL-3.0-or-later", GPL),
        ("Material Symbols icons — Apache-2.0", ICONS),
        ("Inter font — SIL Open Font License 1.1", FONT),
    ]
    .map(|(heading, text)| format!("{heading}\n\n{text}"))
    .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licenses_text_has_all_three() {
        let text = licenses_text();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        let gpl = at("GNU GENERAL PUBLIC LICENSE");
        let apache = at("Apache License");
        let ofl = at("SIL Open Font License, Version 1.1");
        assert!(gpl < apache && apache < ofl);
    }
}
