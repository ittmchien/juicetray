//! The Help submenu's targets: project pages in the browser and the log folder in the file
//! manager.

use crate::update::RELEASES_URL;

/// The project's home page.
const REPO_URL: &str = "https://github.com/ittmchien/juicetray";
/// The published device list (Supported and Requested tabs only).
const DEVICE_LIST_URL: &str = "https://docs.google.com/spreadsheets/d/e/2PACX-1vRTKUOHRmpLeXhjxL93T5HwuYsJKnzMHSUhN2AWslNCB938EpQH2YkMeNubTcpvdxpHC2o7Dw4OHfS7/pubhtml";

/// One Help submenu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpLink {
    GitHub,
    /// The release page of the running version.
    WhatsNew,
    Devices,
    /// A new GitHub issue, with the version line filled in on Windows.
    Report,
    Logs,
}

/// Opens `link`; a failure only reaches the log, as there is nothing the user could retry.
pub fn open(link: HelpLink) {
    let Some(target) = target(link) else {
        crate::log::line("help: no log folder on this system");
        return;
    };
    open_target(&target);
}

/// The URL or folder `link` opens; `None` when the OS gives no data folder for the log.
fn target(link: HelpLink) -> Option<String> {
    Some(match link {
        HelpLink::GitHub => REPO_URL.to_owned(),
        HelpLink::WhatsNew => format!("{RELEASES_URL}/tag/v{}", env!("CARGO_PKG_VERSION")),
        HelpLink::Devices => DEVICE_LIST_URL.to_owned(),
        HelpLink::Report => report_url(),
        HelpLink::Logs => crate::log::path()?.parent()?.display().to_string(),
    })
}

#[cfg(windows)]
fn report_url() -> String {
    crate::report::issue_url("", &crate::report::environment())
}

/// macOS builds have no report module; the plain new-issue page is enough there.
#[cfg(not(windows))]
fn report_url() -> String {
    format!("{REPO_URL}/issues/new")
}

#[cfg(windows)]
fn open_target(target: &str) {
    if let Err(error) = crate::update::shell_open(target) {
        crate::log::line(&format!("help: opening {target} failed: {error}"));
    }
}

#[cfg(not(windows))]
fn open_target(target: &str) {
    crate::update::open_url(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_point_at_the_project_and_this_version() {
        assert_eq!(target(HelpLink::GitHub).unwrap(), REPO_URL);
        assert_eq!(
            target(HelpLink::WhatsNew).unwrap(),
            format!(
                "https://github.com/ittmchien/juicetray/releases/tag/v{}",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert!(target(HelpLink::Devices).unwrap().ends_with("/pubhtml"));
        assert!(
            target(HelpLink::Report)
                .unwrap()
                .starts_with("https://github.com/ittmchien/juicetray/issues/new")
        );
    }
}
