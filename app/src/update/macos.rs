//! macOS side of the update check: the GitHub request through `curl`, the offer dialog, the
//! Homebrew upgrade that replaces the running copy and the launch-time move to /Applications.
//! Dialogs run on the main thread; the request and the upgrade run on their own threads.

use std::fs::{self, File};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use objc2_foundation::{NSDate, NSDateFormatter, NSString};
use tao::event_loop::EventLoopProxy;

use super::check::UpdateInfo;
use super::common::{API_URL, BUSY_MESSAGE, DIALOG_TITLE};
use super::{UPDATED_FLAG, UpdateStage};
use crate::alerts::notify_update_started;
use crate::app::UserEvent;
use crate::config::Config;
use crate::{dialog, instance, log};

const CURL: &str = "/usr/bin/curl";
/// The whole request, connecting to the last byte.
const CURL_MAX_SECONDS: &str = "30";
/// Far above the release document's size; a bigger reply is a wrong server, not an update.
const CURL_MAX_FILESIZE: &str = "524288";
/// Homebrew's usual locations, Apple silicon first; a GUI app has no shell PATH to search.
const BREW_PATHS: [&str; 2] = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
const FORMULA: &str = "ittmchien/tap/juicetray";
/// Where the formula keeps the app, below Homebrew's prefix.
const OPT_APP: &str = "opt/juicetray/Juicetray.app";
const BREW_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const BREW_POLL: Duration = Duration::from_millis(500);
/// Lines of brew's output copied to the log after a failure.
const LOG_TAIL_LINES: usize = 30;
/// Waits up to 60 s (300 x 0.2 s) for process `$2` (this one) to exit, then opens the app at
/// `$1` with the flag `$3`; backgrounded, so the shell returns at once.
const RELAUNCH_SCRIPT: &str = r#"(n=0; while kill -0 "$2" 2>/dev/null; do n=$((n+1)); [ "$n" -ge 300 ] && exit 0; sleep 0.2; done; exec /usr/bin/open "$1" --args "$3") >/dev/null 2>&1 &"#;

/// The folder a copy offers to move itself into, and the bundle it becomes there.
const APPLICATIONS: &str = "/Applications";
const APP_NAME: &str = "Juicetray.app";

/// One install at a time; it stays set after a success, while the app quits.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// GETs the latest-release document through the system `curl`: its HTTP status and body.
/// Redirects are not followed, so a moved repository reads as an HTTP 301 error.
pub(super) fn fetch_latest() -> Result<(u16, String), String> {
    let agent = format!("juicetray/{}", env!("CARGO_PKG_VERSION"));
    let output = Command::new(CURL)
        .args([
            // First, so a user's ~/.curlrc cannot change the request.
            "--disable",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--max-time",
            CURL_MAX_SECONDS,
            "--max-filesize",
            CURL_MAX_FILESIZE,
            "--header",
            "Accept: application/vnd.github+json",
            "--user-agent",
            &agent,
            "--write-out",
            "\n%{http_code}",
            API_URL,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running curl failed: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed: {}", error.trim()));
    }
    parse_curl_reply(&String::from_utf8_lossy(&output.stdout))
}

/// Tells the owner a check or install is under way; the caller is the main thread.
pub(super) fn busy_notice() {
    dialog::notice(DIALOG_TITLE, BUSY_MESSAGE);
}

/// Asks about a newer release on the main thread: a Homebrew install gets "Update now",
/// anything else the download page (a browser download is quarantined).
pub fn offer(info: &UpdateInfo, proxy: &EventLoopProxy<UserEvent>) {
    if INSTALLING.load(Ordering::Relaxed) {
        busy_notice();
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    match find_homebrew(Path::exists) {
        Some(homebrew) => {
            let message = format!(
                "Juicetray v{} is available.\n\nYou have v{current}. Homebrew installs the update \
                 and Juicetray restarts.",
                info.version
            );
            if dialog::confirm(DIALOG_TITLE, &message, "Update now", "Later") {
                spawn_install(homebrew, info.version.clone(), proxy.clone());
            } else {
                later(proxy);
            }
        }
        None => {
            let message = format!(
                "Juicetray v{} is available.\n\nYou have v{current}.",
                info.version
            );
            if dialog::confirm(DIALOG_TITLE, &message, "Open download page", "Later") {
                super::open_url(&info.url);
            } else {
                later(proxy);
            }
        }
    }
}

/// "Later" holds automatic offers until tomorrow; the event loop stores the date.
fn later(proxy: &EventLoopProxy<UserEvent>) {
    // A closed event loop means the app is exiting.
    let _ = proxy.send_event(UserEvent::UpdateLater);
}

/// Today's local date as `yyyy-MM-dd`, the day a "Later" holds offers for.
pub fn today() -> Option<String> {
    let formatter = NSDateFormatter::new();
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd")));
    Some(formatter.stringFromDate(&NSDate::now()).to_string())
}

/// Offers to copy a bundle running outside Applications into /Applications; true when the copy
/// starts once this one exits. Main thread; "Not now" is remembered in the config.
pub fn offer_move_to_applications() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(source) = bundle_to_move(running_bundle(), home.as_deref()) else {
        return false;
    };
    let mut config = Config::load();
    if !config.ask_move_to_applications {
        return false;
    }
    let message = "Move Juicetray to Applications?\n\nIt then starts from the Applications folder \
                   like your other apps, and Update now replaces that copy.";
    if !dialog::confirm("Juicetray", message, "Move", "Not now") {
        config.ask_move_to_applications = false;
        if let Err(error) = config.save() {
            log::line(&format!("saving the move choice failed: {error:#}"));
        }
        return false;
    }
    let target = Path::new(APPLICATIONS).join(APP_NAME);
    match replace_bundle(&source, &target).and_then(|()| relaunch(&target)) {
        Ok(()) => true,
        Err(reason) => {
            log::line(&format!("moving to Applications failed: {reason}"));
            dialog::notice(
                "Move to Applications failed",
                &format!(
                    "Juicetray keeps running from {}.\n\n{reason}",
                    source.display()
                ),
            );
            false
        }
    }
}

/// The running bundle unless /Applications or `<home>/Applications` is an ancestor of it; a bare
/// binary (no bundle) is never moved.
fn bundle_to_move(running: Option<PathBuf>, home: Option<&Path>) -> Option<PathBuf> {
    let user_applications = home.map(|home| home.join("Applications"));
    running.filter(|bundle| {
        !bundle.ancestors().skip(1).any(|folder| {
            folder == Path::new(APPLICATIONS) || Some(folder) == user_applications.as_deref()
        })
    })
}

/// Upgrades on its own thread, reporting each step as `UserEvent::UpdateProgress`, and answers
/// with `UserEvent::UpdateRestart` once the new copy is waiting to start, or
/// `UserEvent::UpdateFailed` with the reason; this copy keeps running then.
fn spawn_install(homebrew: Homebrew, version: String, proxy: EventLoopProxy<UserEvent>) {
    if INSTALLING.swap(true, Ordering::Relaxed) {
        busy_notice();
        return;
    }
    let running = running_bundle();
    let spawned = thread::Builder::new()
        .name("update-install".into())
        .spawn(move || {
            let _span = log::span("update install");
            notify_update_started(&version);
            // A closed event loop means the app is exiting, so a lost step changes nothing.
            let step = |stage| {
                let _ = proxy.send_event(UserEvent::UpdateProgress(Some(stage)));
            };
            step(UpdateStage::Homebrew(version.clone()));
            let event = match install(&homebrew, running.as_deref(), || {
                step(UpdateStage::Installing(version.clone()));
            }) {
                Ok(()) => UserEvent::UpdateRestart,
                Err(reason) => {
                    log::line(&format!("update failed: {reason}"));
                    INSTALLING.store(false, Ordering::Relaxed);
                    UserEvent::UpdateFailed(reason)
                }
            };
            // A closed event loop means the app is exiting anyway.
            let _ = proxy.send_event(event);
        });
    if let Err(error) = spawned {
        INSTALLING.store(false, Ordering::Relaxed);
        eprintln!("update install thread failed: {error}");
    }
}

/// `brew upgrade`, then (after `installing` is told) the copy the owner runs is replaced unless
/// it is Homebrew's own, and the new one is started once this process exits.
fn install(
    homebrew: &Homebrew,
    running: Option<&Path>,
    installing: impl FnOnce(),
) -> Result<(), String> {
    brew_upgrade(&homebrew.brew)?;
    installing();
    let app = homebrew.app();
    let plist = fs::read_to_string(app.join("Contents/Info.plist"))
        .map_err(|error| format!("reading the installed app's version failed: {error}"))?;
    let installed = bundle_version(&plist).ok_or("the installed app has no version")?;
    // An unchanged version means the tap has no newer formula yet; restarting would change nothing.
    if installed == env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "Homebrew still has v{installed}; the new version may not be in the tap yet. Try \
             again later."
        ));
    }
    let plan = plan_relaunch(running, homebrew);
    if let Some(target) = &plan.replace {
        replace_bundle(&app, target)?;
    }
    relaunch(&plan.launch)
}

/// The running `.app`; `None` for a bare binary (a development build).
fn running_bundle() -> Option<PathBuf> {
    instance::is_bundled().then(|| {
        PathBuf::from(
            objc2_foundation::NSBundle::mainBundle()
                .bundlePath()
                .to_string(),
        )
    })
}

/// Runs `brew upgrade` in its own process group with its output in a temp file; after
/// `BREW_TIMEOUT` the whole group is killed. A failure logs the output's tail.
fn brew_upgrade(brew: &Path) -> Result<(), String> {
    let output_path = std::env::temp_dir().join("juicetray-brew-upgrade.log");
    let open_error =
        |error: io::Error| format!("creating {} failed: {error}", output_path.display());
    let stdout = File::create(&output_path).map_err(open_error)?;
    let stderr = stdout.try_clone().map_err(open_error)?;
    let mut child = Command::new(brew)
        .args(["upgrade", FORMULA])
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|error| format!("running brew failed: {error}"))?;
    let deadline = Instant::now() + BREW_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(BREW_POLL),
            outcome => {
                // Killing an exited child fails harmlessly; `wait` reaps it either way.
                kill_group(&mut child);
                let _ = child.wait();
                log_brew_output(&output_path);
                return Err(match outcome {
                    Err(error) => format!("waiting for brew failed: {error}"),
                    _ => "brew upgrade took longer than 10 minutes".to_owned(),
                });
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    let last_line = log_brew_output(&output_path);
    Err(format!(
        "brew upgrade failed ({status}){}",
        last_line.map_or_else(String::new, |line| format!(": {line}"))
    ))
}

/// Stops brew and everything it spawned: the group id is the child's pid (`process_group(0)`).
fn kill_group(child: &mut Child) {
    let group = format!("-{}", child.id());
    if let Err(error) = Command::new("/bin/kill")
        .args(["-TERM", "--", &group])
        .status()
    {
        log::line(&format!("stopping brew's processes failed: {error}"));
    }
    let _ = child.kill();
}

/// Copies the tail of brew's output to the log and returns its last non-empty line.
fn log_brew_output(path: &Path) -> Option<String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            log::line(&format!("reading brew's output failed: {error}"));
            return None;
        }
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(LOG_TAIL_LINES)..];
    log::line(&format!("brew output: {}", tail.join(" | ")));
    tail.last().map(|line| (*line).to_owned())
}

/// Copies `source` next to `target` first and only then swaps the two by renaming, so a failed
/// copy never leaves the owner without an app; a missing `target` is simply created.
fn replace_bundle(source: &Path, target: &Path) -> Result<(), String> {
    let (staged, backup) = swap_paths(target);
    remove_tree(&staged)?;
    remove_tree(&backup)?;
    let copied = Command::new("/usr/bin/ditto")
        .arg(source)
        .arg(&staged)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("running ditto failed: {error}"));
    let copied = copied.and_then(|status| {
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("copying the new app failed ({status})"))
    });
    if let Err(error) = copied {
        discard(&staged);
        return Err(error);
    }
    let has_previous = fs::symlink_metadata(target).is_ok();
    if has_previous && let Err(error) = fs::rename(target, &backup) {
        discard(&staged);
        return Err(format!("moving {} aside failed: {error}", target.display()));
    }
    if let Err(error) = fs::rename(&staged, target) {
        discard(&staged);
        if !has_previous {
            return Err(format!("putting the new app in place failed: {error}"));
        }
        // The previous copy goes back, so the app is never missing.
        return Err(match fs::rename(&backup, target) {
            Ok(()) => format!("putting the new app in place failed: {error}"),
            Err(restore) => format!(
                "putting the new app in place failed: {error}; the previous app is at {} \
                 ({restore})",
                backup.display()
            ),
        });
    }
    // The previous copy is a leftover now; failing to delete it only costs disk space.
    if has_previous {
        discard(&backup);
    }
    Ok(())
}

/// Deletes a staged or backup copy, logging (not failing) when that does not work.
fn discard(path: &Path) {
    if let Err(error) = remove_tree(path) {
        log::line(&error);
    }
}

/// Deletes a directory tree; one that does not exist is already gone.
fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(format!("removing {} failed: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Starts `app` with `UPDATED_FLAG` once this process has exited, from a shell in its own
/// process group so it outlives this process.
fn relaunch(app: &Path) -> Result<(), String> {
    let status = Command::new("/bin/sh")
        .args(["-c", RELAUNCH_SCRIPT, "sh"])
        .arg(app)
        .arg(std::process::id().to_string())
        .arg(UPDATED_FLAG)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("scheduling the restart failed: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("scheduling the restart failed ({status})"))
}

/// A Homebrew install of the app: the `brew` to run and the prefix it lives under.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Homebrew {
    brew: PathBuf,
    prefix: PathBuf,
}

impl Homebrew {
    /// The app the formula installs; after an upgrade it is the new version.
    fn app(&self) -> PathBuf {
        self.prefix.join(OPT_APP)
    }
}

/// The first Homebrew whose `brew` exists and that has the formula's app installed; `None`
/// means the app did not come from Homebrew, so only the download page is offered.
fn find_homebrew(exists: impl Fn(&Path) -> bool) -> Option<Homebrew> {
    BREW_PATHS.iter().map(Path::new).find_map(|brew| {
        let homebrew = Homebrew {
            brew: brew.to_owned(),
            prefix: brew.parent()?.parent()?.to_owned(),
        };
        (exists(brew) && exists(&homebrew.app())).then_some(homebrew)
    })
}

/// What happens after the upgrade: which copy is replaced and which one starts.
#[derive(Debug, PartialEq, Eq)]
struct Relaunch {
    replace: Option<PathBuf>,
    launch: PathBuf,
}

/// A copy outside Homebrew's prefix (normally /Applications/Juicetray.app) is replaced and
/// restarted; Homebrew's own copy (or a bare binary) restarts as the upgraded opt app.
fn plan_relaunch(running: Option<&Path>, homebrew: &Homebrew) -> Relaunch {
    match running {
        Some(bundle) if !bundle.starts_with(&homebrew.prefix) => Relaunch {
            replace: Some(bundle.to_owned()),
            launch: bundle.to_owned(),
        },
        _ => Relaunch {
            replace: None,
            launch: homebrew.app(),
        },
    }
}

/// Hidden siblings of `target`: the new copy is staged in the first, the old one parked in the
/// second during the swap. Same folder, so both renames stay on one volume.
fn swap_paths(target: &Path) -> (PathBuf, PathBuf) {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let sibling = |suffix: &str| target.with_file_name(format!(".{name}.{suffix}"));
    (sibling("new"), sibling("old"))
}

/// `CFBundleShortVersionString` from an XML Info.plist, as the bundle script writes it.
fn bundle_version(plist: &str) -> Option<&str> {
    let (_, after_key) = plist.split_once("<key>CFBundleShortVersionString</key>")?;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    let (version, _) = value.split_once("</string>")?;
    Some(version.trim()).filter(|version| !version.is_empty())
}

/// Splits curl's output into the body and the status `--write-out` appends on its own line.
fn parse_curl_reply(stdout: &str) -> Result<(u16, String), String> {
    let (body, code) = stdout
        .rsplit_once('\n')
        .ok_or("curl printed no HTTP status")?;
    let status = code
        .trim()
        .parse()
        .map_err(|_| format!("curl printed an odd HTTP status {code:?}"))?;
    Ok((status, body.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARM_BREW: &str = "/opt/homebrew/bin/brew";
    const ARM_APP: &str = "/opt/homebrew/opt/juicetray/Juicetray.app";
    const INTEL_BREW: &str = "/usr/local/bin/brew";
    const INTEL_APP: &str = "/usr/local/opt/juicetray/Juicetray.app";

    fn found(paths: &[&str]) -> Option<Homebrew> {
        find_homebrew(|path| paths.iter().any(|existing| Path::new(existing) == path))
    }

    fn arm() -> Homebrew {
        Homebrew {
            brew: ARM_BREW.into(),
            prefix: "/opt/homebrew".into(),
        }
    }

    #[test]
    fn homebrew_needs_both_brew_and_the_installed_app() {
        let homebrew = found(&[ARM_BREW, ARM_APP]).unwrap();
        assert_eq!(homebrew, arm());
        assert_eq!(homebrew.app(), Path::new(ARM_APP));
        assert_eq!(
            found(&[INTEL_BREW, INTEL_APP]).map(|found| found.prefix),
            Some("/usr/local".into())
        );
        // brew without the formula, or the app without brew: offer the download page.
        assert_eq!(found(&[ARM_BREW]), None);
        assert_eq!(found(&[ARM_APP]), None);
        assert_eq!(found(&[]), None);
        // Each prefix pairs its own brew with its own app.
        assert_eq!(found(&[ARM_BREW, INTEL_APP]), None);
    }

    #[test]
    fn apple_silicon_homebrew_wins_when_both_have_the_app() {
        let both = [ARM_BREW, ARM_APP, INTEL_BREW, INTEL_APP];
        assert_eq!(found(&both), Some(arm()));
        // Only the Intel prefix has the app: that one.
        let intel = found(&[ARM_BREW, INTEL_BREW, INTEL_APP]).unwrap();
        assert_eq!(intel.brew, Path::new(INTEL_BREW));
    }

    #[test]
    fn a_copy_outside_homebrew_is_replaced_and_restarted() {
        let applications = Path::new("/Applications/Juicetray.app");
        assert_eq!(
            plan_relaunch(Some(applications), &arm()),
            Relaunch {
                replace: Some(applications.into()),
                launch: applications.into(),
            }
        );
    }

    #[test]
    fn homebrews_own_copy_or_a_bare_binary_restarts_the_opt_app() {
        let opt = Relaunch {
            replace: None,
            launch: ARM_APP.into(),
        };
        assert_eq!(plan_relaunch(Some(Path::new(ARM_APP)), &arm()), opt);
        let cellar = Path::new("/opt/homebrew/Cellar/juicetray/0.4.1/Juicetray.app");
        assert_eq!(plan_relaunch(Some(cellar), &arm()), opt);
        assert_eq!(plan_relaunch(None, &arm()), opt);
        // A prefix that merely starts the same is not Homebrew's.
        let lookalike = Path::new("/opt/homebrew-old/Juicetray.app");
        assert_eq!(
            plan_relaunch(Some(lookalike), &arm()).replace.as_deref(),
            Some(lookalike)
        );
    }

    #[test]
    fn only_a_bundle_outside_applications_offers_to_move() {
        let home = Path::new("/Users/me");
        let offered =
            |path: Option<&str>| bundle_to_move(path.map(PathBuf::from), Some(home)).is_some();
        assert!(offered(Some(ARM_APP)));
        assert!(offered(Some(
            "/opt/homebrew/Cellar/juicetray/0.4.2/Juicetray.app"
        )));
        assert!(offered(Some("/Users/me/Downloads/Juicetray.app")));
        assert!(!offered(Some("/Applications/Juicetray.app")));
        assert!(!offered(Some("/Users/me/Applications/Juicetray.app")));
        // Any ancestor counts, so a subfolder of an Applications folder is left alone.
        assert!(!offered(Some("/Applications/Utilities/Juicetray.app")));
        assert!(!offered(Some("/Users/me/Applications/Tools/Juicetray.app")));
        // Another user's Applications folder is not this user's.
        assert!(offered(Some("/Users/you/Applications/Juicetray.app")));
        // A bare development binary has no bundle to move.
        assert!(!offered(None));
        // Only the /Applications folder itself counts, not a name that starts the same.
        assert!(offered(Some("/Applications Old/Juicetray.app")));
    }

    /// A fresh temp folder that is deleted when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("juicetray-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A fake bundle at `dir/Juicetray.app` whose marker file holds `version`.
    fn fake_bundle(dir: &Path, version: &str) -> PathBuf {
        let bundle = dir.join(APP_NAME);
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/version"), version).unwrap();
        bundle
    }

    fn version_of(bundle: &Path) -> String {
        fs::read_to_string(bundle.join("Contents/version")).unwrap()
    }

    #[test]
    fn replace_bundle_creates_or_swaps_and_leaves_no_siblings() {
        let scratch = Scratch::new("replace");
        let source = fake_bundle(&scratch.0.join("src"), "new");
        let folder = scratch.0.join("apps");
        fs::create_dir_all(&folder).unwrap();
        let target = folder.join(APP_NAME);
        let (staged, backup) = swap_paths(&target);

        // A missing target is created.
        replace_bundle(&source, &target).unwrap();
        assert_eq!(version_of(&target), "new");

        // An existing one is replaced, with no `.new` / `.old` left behind.
        fake_bundle(&folder, "old");
        replace_bundle(&source, &target).unwrap();
        assert_eq!(version_of(&target), "new");
        assert!(!staged.exists() && !backup.exists());

        // Leftovers of a failed earlier run are cleared first.
        fake_bundle(&folder, "old");
        fs::create_dir_all(staged.join("stale")).unwrap();
        fs::create_dir_all(backup.join("stale")).unwrap();
        replace_bundle(&source, &target).unwrap();
        assert_eq!(version_of(&target), "new");
        assert!(!target.join("stale").exists());
        assert!(!staged.exists() && !backup.exists());
    }

    #[test]
    fn swap_paths_are_hidden_siblings() {
        let (staged, backup) = swap_paths(Path::new("/Applications/Juicetray.app"));
        assert_eq!(staged, Path::new("/Applications/.Juicetray.app.new"));
        assert_eq!(backup, Path::new("/Applications/.Juicetray.app.old"));
    }

    #[test]
    fn bundle_version_reads_the_short_version() {
        let plist = "<dict>\n\t<key>CFBundleVersion</key>\n\t<string>9</string>\n\t\
                     <key>CFBundleShortVersionString</key>\n\t<string>0.4.2</string>\n</dict>";
        assert_eq!(bundle_version(plist), Some("0.4.2"));
        assert_eq!(bundle_version("<dict></dict>"), None);
        let empty = "<key>CFBundleShortVersionString</key><string></string>";
        assert_eq!(bundle_version(empty), None);
        let not_string = "<key>CFBundleShortVersionString</key><integer>1</integer>";
        assert_eq!(bundle_version(not_string), None);
    }

    #[test]
    fn curl_reply_splits_body_and_status() {
        assert_eq!(
            parse_curl_reply("{\"a\":\n1}\n200"),
            Ok((200, "{\"a\":\n1}".to_owned()))
        );
        assert_eq!(parse_curl_reply("\n404"), Ok((404, String::new())));
        assert!(parse_curl_reply("200").is_err());
        assert!(parse_curl_reply("body\nabc").is_err());
    }
}
