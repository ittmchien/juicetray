//! The pure half of an update check: reading GitHub's reply and comparing versions.

use super::RELEASES_URL;

/// A release page on this project's own repository is the only link the app will open.
const RELEASE_PAGE_PREFIX: &str = "https://github.com/ittmchien/juicetray/";

/// Release assets are only fetched from this project's own download URLs.
const DOWNLOAD_PREFIX: &str = "https://github.com/ittmchien/juicetray/releases/download/";

/// The portable executable's asset name and the checksum list published next to it.
pub const EXE_ASSET: &str = "juicetray-windows-x86_64.exe";
pub const SUMS_ASSET: &str = "SHA256SUMS.txt";

/// Wait before the next automatic check: a day after the last one (kept across restarts), but
/// never less than `min_wait` after launch; a last check in the future (clock moved) is ignored.
pub fn next_check_wait(
    last: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
    interval: std::time::Duration,
    min_wait: std::time::Duration,
) -> std::time::Duration {
    let since = last.and_then(|last| now.duration_since(last).ok());
    let due = since.map_or(std::time::Duration::ZERO, |since| {
        interval.saturating_sub(since)
    });
    due.max(min_wait)
}

/// Where a release's executable and its checksum list can be downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAssets {
    pub exe_url: String,
    pub sums_url: String,
}

/// A newer release: its version without the leading `v`, the page to download it from and, when
/// the release carries both files, the assets for a one-click update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub assets: Option<UpdateAssets>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    Available(UpdateInfo),
}

/// `major.minor.patch` plus whether it is a final release; field order makes the derived
/// ordering right, and a `-pre` version sorts below the plain one.
/// shortcut: two different pre-releases of one version compare equal; upgrade if the app ever
/// ships pre-release builds that must be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    is_release: bool,
}

impl Version {
    /// `1.2.3`, `v1.2.3`, `1.2.3-rc1` or `1.2.3+build`; anything else is `None`.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let (core, suffix) = text.split_at(text.find(['-', '+']).unwrap_or(text.len()));
        let mut numbers = core.split('.').map(|part| {
            let is_number = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            is_number.then(|| part.parse::<u64>().ok()).flatten()
        });
        let version = Self {
            major: numbers.next()??,
            minor: numbers.next()??,
            patch: numbers.next()??,
            is_release: !suffix.starts_with('-'),
        };
        numbers.next().is_none().then_some(version)
    }
}

/// Turns GitHub's answer into a verdict; a missing release (404) means nothing newer exists.
pub fn evaluate(status: u16, body: &str, current: &str) -> Result<UpdateCheck, String> {
    match status {
        404 => return Ok(UpdateCheck::UpToDate),
        200 => {}
        other => return Err(format!("GitHub answered HTTP {other}")),
    }
    let tag = json_string_field(body, "tag_name").ok_or("the reply has no tag_name")?;
    let latest = Version::parse(&tag).ok_or_else(|| format!("unrecognised version \"{tag}\""))?;
    let running =
        Version::parse(current).ok_or_else(|| format!("unrecognised own version \"{current}\""))?;
    if latest <= running {
        return Ok(UpdateCheck::UpToDate);
    }
    let url = json_string_field(body, "html_url")
        .filter(|url| url.starts_with(RELEASE_PAGE_PREFIX))
        .unwrap_or_else(|| RELEASES_URL.to_owned());
    Ok(UpdateCheck::Available(UpdateInfo {
        version: tag.trim().trim_start_matches(['v', 'V']).to_owned(),
        url,
        assets: update_assets(body, &tag),
    }))
}

/// True the first time `found` is seen: the owner is told once per version.
pub fn should_notify(last_notified: Option<&str>, found: &str) -> bool {
    last_notified != Some(found)
}

/// Both download URLs, or `None` when the release lacks either file or links elsewhere. Both
/// must sit under the release's own tag so the exe and its checksum come from the same release
/// as the version shown.
fn update_assets(json: &str, tag: &str) -> Option<UpdateAssets> {
    let listed = release_assets(json);
    let prefix = format!("{DOWNLOAD_PREFIX}{tag}/");
    let url_of = |wanted: &str| {
        listed
            .iter()
            .find(|(name, url)| name == wanted && url.starts_with(&prefix))
            .map(|(_, url)| url.clone())
    };
    Some(UpdateAssets {
        exe_url: url_of(EXE_ASSET)?,
        sums_url: url_of(SUMS_ASSET)?,
    })
}

/// `(name, browser_download_url)` of each object in the top-level `assets` array; an asset
/// lacking either string is left out. Depth 1 is the release, 2 the array, 3 an asset.
fn release_assets(json: &str) -> Vec<(String, String)> {
    let mut chars = json.chars().peekable();
    let mut found = Vec::new();
    let (mut name, mut url) = (None, None);
    let mut depth = 0_u32;
    let mut in_assets = false;
    while let Some(c) = chars.next() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                if in_assets && c == '}' && depth == 3 {
                    if let (Some(name), Some(url)) = (name.take(), url.take()) {
                        found.push((name, url));
                    }
                } else if in_assets && c == ']' && depth == 2 {
                    break;
                }
                depth = depth.saturating_sub(1);
            }
            '"' => {
                let Some(key) = read_string(&mut chars) else {
                    break;
                };
                skip_whitespace(&mut chars);
                if chars.next_if_eq(&':').is_none() {
                    continue;
                }
                skip_whitespace(&mut chars);
                match (depth, key.as_str()) {
                    (1, "assets") if chars.next_if_eq(&'[').is_some() => {
                        in_assets = true;
                        depth += 1;
                    }
                    (3, "name" | "browser_download_url")
                        if in_assets && chars.next_if_eq(&'"').is_some() =>
                    {
                        let value = read_string(&mut chars);
                        if key == "name" {
                            name = value;
                        } else {
                            url = value;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    found
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// The string value of `key` among the top-level members of a JSON object; nested objects
/// (such as the release's `author`) are skipped.
/// shortcut: `\u` escapes decode one UTF-16 unit, so a surrogate pair becomes two U+FFFD;
/// upgrade if a field that may hold astral characters is ever read.
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let mut chars = json.chars().peekable();
    let mut depth = 0_u32;
    while let Some(c) = chars.next() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            '"' => {
                let text = read_string(&mut chars)?;
                if depth != 1 || text != key {
                    continue;
                }
                skip_whitespace(&mut chars);
                if chars.next_if_eq(&':').is_none() {
                    continue;
                }
                skip_whitespace(&mut chars);
                if chars.next_if_eq(&'"').is_some() {
                    return read_string(&mut chars);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_whitespace(chars: &mut Chars) {
    while chars.next_if(|c| c.is_whitespace()).is_some() {}
}

/// The rest of a string whose opening quote is consumed, escapes decoded; `None` if unterminated.
fn read_string(chars: &mut Chars) -> Option<String> {
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{FFFD}')
                }
                // `"`, `\`, `/` and anything unknown stand for themselves.
                other => other,
            }),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: &str = r#"{"url":"https://api.github.com/x","html_url":"https://github.com/ittmchien/juicetray/releases/tag/v0.2.0","id":1,"author":{"login":"a","html_url":"https://github.com/a"},"tag_name":"v0.2.0","assets":[{"name":"x","html_url":"nested"}]}"#;

    fn check(status: u16, body: &str) -> Result<UpdateCheck, String> {
        evaluate(status, body, "0.1.0")
    }

    #[test]
    fn next_check_waits_a_day_from_the_last_check() {
        use std::time::{Duration, SystemTime};
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        let (day, minute) = (Duration::from_secs(86_400), Duration::from_secs(60));
        let ago = |secs| Some(now - Duration::from_secs(secs));
        assert_eq!(next_check_wait(None, now, day, minute), minute);
        assert_eq!(
            next_check_wait(ago(3_600), now, day, minute),
            day - Duration::from_secs(3_600)
        );
        assert_eq!(next_check_wait(ago(2 * 86_400), now, day, minute), minute);
        assert_eq!(next_check_wait(ago(86_400 - 10), now, day, minute), minute);
        let future = Some(now + Duration::from_secs(600));
        assert_eq!(next_check_wait(future, now, day, minute), minute);
    }

    #[test]
    fn versions_compare_numerically() {
        let v = |text| Version::parse(text).unwrap();
        assert!(v("0.1.1") > v("0.1.0"));
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3+build5"), v("1.2.3"));
    }

    #[test]
    fn a_pre_release_is_older_than_its_release() {
        let v = |text| Version::parse(text).unwrap();
        assert!(v("1.0.0-rc1") < v("1.0.0"));
        assert!(v("1.0.0") > v("1.0.0-rc1"));
        assert!(v("1.0.0-rc1") > v("0.9.9"));
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for text in [
            "", "latest", "1.0", "1.0.0.0", "1.x.0", "1..0", "-1.0.0", "v", "1.0.+2",
        ] {
            assert!(Version::parse(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn extracts_top_level_strings_only() {
        assert_eq!(
            json_string_field(NORMAL, "tag_name").as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(
            json_string_field(NORMAL, "html_url").as_deref(),
            Some("https://github.com/ittmchien/juicetray/releases/tag/v0.2.0")
        );
        // A nested object's field is not the release's own.
        let nested_only = r#"{"author":{"html_url":"https://github.com/a"}}"#;
        assert_eq!(json_string_field(nested_only, "html_url"), None);
    }

    #[test]
    fn extraction_handles_missing_and_odd_fields() {
        assert_eq!(json_string_field("{}", "tag_name"), None);
        assert_eq!(json_string_field("", "tag_name"), None);
        assert_eq!(json_string_field(r#"{"tag_name":null}"#, "tag_name"), None);
        assert_eq!(json_string_field(r#"{"tag_name":1}"#, "tag_name"), None);
        assert_eq!(json_string_field(r#"{"tag_name":"v1"#, "tag_name"), None);
        // The key's text inside a value, or braces inside a string, change nothing.
        let tricky = r#"{"body":"tag_name } ] {","tag_name" : "v2"}"#;
        assert_eq!(json_string_field(tricky, "tag_name").as_deref(), Some("v2"));
        let as_value = r#"{"name":"tag_name","tag_name":"v3"}"#;
        assert_eq!(
            json_string_field(as_value, "tag_name").as_deref(),
            Some("v3")
        );
    }

    #[test]
    fn extraction_decodes_escapes() {
        let json = r#"{"tag_name":"v1\u0041\n\"q\"\\\/","html_url":"a\/b"}"#;
        assert_eq!(
            json_string_field(json, "tag_name").as_deref(),
            Some("v1A\n\"q\"\\/")
        );
        assert_eq!(json_string_field(json, "html_url").as_deref(), Some("a/b"));
        // A bad escape never panics.
        assert_eq!(
            json_string_field(r#"{"tag_name":"\uZZZZ"}"#, "tag_name").as_deref(),
            Some("\u{FFFD}")
        );
    }

    #[test]
    fn newer_release_is_available_with_its_page() {
        assert_eq!(
            check(200, NORMAL),
            Ok(UpdateCheck::Available(UpdateInfo {
                version: "0.2.0".into(),
                url: "https://github.com/ittmchien/juicetray/releases/tag/v0.2.0".into(),
                assets: None,
            }))
        );
    }

    const DL: &str = "https://github.com/ittmchien/juicetray/releases/download/v0.2.0";

    fn release_with_assets(assets: &str) -> String {
        format!(
            r#"{{"tag_name":"v0.2.0","author":{{"name":"x"}},"assets":[{assets}],"body":"[]"}}"#
        )
    }

    fn asset(name: &str, url: &str) -> String {
        format!(
            r#"{{"name":"{name}","uploader":{{"name":"u","browser_download_url":"nested"}},"browser_download_url":"{url}"}}"#
        )
    }

    #[test]
    fn both_assets_are_found_among_others() {
        let body = release_with_assets(&format!(
            "{},{},{}",
            asset("notes.txt", &format!("{DL}/notes.txt")),
            asset(SUMS_ASSET, &format!("{DL}/{SUMS_ASSET}")),
            asset(EXE_ASSET, &format!("{DL}/{EXE_ASSET}")),
        ));
        assert_eq!(
            update_assets(&body, "v0.2.0"),
            Some(UpdateAssets {
                exe_url: format!("{DL}/{EXE_ASSET}"),
                sums_url: format!("{DL}/{SUMS_ASSET}"),
            })
        );
        // The check hands them on with the verdict.
        let Ok(UpdateCheck::Available(info)) = check(200, &body) else {
            panic!("expected an update");
        };
        assert!(info.assets.is_some());
    }

    #[test]
    fn a_missing_asset_or_foreign_url_gives_no_assets() {
        let exe = asset(EXE_ASSET, &format!("{DL}/{EXE_ASSET}"));
        let sums = asset(SUMS_ASSET, &format!("{DL}/{SUMS_ASSET}"));
        let assets_of = |body: &str| update_assets(body, "v0.2.0");
        assert_eq!(assets_of(&release_with_assets(&exe)), None);
        assert_eq!(assets_of(&release_with_assets(&sums)), None);
        assert_eq!(assets_of(&release_with_assets("")), None);
        assert_eq!(assets_of(r#"{"tag_name":"v1"}"#), None);
        let evil = asset(EXE_ASSET, "https://evil.example/juicetray.exe");
        let with_evil = release_with_assets(&format!("{evil},{sums}"));
        assert_eq!(assets_of(&with_evil), None);
        // Another repository's download link is not accepted either.
        let other = asset(
            EXE_ASSET,
            "https://github.com/someone/else/releases/download/v1/x",
        );
        let with_other = release_with_assets(&format!("{other},{sums}"));
        assert_eq!(assets_of(&with_other), None);
        // Only the exact name counts: a similar file name or a nested "name" does not.
        let similar = asset("juicetray-windows-x86_64.exe.sig", &format!("{DL}/x"));
        let with_similar = release_with_assets(&format!("{similar},{sums}"));
        assert_eq!(assets_of(&with_similar), None);
    }

    #[test]
    fn assets_from_another_tag_are_rejected() {
        let other_dl = "https://github.com/ittmchien/juicetray/releases/download/v0.1.0";
        let exe = asset(EXE_ASSET, &format!("{DL}/{EXE_ASSET}"));
        let sums = asset(SUMS_ASSET, &format!("{DL}/{SUMS_ASSET}"));
        let old_exe = asset(EXE_ASSET, &format!("{other_dl}/{EXE_ASSET}"));
        let old_sums = asset(SUMS_ASSET, &format!("{other_dl}/{SUMS_ASSET}"));
        // Either file from another release, or both, gives no assets.
        for (first, second) in [(&old_exe, &sums), (&exe, &old_sums), (&old_exe, &old_sums)] {
            let body = release_with_assets(&format!("{first},{second}"));
            assert_eq!(update_assets(&body, "v0.2.0"), None);
            assert!(matches!(
                check(200, &body),
                Ok(UpdateCheck::Available(UpdateInfo { assets: None, .. }))
            ));
        }
        // A tag that merely starts the same does not match.
        let body = release_with_assets(&format!("{exe},{sums}"));
        assert_eq!(update_assets(&body, "v0.2"), None);
    }

    #[test]
    fn a_truncated_asset_list_gives_no_assets() {
        let exe = asset(EXE_ASSET, &format!("{DL}/{EXE_ASSET}"));
        let cut = format!(r#"{{"tag_name":"v0.2.0","assets":[{exe},{{"name":"SHA2"#);
        assert_eq!(update_assets(&cut, "v0.2.0"), None);
    }

    #[test]
    fn same_older_or_missing_release_is_up_to_date() {
        let tag = |tag: &str| format!(r#"{{"tag_name":"{tag}"}}"#);
        assert_eq!(check(200, &tag("v0.1.0")), Ok(UpdateCheck::UpToDate));
        assert_eq!(check(200, &tag("0.0.9")), Ok(UpdateCheck::UpToDate));
        assert_eq!(check(200, &tag("v0.1.0-rc1")), Ok(UpdateCheck::UpToDate));
        assert_eq!(check(404, "{}"), Ok(UpdateCheck::UpToDate));
        // Running a pre-release, the final release of that version is newer.
        assert!(matches!(
            evaluate(200, &tag("0.2.0"), "0.2.0-dev"),
            Ok(UpdateCheck::Available(_))
        ));
    }

    #[test]
    fn foreign_or_missing_release_link_falls_back_to_the_releases_page() {
        let body = |url: &str| format!(r#"{{"tag_name":"v0.2.0","html_url":"{url}"}}"#);
        for url in ["https://evil.example/x", "file:///c:/windows/calc.exe", ""] {
            let Ok(UpdateCheck::Available(info)) = check(200, &body(url)) else {
                panic!("expected an update for {url:?}");
            };
            assert_eq!(info.url, RELEASES_URL);
        }
        let Ok(UpdateCheck::Available(info)) = check(200, r#"{"tag_name":"v0.2.0"}"#) else {
            panic!("expected an update");
        };
        assert_eq!(info.url, RELEASES_URL);
    }

    #[test]
    fn bad_replies_are_errors() {
        assert_eq!(check(403, ""), Err("GitHub answered HTTP 403".into()));
        assert_eq!(check(500, "{}"), Err("GitHub answered HTTP 500".into()));
        assert!(check(200, "{}").unwrap_err().contains("tag_name"));
        assert!(
            check(200, r#"{"tag_name":"nightly"}"#)
                .unwrap_err()
                .contains("nightly")
        );
    }

    #[test]
    fn a_version_is_announced_once() {
        assert!(should_notify(None, "0.2.0"));
        assert!(!should_notify(Some("0.2.0"), "0.2.0"));
        // A still newer release is news again.
        assert!(should_notify(Some("0.2.0"), "0.3.0"));
    }
}
