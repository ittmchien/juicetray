//! Licence texts embedded in the binary, for the `licenses` subcommand.

const GPL: &str = include_str!("../../LICENSE");
const ICONS: &str = include_str!("../../crates/strip-render/assets/icons/LICENSE");
#[cfg(not(windows))]
const FONT: &str = include_str!("../../crates/strip-render/assets/OFL.txt");

/// Project, icon and font licences, each under a heading line, in that order. Inter ships only
/// where it is compiled in; the Windows build uses system fonts.
pub fn licenses_text() -> String {
    let sections = [
        ("Juicetray — GPL-3.0-or-later", GPL),
        ("Material Symbols icons — Apache-2.0", ICONS),
        #[cfg(not(windows))]
        ("Inter font — SIL Open Font License 1.1", FONT),
    ];
    sections
        .map(|(heading, text)| format!("{heading}\n\n{text}"))
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licenses_text_lists_what_ships() {
        let text = licenses_text();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        let gpl = at("GNU GENERAL PUBLIC LICENSE");
        let apache = at("Apache License");
        assert!(gpl < apache);
        // Inter is compiled in everywhere but Windows.
        let ofl = text.find("SIL Open Font License, Version 1.1");
        assert_eq!(ofl.is_some(), cfg!(not(windows)));
        assert!(ofl.is_none_or(|ofl| apache < ofl));
    }
}
